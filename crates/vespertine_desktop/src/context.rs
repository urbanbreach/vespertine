//! Bounded accessibility context queries (PLT-013).
//!
//! For one identified top-level window of another application (target process
//! id plus exact title) this module reads a bounded textual outline of the
//! window's accessibility tree over AT-SPI so a screenshot can be accompanied
//! by text. The query is strictly bounded in depth, node count, output bytes
//! and wall-clock time, matches its target unambiguously or returns nothing,
//! and everything it returns is an *untrusted observation* (see
//! [`untrusted`]).
//!
//! All bus access goes through the [`AccessibilityBus`] and
//! [`AccessibilityConnection`] traits and all time through [`ClockSource`], so
//! tests drive the query with the in-memory [`fake`] bus and a manual clock.
//! [`ContextQuery::new`] uses the real AT-SPI bus (Linux only).

mod clock;
pub mod fake;
mod select;
pub mod untrusted;
mod walk;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) mod zbus_bus;

pub use clock::{ClockSource, SystemClock};
pub use untrusted::{
    DEFAULT_UNTRUSTED_MARKER, ObservedWindow, escape_observed_text,
    serialize_untrusted_observations,
};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use zbus_bus::{ZbusAccessibilityBus, probe_bus_available};

use futures::future::{BoxFuture, FutureExt};
use parking_lot::Mutex;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

/// Snapshot format version produced by this module.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;
/// Maximum depth of visited nodes (the window itself is depth 0).
pub const DEFAULT_MAX_DEPTH: usize = 24;
/// Maximum number of visited (dequeued) nodes.
pub const DEFAULT_MAX_NODES: usize = 1500;
/// Maximum size of the snapshot content in bytes (96 KiB).
pub const DEFAULT_MAX_BYTES: usize = 96 * 1024;
/// Maximum number of characters of any text read or cleaned field.
pub const DEFAULT_MAX_VALUE_CHARS: usize = 4096;
/// Overall wall-clock budget of one query.
pub const DEFAULT_DEADLINE: Duration = Duration::from_millis(900);

/// An object on the accessibility bus: the owner's bus name and object path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    /// Unique bus name of the application that owns the object.
    pub bus_name: String,
    /// Object path of the accessible.
    pub path: String,
}

impl ObjectRef {
    /// Creates a reference from a bus name and an object path.
    pub fn new(bus_name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            bus_name: bus_name.into(),
            path: path.into(),
        }
    }
}

/// The role of an accessible node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRole {
    /// The AT-SPI role name (for example `push button`).
    pub name: String,
    /// Whether the role is the password-text role. The text of such nodes is
    /// never read.
    pub password: bool,
}

/// The subset of AT-SPI states the query consults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeState {
    /// The `active` state (a window the user is working with).
    pub active: bool,
    /// The `focused` state.
    pub focused: bool,
}

/// A failed bus request. Carries only a diagnostic message.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct BusError(pub String);

/// Entry point to the accessibility bus.
pub trait AccessibilityBus: Send + Sync + 'static {
    /// Connects to the accessibility bus. The returned future is part of the
    /// query budget and is dropped when the query expires or is cancelled.
    fn connect(&self) -> BoxFuture<'static, Result<Arc<dyn AccessibilityConnection>, BusError>>;
}

/// A connection to the accessibility bus. Every method is one bus request and
/// may fail or stall independently; implementations must not hold locks across
/// the returned future's suspension points.
pub trait AccessibilityConnection: Send + Sync {
    /// Children of the registry root: the root accessible of every application.
    fn applications(&self) -> BoxFuture<'_, Result<Vec<ObjectRef>, BusError>>;
    /// Process id of the connection that owns `bus_name`, as reported by the
    /// accessibility bus daemon (never derived from names).
    fn process_id<'a>(&'a self, bus_name: &'a str) -> BoxFuture<'a, Result<u32, BusError>>;
    /// Children of an accessible, in order.
    fn children<'a>(
        &'a self,
        node: &'a ObjectRef,
    ) -> BoxFuture<'a, Result<Vec<ObjectRef>, BusError>>;
    /// Accessible name.
    fn name<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>>;
    /// Accessible description.
    fn description<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>>;
    /// Role of the accessible.
    fn role<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeRole, BusError>>;
    /// Active/focused state of the accessible.
    fn state<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeState, BusError>>;
    /// Whether the accessible implements the AT-SPI Text interface.
    fn implements_text<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<bool, BusError>>;
    /// Character count of the Text interface.
    fn character_count<'a>(&'a self, node: &'a ObjectRef)
    -> BoxFuture<'a, Result<usize, BusError>>;
    /// Text between character offsets `start..end` of the Text interface.
    fn text<'a>(
        &'a self,
        node: &'a ObjectRef,
        start: usize,
        end: usize,
    ) -> BoxFuture<'a, Result<String, BusError>>;
}

/// Limits of one context query. Defaults are the Parameters of PLT-013.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBudgets {
    /// Deepest node depth that is visited (root is 0).
    pub max_depth: usize,
    /// Number of nodes that may be visited (counted when dequeued).
    pub max_nodes: usize,
    /// Output size limit in bytes.
    pub max_bytes: usize,
    /// Character limit for text read and for every cleaned field.
    pub max_value_chars: usize,
    /// One wall-clock budget covering connect, requests, selection, traversal
    /// and re-selection.
    pub deadline: Duration,
}

impl Default for ContextBudgets {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
            max_bytes: DEFAULT_MAX_BYTES,
            max_value_chars: DEFAULT_MAX_VALUE_CHARS,
            deadline: DEFAULT_DEADLINE,
        }
    }
}

/// What to query: the target process and the exact top-level window title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRequest {
    /// Target process id; must be non-zero.
    pub process_id: u32,
    /// Exact, case-sensitive top-level window title; must not be blank. It is
    /// compared without trimming or normalization.
    pub title: String,
    /// Budgets; default to the PLT-013 parameters.
    pub budgets: ContextBudgets,
}

impl ContextRequest {
    /// A request with default budgets.
    pub fn new(process_id: u32, title: impl Into<String>) -> Self {
        Self {
            process_id,
            title: title.into(),
            budgets: ContextBudgets::default(),
        }
    }

    /// Replaces the budgets.
    pub fn with_budgets(mut self, budgets: ContextBudgets) -> Self {
        self.budgets = budgets;
        self
    }
}

/// A bounded textual outline of an accessibility tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessibilitySnapshot {
    /// Format version, currently [`SNAPSHOT_FORMAT_VERSION`].
    pub version: u32,
    /// One line per node that has at least one field.
    pub content: String,
    /// Whether a limit stopped the walk before the tree was exhausted.
    pub truncated: bool,
}

impl AccessibilitySnapshot {
    /// The empty screenshot-only snapshot: version 1, empty content, not
    /// truncated.
    pub fn empty() -> Self {
        Self {
            version: SNAPSHOT_FORMAT_VERSION,
            content: String::new(),
            truncated: false,
        }
    }
}

/// A successful query result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessibilityContext {
    /// Accessible name of the target's application (an untrusted observation).
    pub application_name: String,
    /// The window's outline.
    pub snapshot: AccessibilitySnapshot,
}

/// Why a query produced no context. Every reason means "screenshot-only".
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContextUnavailable {
    /// Missing metadata, no matching window, a window that is not active or
    /// focused, or a window whose identity changed during traversal.
    #[error("no matching accessible window")]
    NoMatch,
    /// Several windows or applications matched.
    #[error("several accessible windows match")]
    Ambiguous,
    /// The 900 ms budget (or the configured deadline) ran out.
    #[error("the accessibility query ran out of time")]
    Expired,
    /// The caller cancelled the query.
    #[error("the accessibility query was cancelled")]
    Cancelled,
    /// The accessibility bus could not be reached or did not answer.
    #[error("the accessibility bus is unavailable")]
    UnavailableBus,
}

impl ContextUnavailable {
    /// The screenshot-only snapshot that accompanies this outcome.
    pub fn screenshot_only(self) -> AccessibilitySnapshot {
        AccessibilitySnapshot::empty()
    }
}

/// Handle that cancels a [`ContextQuery`] from anywhere.
#[derive(Clone, Default)]
pub struct CancelHandle {
    state: Arc<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl CancelHandle {
    /// Cancels the query: its pending work is dropped the next time the query
    /// is polled, which this call schedules, and it resolves to
    /// [`ContextUnavailable::Cancelled`].
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        let waker = self.state.waker.lock().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Whether [`cancel`](Self::cancel) was called.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    fn register(&self, waker: &Waker) {
        let mut slot = self.state.waker.lock();
        match slot.as_mut() {
            Some(existing) if existing.will_wake(waker) => {}
            _ => *slot = Some(waker.clone()),
        }
    }
}

type QueryWork = BoxFuture<'static, Result<AccessibilityContext, ContextUnavailable>>;

/// A cancellable accessibility context query. Dropping it cancels it; it never
/// holds a lock or a service across polls. It resolves to the context or to
/// the reason there is none.
#[must_use = "a ContextQuery does nothing unless polled"]
pub struct ContextQuery {
    work: Option<QueryWork>,
    timer: BoxFuture<'static, ()>,
    cancel: CancelHandle,
}

impl ContextQuery {
    /// A query against the real AT-SPI accessibility bus using the system
    /// clock.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn new(request: ContextRequest) -> Self {
        Self::with_services(
            Arc::new(ZbusAccessibilityBus),
            Arc::new(SystemClock),
            request,
        )
    }

    /// A query against the given bus and clock. The budget starts now.
    pub fn with_services(
        bus: Arc<dyn AccessibilityBus>,
        clock: Arc<dyn ClockSource>,
        request: ContextRequest,
    ) -> Self {
        let started = clock.now();
        let deadline = started + request.budgets.deadline;
        let timer = clock.sleep(request.budgets.deadline);
        let work = run_query(bus, clock, request, deadline).boxed();
        Self {
            work: Some(work),
            timer,
            cancel: CancelHandle::default(),
        }
    }

    /// A handle that cancels this query.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }
}

impl Future for ContextQuery {
    type Output = Result<AccessibilityContext, ContextUnavailable>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        this.cancel.register(cx.waker());
        if this.cancel.is_cancelled() {
            this.work = None;
            return Poll::Ready(Err(ContextUnavailable::Cancelled));
        }
        let Some(work) = this.work.as_mut() else {
            panic!("ContextQuery polled after completion");
        };
        if let Poll::Ready(result) = work.poll_unpin(cx) {
            this.work = None;
            return Poll::Ready(result);
        }
        if this.timer.poll_unpin(cx).is_ready() {
            this.work = None;
            return Poll::Ready(Err(ContextUnavailable::Expired));
        }
        Poll::Pending
    }
}

async fn run_query(
    bus: Arc<dyn AccessibilityBus>,
    clock: Arc<dyn ClockSource>,
    request: ContextRequest,
    deadline: std::time::Instant,
) -> Result<AccessibilityContext, ContextUnavailable> {
    if request.process_id == 0 || request.title.trim().is_empty() {
        return Err(ContextUnavailable::NoMatch);
    }
    let connection = bus
        .connect()
        .await
        .map_err(|_| ContextUnavailable::UnavailableBus)?;
    let connection = connection.as_ref();

    let selection = select::select(connection, request.process_id, &request.title).await?;
    let snapshot = walk::walk(
        connection,
        selection.window.object.clone(),
        &request.budgets,
        clock.as_ref(),
        deadline,
    )
    .await;
    // Identity is verified again after the traversal, inside the same budget.
    let reselection = select::select(connection, request.process_id, &request.title).await?;
    if reselection.window != selection.window {
        return Err(ContextUnavailable::NoMatch);
    }
    if clock.now() >= deadline {
        return Err(ContextUnavailable::Expired);
    }
    Ok(AccessibilityContext {
        application_name: selection.application_name,
        snapshot,
    })
}

#[cfg(test)]
mod tests;

//! In-memory accessibility bus and manual clock.
//!
//! [`FakeBus`] serves a scripted accessibility tree through the same traits as
//! the real AT-SPI bus, records every request, and can add per-request or
//! connect latency measured on an injected [`ClockSource`]. [`ManualClock`] is
//! a virtual clock whose timers fire only when the clock is advanced, so
//! deadline behavior is tested without sleeping. They are used by this crate's
//! tests and by the `a11y_context` example, and are available to consumers that
//! need to test code built on [`ContextQuery`](super::ContextQuery).

use super::{
    AccessibilityBus, AccessibilityConnection, BusError, ClockSource, NodeRole, NodeState,
    ObjectRef,
};
use futures::future::{BoxFuture, FutureExt};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------------
// Manual clock
// ---------------------------------------------------------------------------

struct ClockInner {
    elapsed: Duration,
    next_id: u64,
    timers: BTreeMap<u64, (Duration, Option<Waker>)>,
}

/// A virtual clock. Time passes only through [`advance`](Self::advance),
/// [`advance_to_next_timer`](Self::advance_to_next_timer) or
/// [`run`](Self::run).
#[derive(Clone)]
pub struct ManualClock {
    base: Instant,
    inner: Arc<Mutex<ClockInner>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualClock {
    /// A clock at virtual time zero.
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            inner: Arc::new(Mutex::new(ClockInner {
                elapsed: Duration::ZERO,
                next_id: 0,
                timers: BTreeMap::new(),
            })),
        }
    }

    /// Virtual time elapsed since creation.
    pub fn elapsed(&self) -> Duration {
        self.inner.lock().elapsed
    }

    /// Number of timers that have not fired or been dropped.
    pub fn pending_timers(&self) -> usize {
        self.inner.lock().timers.len()
    }

    /// Advances virtual time and wakes the timers that became due.
    pub fn advance(&self, by: Duration) {
        let wakers = {
            let mut inner = self.inner.lock();
            inner.elapsed += by;
            let elapsed = inner.elapsed;
            Self::take_due(&mut inner, elapsed)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Advances virtual time to the earliest pending timer. Returns `false`
    /// when there is none.
    pub fn advance_to_next_timer(&self) -> bool {
        let wakers = {
            let mut inner = self.inner.lock();
            let Some(due) = inner.timers.values().map(|(due, _)| *due).min() else {
                return false;
            };
            inner.elapsed = inner.elapsed.max(due);
            let elapsed = inner.elapsed;
            Self::take_due(&mut inner, elapsed)
        };
        for waker in wakers {
            waker.wake();
        }
        true
    }

    fn take_due(inner: &mut ClockInner, elapsed: Duration) -> Vec<Waker> {
        inner
            .timers
            .values_mut()
            .filter(|(due, _)| *due <= elapsed)
            .filter_map(|(_, waker)| waker.take())
            .collect()
    }

    /// Polls `future` to completion on the calling thread in virtual time:
    /// whenever it is pending, the clock jumps to the next timer. Panics when
    /// the future is pending with no timer to wait for.
    pub fn run<F: Future>(&self, future: F) -> F::Output {
        let mut future = Box::pin(future);
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
            assert!(
                self.advance_to_next_timer(),
                "future is pending and the manual clock has no timer to advance to"
            );
        }
    }
}

struct ManualSleep {
    inner: Arc<Mutex<ClockInner>>,
    id: u64,
    due: Duration,
}

impl Future for ManualSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.inner.lock();
        if inner.elapsed >= self.due {
            inner.timers.remove(&self.id);
            return Poll::Ready(());
        }
        if let Some((_, waker)) = inner.timers.get_mut(&self.id) {
            *waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for ManualSleep {
    fn drop(&mut self) {
        self.inner.lock().timers.remove(&self.id);
    }
}

impl ClockSource for ManualClock {
    fn now(&self) -> Instant {
        self.base + self.inner.lock().elapsed
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        let (id, due) = {
            let mut inner = self.inner.lock();
            let id = inner.next_id;
            inner.next_id += 1;
            let due = inner.elapsed + duration;
            inner.timers.insert(id, (due, None));
            (id, due)
        };
        ManualSleep {
            inner: self.inner.clone(),
            id,
            due,
        }
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// Fake bus
// ---------------------------------------------------------------------------

/// Identifies a node of a [`FakeBus`] tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

/// Describes a node to add to a [`FakeBus`].
#[derive(Clone, Debug, Default)]
pub struct FakeNodeSpec {
    role: String,
    name: String,
    description: String,
    text: Option<String>,
    password: bool,
    active: bool,
    focused: bool,
    failing: bool,
}

impl FakeNodeSpec {
    /// A node with the given AT-SPI role name.
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            ..Self::default()
        }
    }

    /// Sets the accessible name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the accessible description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Makes the node implement the Text interface with this content.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Gives the node the password role (named `password text`).
    pub fn password(mut self) -> Self {
        self.password = true;
        self.role = "password text".to_owned();
        self
    }

    /// Sets the active state.
    pub fn active(mut self) -> Self {
        self.active = true;
        self
    }

    /// Sets the focused state.
    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }

    /// Makes every property request on the node fail (child lists still work).
    pub fn failing(mut self) -> Self {
        self.failing = true;
        self
    }
}

struct FakeNode {
    object: ObjectRef,
    spec: FakeNodeSpec,
    children: Vec<usize>,
}

/// A request received by a [`FakeBus`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeRequest {
    /// Registry root children.
    Applications,
    /// Process id for a bus name.
    ProcessId(String),
    /// Child list of a node.
    Children(ObjectRef),
    /// Accessible name.
    Name(ObjectRef),
    /// Accessible description.
    Description(ObjectRef),
    /// Role.
    Role(ObjectRef),
    /// State set.
    State(ObjectRef),
    /// Interface list check for Text.
    ImplementsText(ObjectRef),
    /// Text character count.
    CharacterCount(ObjectRef),
    /// Text read of `start..end`.
    Text {
        /// Node read.
        node: ObjectRef,
        /// First character offset.
        start: usize,
        /// End character offset.
        end: usize,
    },
}

type RequestHook = Arc<dyn Fn(&FakeBus, &FakeRequest) + Send + Sync>;

struct FakeInner {
    nodes: Vec<FakeNode>,
    applications: Vec<usize>,
    process_ids: HashMap<String, u32>,
    by_object: HashMap<ObjectRef, usize>,
    request_latency: Duration,
    connect_latency: Duration,
    connect_hangs: bool,
    connect_fails: bool,
    log: Vec<FakeRequest>,
    hook: Option<RequestHook>,
}

struct InFlightGuard(Arc<AtomicUsize>);

impl InFlightGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An in-memory accessibility bus serving a scripted tree. Cloning shares the
/// same tree, log and settings.
#[derive(Clone)]
pub struct FakeBus {
    clock: Arc<dyn ClockSource>,
    inner: Arc<Mutex<FakeInner>>,
    in_flight: Arc<AtomicUsize>,
}

impl FakeBus {
    /// An empty bus whose latencies are measured on `clock`.
    pub fn new(clock: Arc<dyn ClockSource>) -> Self {
        Self {
            clock,
            inner: Arc::new(Mutex::new(FakeInner {
                nodes: Vec::new(),
                applications: Vec::new(),
                process_ids: HashMap::new(),
                by_object: HashMap::new(),
                request_latency: Duration::ZERO,
                connect_latency: Duration::ZERO,
                connect_hangs: false,
                connect_fails: false,
                log: Vec::new(),
                hook: None,
            })),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Adds an application (a child of the registry root) owned by connection
    /// `bus_name` of process `process_id`.
    pub fn add_application(
        &self,
        bus_name: impl Into<String>,
        process_id: u32,
        application_name: impl Into<String>,
    ) -> NodeId {
        let bus_name = bus_name.into();
        let mut inner = self.inner.lock();
        inner.process_ids.insert(bus_name.clone(), process_id);
        let object = ObjectRef::new(bus_name, "/org/a11y/atspi/accessible/root");
        let id = Self::push(
            &mut inner,
            object,
            FakeNodeSpec::new("application").name(application_name),
        );
        inner.applications.push(id);
        NodeId(id)
    }

    /// Adds a top-level window with the given title below an application.
    pub fn add_window(
        &self,
        application: NodeId,
        title: impl Into<String>,
        active: bool,
    ) -> NodeId {
        let mut spec = FakeNodeSpec::new("frame").name(title);
        if active {
            spec = spec.active();
        }
        self.add_node(application, spec)
    }

    /// Adds a node as the last child of `parent`.
    pub fn add_node(&self, parent: NodeId, spec: FakeNodeSpec) -> NodeId {
        let mut inner = self.inner.lock();
        let bus_name = inner.nodes[parent.0].object.bus_name.clone();
        let index = inner.nodes.len();
        let object = ObjectRef::new(bus_name, format!("/org/a11y/atspi/accessible/{index}"));
        let id = Self::push(&mut inner, object, spec);
        inner.nodes[parent.0].children.push(id);
        NodeId(id)
    }

    fn push(inner: &mut FakeInner, object: ObjectRef, spec: FakeNodeSpec) -> usize {
        let id = inner.nodes.len();
        inner.by_object.insert(object.clone(), id);
        inner.nodes.push(FakeNode {
            object,
            spec,
            children: Vec::new(),
        });
        id
    }

    /// The bus reference of a node.
    pub fn object(&self, node: NodeId) -> ObjectRef {
        self.inner.lock().nodes[node.0].object.clone()
    }

    /// Sets or clears the active state of a node.
    pub fn set_active(&self, node: NodeId, active: bool) {
        self.inner.lock().nodes[node.0].spec.active = active;
    }

    /// Renames a node.
    pub fn set_name(&self, node: NodeId, name: impl Into<String>) {
        self.inner.lock().nodes[node.0].spec.name = name.into();
    }

    /// Makes every request take `latency` on the injected clock.
    pub fn set_request_latency(&self, latency: Duration) {
        self.inner.lock().request_latency = latency;
    }

    /// Makes connecting take `latency` on the injected clock.
    pub fn set_connect_latency(&self, latency: Duration) {
        self.inner.lock().connect_latency = latency;
    }

    /// Makes connecting never complete.
    pub fn set_connect_hangs(&self, hangs: bool) {
        self.inner.lock().connect_hangs = hangs;
    }

    /// Makes connecting fail immediately (no accessibility bus).
    pub fn set_connect_fails(&self, fails: bool) {
        self.inner.lock().connect_fails = fails;
    }

    /// Registers a callback that runs for every request, after it is logged
    /// and before it is answered, with no internal lock held. Tests use it to
    /// change the tree mid-query.
    pub fn set_request_hook(&self, hook: impl Fn(&FakeBus, &FakeRequest) + Send + Sync + 'static) {
        self.inner.lock().hook = Some(Arc::new(hook));
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<FakeRequest> {
        self.inner.lock().log.clone()
    }

    /// Number of connect/request futures currently suspended or running.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Whether the internal lock is free right now (it is never held across a
    /// suspension point).
    pub fn is_unlocked(&self) -> bool {
        self.inner.try_lock().is_some()
    }

    fn request<T: Send + 'static>(
        &self,
        request: FakeRequest,
        answer: impl FnOnce(&FakeInner) -> Result<T, BusError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, BusError>> {
        let bus = self.clone();
        async move {
            let _guard = InFlightGuard::new(&bus.in_flight);
            let (hook, latency) = {
                let mut inner = bus.inner.lock();
                inner.log.push(request.clone());
                (inner.hook.clone(), inner.request_latency)
            };
            if let Some(hook) = hook {
                hook(&bus, &request);
            }
            if !latency.is_zero() {
                bus.clock.sleep(latency).await;
            }
            let inner = bus.inner.lock();
            answer(&inner)
        }
        .boxed()
    }

    fn node_request<T: Send + 'static>(
        &self,
        request: FakeRequest,
        node: &ObjectRef,
        answer: impl FnOnce(&FakeNode) -> Result<T, BusError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, BusError>> {
        let node = node.clone();
        self.request(request, move |inner| {
            let found = inner
                .by_object
                .get(&node)
                .map(|index| &inner.nodes[*index])
                .ok_or_else(|| {
                    BusError(format!("no such accessible {}{}", node.bus_name, node.path))
                })?;
            answer(found)
        })
    }
}

fn property_result<T>(node: &FakeNode, value: impl FnOnce(&FakeNode) -> T) -> Result<T, BusError> {
    if node.spec.failing {
        Err(BusError("property read failed".into()))
    } else {
        Ok(value(node))
    }
}

impl AccessibilityBus for FakeBus {
    fn connect(&self) -> BoxFuture<'static, Result<Arc<dyn AccessibilityConnection>, BusError>> {
        let bus = self.clone();
        async move {
            let _guard = InFlightGuard::new(&bus.in_flight);
            let (latency, hangs, fails) = {
                let inner = bus.inner.lock();
                (
                    inner.connect_latency,
                    inner.connect_hangs,
                    inner.connect_fails,
                )
            };
            if hangs {
                futures::future::pending::<()>().await;
            }
            if !latency.is_zero() {
                bus.clock.sleep(latency).await;
            }
            if fails {
                return Err(BusError("no accessibility bus".into()));
            }
            Ok(Arc::new(bus.clone()) as Arc<dyn AccessibilityConnection>)
        }
        .boxed()
    }
}

impl AccessibilityConnection for FakeBus {
    fn applications(&self) -> BoxFuture<'_, Result<Vec<ObjectRef>, BusError>> {
        self.request(FakeRequest::Applications, |inner| {
            Ok(inner
                .applications
                .iter()
                .map(|index| inner.nodes[*index].object.clone())
                .collect())
        })
    }

    fn process_id<'a>(&'a self, bus_name: &'a str) -> BoxFuture<'a, Result<u32, BusError>> {
        let name = bus_name.to_owned();
        self.request(FakeRequest::ProcessId(name.clone()), move |inner| {
            inner
                .process_ids
                .get(&name)
                .copied()
                .ok_or_else(|| BusError(format!("unknown connection {name}")))
        })
    }

    fn children<'a>(
        &'a self,
        node: &'a ObjectRef,
    ) -> BoxFuture<'a, Result<Vec<ObjectRef>, BusError>> {
        self.request(FakeRequest::Children(node.clone()), {
            let node = node.clone();
            move |inner| {
                let index = inner
                    .by_object
                    .get(&node)
                    .ok_or_else(|| BusError("no such accessible".into()))?;
                Ok(inner.nodes[*index]
                    .children
                    .iter()
                    .map(|child| inner.nodes[*child].object.clone())
                    .collect())
            }
        })
    }

    fn name<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>> {
        self.node_request(FakeRequest::Name(node.clone()), node, |found| {
            property_result(found, |found| found.spec.name.clone())
        })
    }

    fn description<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<String, BusError>> {
        self.node_request(FakeRequest::Description(node.clone()), node, |found| {
            property_result(found, |found| found.spec.description.clone())
        })
    }

    fn role<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeRole, BusError>> {
        self.node_request(FakeRequest::Role(node.clone()), node, |found| {
            property_result(found, |found| NodeRole {
                name: found.spec.role.clone(),
                password: found.spec.password,
            })
        })
    }

    fn state<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<NodeState, BusError>> {
        self.node_request(FakeRequest::State(node.clone()), node, |found| {
            property_result(found, |found| NodeState {
                active: found.spec.active,
                focused: found.spec.focused,
            })
        })
    }

    fn implements_text<'a>(&'a self, node: &'a ObjectRef) -> BoxFuture<'a, Result<bool, BusError>> {
        self.node_request(FakeRequest::ImplementsText(node.clone()), node, |found| {
            property_result(found, |found| found.spec.text.is_some())
        })
    }

    fn character_count<'a>(
        &'a self,
        node: &'a ObjectRef,
    ) -> BoxFuture<'a, Result<usize, BusError>> {
        self.node_request(FakeRequest::CharacterCount(node.clone()), node, |found| {
            property_result(found, |found| {
                found
                    .spec
                    .text
                    .as_deref()
                    .map_or(0, |text| text.chars().count())
            })
        })
    }

    fn text<'a>(
        &'a self,
        node: &'a ObjectRef,
        start: usize,
        end: usize,
    ) -> BoxFuture<'a, Result<String, BusError>> {
        let request = FakeRequest::Text {
            node: node.clone(),
            start,
            end,
        };
        self.node_request(request, node, move |found| {
            property_result(found, |found| {
                found
                    .spec
                    .text
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .collect()
            })
        })
    }
}

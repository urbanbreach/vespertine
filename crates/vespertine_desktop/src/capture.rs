//! Permission-aware window capture (PLT-012).
//!
//! A [`CaptureService`] captures one other application window as a PNG image
//! on demand without ever bypassing the desktop's consent mechanisms:
//!
//! * Wayland uses only the Screenshot portal ([`portal`]), probing its
//!   available targets before every capture.
//! * X11 uses the portal when it supports the active-window target and its
//!   outcome is final; otherwise a native `GetImage` of the active window
//!   ([`x11`]).
//!
//! Every external boundary is an injectable trait: the portal
//! ([`ScreenshotPortal`]), the X server ([`X11Server`]), the filesystem
//! ([`DesktopFs`]), the clock ([`Clock`]), the application's own windows on
//! Wayland ([`OwnWindowProbe`]) and optional semantic enrichment
//! ([`ContextEnricher`], PLT-013). The buffer budgets of PLT-012 R6 are public
//! pure functions in [`budget`].

pub mod budget;
pub mod metadata;
pub mod portal;
pub mod x11;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::{
    FutureExt as _,
    future::{BoxFuture, Either, select},
};
use smol::unblock;
use thiserror::Error;

pub use budget::{
    BudgetError, DEFAULT_ENCODED_IMAGE_CAP, MAX_DIMENSION, MAX_PIXELS, MAX_RGBA_BYTES,
    png_header_dimensions, validate_dimensions,
};
pub use metadata::{DesktopFs, StdDesktopFs, TargetKind, TargetMetadata};
pub use portal::{
    MIN_SCREENSHOT_VERSION, PortalError, PortalProbe, PortalTargets, ScreenshotPortal,
    ScreenshotRequest, WindowTarget,
};
pub use x11::{WindowId, X11Error, X11Server};

use crate::shortcut::ShortcutService;

/// Default budget for optional semantic enrichment after delivery.
pub const DEFAULT_ENRICHMENT_BUDGET: Duration = Duration::from_millis(1000);
/// Default self-capture message.
pub const DEFAULT_SELF_CAPTURE_MESSAGE: &str = "Switch to another application to capture.";

const UNSUPPORTED_PORTAL_MESSAGE: &str =
    "The desktop portal does not support window-only capture and needs an update";

/// The window system the service runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopPlatform {
    Wayland,
    X11,
}

/// State of one capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Checking,
    Ready,
    /// The portal shows its own window picker.
    UserSelection,
    PermissionRequired,
    SetupRequired,
    Unavailable,
}

/// What is captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// The currently active window (non-interactive).
    ActiveWindow,
    /// A window chosen in the portal's own picker (interactive).
    PortalWindowPicker,
}

/// The capability record (R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCapability {
    pub platform: DesktopPlatform,
    pub shortcut: CapabilityState,
    pub window_capture: CapabilityState,
    /// Semantic context (PLT-013): unavailable on Wayland, ready on X11 when
    /// the accessibility service reports available.
    pub application_text: CapabilityState,
    pub target: CaptureTarget,
}

/// Window capture availability as consumed by the permission status service
/// (PLT-013): the probe result, independent of earlier denials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAvailability {
    Ready,
    UserSelection,
    Unavailable,
}

/// Outcome of a user-triggered portal capture operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcomeKind {
    Succeeded,
    Denied,
    Cancelled,
    Failed,
}

/// Receives the outcome of every portal capture operation (for example the
/// permission status service).
pub trait CaptureOutcomeSink: Send + Sync {
    fn report(&self, outcome: CaptureOutcomeKind);
}

/// A distinct capture failure (R1).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptureError {
    /// The portal or permission store refused the capture.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// The user dismissed the consent dialog or picker.
    #[error("capture cancelled")]
    Cancelled,
    /// One of the application's own windows is active.
    #[error("{0}")]
    SelfCapture(String),
    #[error("no eligible window to capture")]
    NoEligibleWindow,
    /// The shortcut is disabled, being recorded, or held by another client.
    #[error("shortcut unavailable: {0}")]
    ShortcutUnavailable(String),
    #[error("capture failed: {0}")]
    Failed(String),
}

impl From<PortalError> for CaptureError {
    fn from(error: PortalError) -> Self {
        match error {
            PortalError::Cancelled => CaptureError::Cancelled,
            PortalError::Denied(reason) => CaptureError::PermissionDenied(reason),
            other => CaptureError::Failed(other.to_string()),
        }
    }
}

/// Semantic text snapshot of a window (PLT-013 format).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSnapshot {
    pub format_version: u32,
    pub content: String,
    pub truncated: bool,
}

impl SemanticSnapshot {
    /// The empty snapshot of a screenshot-only result.
    pub fn empty() -> Self {
        Self {
            format_version: 1,
            content: String::new(),
            truncated: false,
        }
    }
}

/// A verifiable window identity handed to the enrichment hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIdentity {
    pub pid: u32,
    /// The exact window title.
    pub title: String,
}

/// Result of semantic enrichment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichedContext {
    pub application_name: Option<String>,
    pub snapshot: SemanticSnapshot,
}

/// Optional semantic enrichment (PLT-013). The capture never awaits it: the
/// returned future is cancellable by dropping it.
pub trait ContextEnricher: Send + Sync {
    /// Whether the accessibility service is available.
    fn is_available(&self) -> BoxFuture<'static, bool>;
    /// Reads the semantic context of the window with this identity.
    fn enrich(&self, identity: &WindowIdentity) -> BoxFuture<'static, Option<EnrichedContext>>;
}

/// Time source of the service.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// [`Clock`] over the system time and `smol` timers.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        async move {
            smol::Timer::at(std::time::Instant::now() + duration).await;
        }
        .boxed()
    }
}

/// Whether one of the application's own windows is the active window, for
/// Wayland where the application knows its own focus (R5).
pub trait OwnWindowProbe: Send + Sync {
    fn own_window_is_active(&self) -> bool;
}

/// The window system backend of a [`CaptureService`].
pub enum CaptureBackend {
    Wayland {
        portal: Arc<dyn ScreenshotPortal>,
        own_windows: Arc<dyn OwnWindowProbe>,
    },
    X11 {
        portal: Arc<dyn ScreenshotPortal>,
        server: Arc<dyn X11Server>,
    },
}

impl CaptureBackend {
    fn portal(&self) -> &Arc<dyn ScreenshotPortal> {
        match self {
            CaptureBackend::Wayland { portal, .. } | CaptureBackend::X11 { portal, .. } => portal,
        }
    }

    fn platform(&self) -> DesktopPlatform {
        match self {
            CaptureBackend::Wayland { .. } => DesktopPlatform::Wayland,
            CaptureBackend::X11 { .. } => DesktopPlatform::X11,
        }
    }
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Cap on the encoded portal image (default 24 MiB).
    pub encoded_image_cap: u64,
    /// Budget of the optional enrichment.
    pub enrichment_budget: Duration,
    /// Process id compared with `_NET_WM_PID` for self-capture detection.
    pub own_pid: u32,
    /// Message of the self-capture error.
    pub self_capture_message: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            encoded_image_cap: DEFAULT_ENCODED_IMAGE_CAP,
            enrichment_budget: DEFAULT_ENRICHMENT_BUDGET,
            own_pid: std::process::id(),
            self_capture_message: DEFAULT_SELF_CAPTURE_MESSAGE.to_owned(),
        }
    }
}

/// A captured window image (R7).
pub struct CapturedImage {
    /// Unique id of this capture.
    pub id: String,
    /// PNG bytes.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub captured_at: SystemTime,
    pub metadata: TargetMetadata,
    /// Empty until [`CapturedImage::apply_enrichment`] attaches a context.
    pub semantic: SemanticSnapshot,
    /// Pending optional enrichment, present only for a verifiable identity.
    /// Never awaited by the capture; drop it to cancel.
    pub enrichment: Option<PendingEnrichment>,
}

impl std::fmt::Debug for CapturedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedImage")
            .field("id", &self.id)
            .field("png_bytes", &self.png.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("metadata", &self.metadata)
            .field("has_enrichment", &self.enrichment.is_some())
            .finish()
    }
}

impl CapturedImage {
    /// Attaches a resolved semantic context to the image and updates the
    /// display name with the semantic application name.
    pub fn apply_enrichment(&mut self, context: EnrichedContext) {
        if let Some(name) = &context.application_name {
            self.metadata.apply_semantic_application_name(name);
        }
        self.semantic = context.snapshot;
    }
}

/// A window verified as the capture target (R3 step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWindow {
    pub window: WindowId,
    pub identity: WindowIdentity,
}

/// Returns the verifiable identity of `window`: a non-zero `_NET_WM_PID`, a
/// non-blank title, and it is the only entry of `_NET_CLIENT_LIST` with that
/// same process id and exact title. Blocking.
pub fn verify_window_identity(server: &dyn X11Server, window: WindowId) -> Option<VerifiedWindow> {
    let pid = server
        .window_pid(window)
        .ok()
        .flatten()
        .filter(|pid| *pid != 0)?;
    let title = x11::window_title_text(&server.window_title(window).ok()?)?;
    let mut matches = Vec::new();
    for client in server.client_list().ok()? {
        if server.window_pid(client).ok().flatten() != Some(pid) {
            continue;
        }
        let client_title = server
            .window_title(client)
            .ok()
            .and_then(|properties| x11::window_title_text(&properties));
        if client_title.as_deref() == Some(title.as_str()) {
            matches.push(client);
        }
    }
    if matches == [window] {
        Some(VerifiedWindow {
            window,
            identity: WindowIdentity { pid, title },
        })
    } else {
        None
    }
}

/// Optional semantic enrichment requested separately from the image, with
/// its own budget. Dropping it cancels the work.
pub struct PendingEnrichment {
    target: VerifiedWindow,
    enricher: Arc<dyn ContextEnricher>,
    server: Arc<dyn X11Server>,
    clock: Arc<dyn Clock>,
    budget: Duration,
}

impl std::fmt::Debug for PendingEnrichment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingEnrichment")
            .field("target", &self.target)
            .field("budget", &self.budget)
            .finish()
    }
}

impl PendingEnrichment {
    /// The identity the enrichment will query.
    pub fn identity(&self) -> &WindowIdentity {
        &self.target.identity
    }

    /// Runs the enrichment within its budget. Returns `None` on failure,
    /// expiry or absence, and also when the active window is no longer the
    /// same identity after the enrichment finished. Dropping the returned
    /// future cancels the enrichment.
    pub async fn resolve(self) -> Option<EnrichedContext> {
        let work = self.enricher.enrich(&self.target.identity);
        let timer = self.clock.sleep(self.budget);
        let context = match select(work, timer).await {
            Either::Left((context, _timer)) => context?,
            Either::Right(((), _work)) => return None,
        };
        let server = self.server.clone();
        let target = self.target;
        let still_same = unblock(move || {
            let active = server.active_window().ok().flatten()?;
            verify_window_identity(&*server, active)
        })
        .await;
        (still_same.as_ref() == Some(&target)).then_some(context)
    }

    /// Cancels the enrichment.
    pub fn cancel(self) {}
}

struct Inner {
    backend: CaptureBackend,
    clock: Arc<dyn Clock>,
    fs: Arc<dyn DesktopFs>,
    enricher: Option<Arc<dyn ContextEnricher>>,
    sink: Option<Arc<dyn CaptureOutcomeSink>>,
    shortcuts: Option<ShortcutService>,
    config: CaptureConfig,
    busy: AtomicBool,
    last_denied: AtomicBool,
}

struct BusyGuard(Arc<Inner>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The permission-aware capture service. Cloning is cheap and shares state.
#[derive(Clone)]
pub struct CaptureService {
    inner: Arc<Inner>,
}

impl CaptureService {
    /// Creates a service over `backend` with the system clock and filesystem.
    pub fn new(backend: CaptureBackend) -> Self {
        Self::with_services(backend, Arc::new(SystemClock), Arc::new(StdDesktopFs))
    }

    /// Creates a service with an injected clock and filesystem.
    pub fn with_services(
        backend: CaptureBackend,
        clock: Arc<dyn Clock>,
        fs: Arc<dyn DesktopFs>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                clock,
                fs,
                enricher: None,
                sink: None,
                shortcuts: None,
                config: CaptureConfig::default(),
                busy: AtomicBool::new(false),
                last_denied: AtomicBool::new(false),
            }),
        }
    }

    fn configure(mut self, change: impl FnOnce(&mut Inner)) -> Self {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("capture service is configured before it is cloned");
        change(inner);
        self
    }

    pub fn with_config(self, config: CaptureConfig) -> Self {
        self.configure(|inner| inner.config = config)
    }

    /// Installs the optional semantic enrichment hook.
    pub fn with_enricher(self, enricher: Arc<dyn ContextEnricher>) -> Self {
        self.configure(|inner| inner.enricher = Some(enricher))
    }

    /// Installs the receiver of portal operation outcomes.
    pub fn with_outcome_sink(self, sink: Arc<dyn CaptureOutcomeSink>) -> Self {
        self.configure(|inner| inner.sink = Some(sink))
    }

    /// Ties the service to the shortcut preferences: requests are refused
    /// while the feature is off or a recording is in progress.
    pub fn with_shortcuts(self, shortcuts: ShortcutService) -> Self {
        self.configure(|inner| inner.shortcuts = Some(shortcuts))
    }

    pub fn platform(&self) -> DesktopPlatform {
        self.inner.backend.platform()
    }

    /// Probes the window capture state and target without any side effect.
    async fn probe_window_capture(&self) -> (CapabilityState, CaptureTarget) {
        let probe = self.inner.backend.portal().probe().await;
        let from_probe = |probe: &PortalProbe| match probe.window_target() {
            Some(WindowTarget::ActiveWindow) => {
                (CapabilityState::Ready, CaptureTarget::ActiveWindow)
            }
            Some(WindowTarget::Window) => (
                CapabilityState::UserSelection,
                CaptureTarget::PortalWindowPicker,
            ),
            None => (CapabilityState::Unavailable, CaptureTarget::ActiveWindow),
        };
        match &self.inner.backend {
            CaptureBackend::Wayland { .. } => match &probe {
                Ok(probe) => from_probe(probe),
                Err(_) => (CapabilityState::Unavailable, CaptureTarget::ActiveWindow),
            },
            CaptureBackend::X11 { server, .. } => {
                if let Ok(probe) = &probe
                    && probe.supports(WindowTarget::ActiveWindow)
                {
                    return (CapabilityState::Ready, CaptureTarget::ActiveWindow);
                }
                let server = server.clone();
                if unblock(move || server.active_window().is_ok()).await {
                    (CapabilityState::Ready, CaptureTarget::ActiveWindow)
                } else {
                    (CapabilityState::Unavailable, CaptureTarget::ActiveWindow)
                }
            }
        }
    }

    /// The capability record (R1). Runs a fresh probe.
    pub async fn capability(&self) -> CaptureCapability {
        let (mut window_capture, target) = self.probe_window_capture().await;
        if self.inner.last_denied.load(Ordering::Acquire)
            && matches!(
                window_capture,
                CapabilityState::Ready | CapabilityState::UserSelection
            )
        {
            window_capture = CapabilityState::PermissionRequired;
        }
        let application_text = match (&self.inner.backend, &self.inner.enricher) {
            (CaptureBackend::X11 { .. }, Some(enricher)) => {
                if enricher.is_available().await {
                    CapabilityState::Ready
                } else {
                    CapabilityState::Unavailable
                }
            }
            _ => CapabilityState::Unavailable,
        };
        let shortcut = self.inner.shortcuts.as_ref().map_or(
            CapabilityState::Unavailable,
            ShortcutService::capability_state,
        );
        CaptureCapability {
            platform: self.platform(),
            shortcut,
            window_capture,
            application_text,
            target,
        }
    }

    /// The probe result for the permission status service, independent of
    /// earlier denials.
    pub async fn availability(&self) -> CaptureAvailability {
        match self.probe_window_capture().await.0 {
            CapabilityState::Ready => CaptureAvailability::Ready,
            CapabilityState::UserSelection => CaptureAvailability::UserSelection,
            _ => CaptureAvailability::Unavailable,
        }
    }

    /// Whether the most recent portal operation was denied (until a later
    /// success or [`CaptureService::clear_denied`]).
    pub fn last_capture_denied(&self) -> bool {
        self.inner.last_denied.load(Ordering::Acquire)
    }

    /// Forgets a remembered denial, for an explicit new user request.
    pub fn clear_denied(&self) {
        self.inner.last_denied.store(false, Ordering::Release);
    }

    fn begin(&self) -> Option<BusyGuard> {
        self.inner
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| BusyGuard(self.inner.clone()))
    }

    fn gate(&self) -> Result<(), CaptureError> {
        match self
            .inner
            .shortcuts
            .as_ref()
            .and_then(ShortcutService::refusal_reason)
        {
            Some(reason) => Err(CaptureError::ShortcutUnavailable(reason.to_owned())),
            None => Ok(()),
        }
    }

    fn self_capture_error(&self) -> CaptureError {
        CaptureError::SelfCapture(self.inner.config.self_capture_message.clone())
    }

    /// Fails with self-capture when one of the application's own windows is
    /// the active window. Contacts no portal and reads no pixels; callers can
    /// run it before a capture starts (a passive grab can blur the
    /// application's own window).
    pub async fn check_self_capture(&self) -> Result<(), CaptureError> {
        let own_active = match &self.inner.backend {
            CaptureBackend::Wayland { own_windows, .. } => own_windows.own_window_is_active(),
            CaptureBackend::X11 { server, .. } => {
                let server = server.clone();
                let own_pid = self.inner.config.own_pid;
                unblock(move || active_window_is_own(&*server, own_pid))
                    .await
                    .map_err(|error| CaptureError::Failed(error.to_string()))?
            }
        };
        if own_active {
            Err(self.self_capture_error())
        } else {
            Ok(())
        }
    }

    /// Captures one window with an explicit target. Refused while the
    /// shortcut feature is off or recording, and while another capture is
    /// running.
    pub async fn capture(&self, requested: CaptureTarget) -> Result<CapturedImage, CaptureError> {
        self.gate()?;
        let Some(_guard) = self.begin() else {
            return Err(CaptureError::Failed(
                "a capture is already in progress".into(),
            ));
        };
        self.capture_guarded(requested).await
    }

    /// Captures on a shortcut activation using the freshly probed target. A
    /// second activation while a capture is running is ignored (`None`).
    pub async fn capture_on_activation(&self) -> Option<Result<CapturedImage, CaptureError>> {
        let _guard = self.begin()?;
        if let Err(error) = self.gate() {
            return Some(Err(error));
        }
        let (_, target) = self.probe_window_capture().await;
        Some(self.capture_guarded(target).await)
    }

    async fn capture_guarded(
        &self,
        requested: CaptureTarget,
    ) -> Result<CapturedImage, CaptureError> {
        let mut self_checked = false;
        if requested == CaptureTarget::ActiveWindow {
            self.check_self_capture().await?;
            self_checked = true;
        }
        let fresh = self.inner.backend.portal().probe().await;
        if let CaptureBackend::X11 { .. } = &self.inner.backend {
            let portal_has_active_window =
                matches!(&fresh, Ok(probe) if probe.supports(WindowTarget::ActiveWindow));
            if !portal_has_active_window && requested == CaptureTarget::ActiveWindow {
                return self.native_capture().await;
            }
        }
        self.portal_capture(fresh, self_checked).await
    }

    fn report(&self, outcome: CaptureOutcomeKind) {
        match outcome {
            CaptureOutcomeKind::Denied => self.inner.last_denied.store(true, Ordering::Release),
            CaptureOutcomeKind::Succeeded => self.inner.last_denied.store(false, Ordering::Release),
            _ => {}
        }
        if let Some(sink) = &self.inner.sink {
            sink.report(outcome);
        }
    }

    fn unsupported_portal(&self, detail: Option<&PortalError>) -> CaptureError {
        match detail {
            Some(PortalError::Unreachable(reason)) => CaptureError::Failed(format!(
                "{UNSUPPORTED_PORTAL_MESSAGE}: the Screenshot portal could not be reached ({reason})"
            )),
            _ => CaptureError::Failed(format!("{UNSUPPORTED_PORTAL_MESSAGE}.")),
        }
    }

    /// The Wayland capture path (R2); also the X11 path when the portal
    /// serves the active-window target (R3 step 2).
    async fn portal_capture(
        &self,
        fresh: Result<PortalProbe, PortalError>,
        self_checked: bool,
    ) -> Result<CapturedImage, CaptureError> {
        let portal = self.inner.backend.portal();
        let probe = fresh.map_err(|error| self.unsupported_portal(Some(&error)))?;
        let target = probe
            .window_target()
            .ok_or_else(|| self.unsupported_portal(None))?;
        if target == WindowTarget::ActiveWindow && !self_checked {
            self.check_self_capture().await?;
        }

        // Verify again right before sending; a full-screen or default-target
        // request is never a substitute.
        let recheck = portal
            .probe()
            .await
            .map_err(|error| self.unsupported_portal(Some(&error)))?;
        if !recheck.supports(target) {
            return Err(self.unsupported_portal(None));
        }

        let response = portal.screenshot(ScreenshotRequest::new(target)).await;
        self.report(match &response {
            Ok(_) => CaptureOutcomeKind::Succeeded,
            Err(PortalError::Cancelled) => CaptureOutcomeKind::Cancelled,
            Err(PortalError::Denied(_)) => CaptureOutcomeKind::Denied,
            Err(_) => CaptureOutcomeKind::Failed,
        });
        let uri = response?;

        let (png, width, height) = self.read_portal_image(&uri).await?;
        let kind = match target {
            WindowTarget::ActiveWindow => TargetKind::PortalActiveWindow,
            WindowTarget::Window => TargetKind::PortalSelection,
        };
        Ok(self.deliver(png, width, height, TargetMetadata::portal(kind), None))
    }

    async fn read_portal_image(&self, uri: &str) -> Result<(Vec<u8>, u32, u32), CaptureError> {
        let url = url::Url::parse(uri).map_err(|_| {
            CaptureError::Failed(format!("the portal returned a non-file URI: {uri}"))
        })?;
        let local = url.scheme() == "file"
            && url
                .host_str()
                .is_none_or(|host| host.is_empty() || host == "localhost");
        let path: PathBuf = local
            .then(|| url.to_file_path().ok())
            .flatten()
            .ok_or_else(|| {
                CaptureError::Failed(format!("the portal returned a non-file URI: {uri}"))
            })?;
        let fs = self.inner.fs.clone();
        let cap = self.inner.config.encoded_image_cap;
        unblock(move || read_bounded_image(&*fs, &path, cap)).await
    }

    /// The native X11 capture path (R3 step 3).
    async fn native_capture(&self) -> Result<CapturedImage, CaptureError> {
        let CaptureBackend::X11 { server, .. } = &self.inner.backend else {
            return Err(CaptureError::Failed(
                "native capture is only available on X11".into(),
            ));
        };
        let failed = |error: X11Error| CaptureError::Failed(error.to_string());

        let active = {
            let server = server.clone();
            unblock(move || server.active_window())
                .await
                .map_err(failed)?
        }
        .ok_or(CaptureError::NoEligibleWindow)?;
        let geometry = {
            let server = server.clone();
            match unblock(move || server.window_geometry(active)).await {
                Ok(geometry) => geometry,
                // The active window id is stale: nothing eligible to capture.
                Err(error) if error.bad_window => return Err(CaptureError::NoEligibleWindow),
                Err(error) => return Err(failed(error)),
            }
        };
        if geometry.width == 0 || geometry.height == 0 {
            return Err(CaptureError::NoEligibleWindow);
        }
        validate_dimensions(u64::from(geometry.width), u64::from(geometry.height))
            .map_err(|error| CaptureError::Failed(error.to_string()))?;

        let (metadata, verified) = {
            let server = server.clone();
            let fs = self.inner.fs.clone();
            unblock(move || {
                (
                    metadata::x11_target_metadata(&*server, &*fs, active),
                    verify_window_identity(&*server, active),
                )
            })
            .await
        };

        let raw = {
            let server = server.clone();
            match unblock(move || server.get_image(active, geometry.width, geometry.height)).await {
                Ok(raw) => raw,
                Err(error) if error.bad_window => {
                    return Err(CaptureError::Failed(
                        "The active window changed during capture; try again.".into(),
                    ));
                }
                Err(error) => return Err(failed(error)),
            }
        };
        let still_active = {
            let server = server.clone();
            unblock(move || server.active_window())
                .await
                .map_err(failed)?
        };
        if still_active != Some(active) {
            return Err(CaptureError::Failed(
                "The active window changed during capture; try again.".into(),
            ));
        }
        let png = unblock(move || {
            let rgba = x11::raw_image_to_rgba(&raw)?;
            budget::encode_png_rgba(raw.width, raw.height, &rgba).map_err(|error| error.to_string())
        })
        .await
        .map_err(CaptureError::Failed)?;

        let pending = match (&self.inner.enricher, verified) {
            (Some(enricher), Some(target)) => Some(PendingEnrichment {
                target,
                enricher: enricher.clone(),
                server: server.clone(),
                clock: self.inner.clock.clone(),
                budget: self.inner.config.enrichment_budget,
            }),
            _ => None,
        };
        Ok(self.deliver(png, geometry.width, geometry.height, metadata, pending))
    }

    fn deliver(
        &self,
        png: Vec<u8>,
        width: u32,
        height: u32,
        metadata: TargetMetadata,
        enrichment: Option<PendingEnrichment>,
    ) -> CapturedImage {
        let captured_at = self.inner.clock.now();
        let nanos = captured_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        CapturedImage {
            id: format!("capture-{}-{nanos:x}-{sequence}", self.inner.config.own_pid),
            png,
            width,
            height,
            captured_at,
            metadata,
            semantic: SemanticSnapshot::empty(),
            enrichment,
        }
    }
}

/// Whether the active window belongs to this process (`_NET_WM_PID` equals
/// `own_pid`). Blocking.
pub fn active_window_is_own(server: &dyn X11Server, own_pid: u32) -> Result<bool, X11Error> {
    let Some(active) = server.active_window()? else {
        return Ok(false);
    };
    match server.window_pid(active) {
        Ok(pid) => Ok(matches!(pid, Some(pid) if pid != 0 && pid == own_pid)),
        // A stale active window id cannot be one of the application's windows.
        Err(error) if error.bad_window => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reads a portal image file: the length is checked against `cap` before
/// opening, the read is bounded to `cap + 1` bytes so growth during reading
/// is detected, and the PNG is validated from its header and decoded under
/// the buffer budgets.
fn read_bounded_image(
    fs: &dyn DesktopFs,
    path: &std::path::Path,
    cap: u64,
) -> Result<(Vec<u8>, u32, u32), CaptureError> {
    let failed = |message: String| CaptureError::Failed(message);
    let length = fs
        .file_len(path)
        .map_err(|error| failed(format!("cannot stat the portal image: {error}")))?;
    if length > cap {
        return Err(failed(format!(
            "the portal image of {length} bytes exceeds the {cap} byte cap"
        )));
    }
    let data = fs
        .read_bounded(path, cap.saturating_add(1))
        .map_err(|error| failed(format!("cannot read the portal image: {error}")))?;
    if data.len() as u64 > cap {
        return Err(failed(format!(
            "the portal image grew past the {cap} byte cap while it was read"
        )));
    }
    let decoded = budget::decode_png_rgba(&data).map_err(|error| failed(error.to_string()))?;
    Ok((data, decoded.width, decoded.height))
}

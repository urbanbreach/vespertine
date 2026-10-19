//! Device and desktop permission status (PLT-013 R5, R6).
//!
//! [`PermissionService`] reports, separately from any request, whether desktop
//! capture, accessibility and microphone access are available on this Linux
//! desktop. A status query ([`PermissionService::status`]) never shows a
//! prompt and never writes anything; user-driven request hooks
//! ([`PermissionService::request`]) exist per domain and are never called
//! implicitly. Non-Linux permission mechanisms are neither assumed nor
//! emulated.
//!
//! What each domain probes on Linux:
//!
//! - **Desktop capture**: the capture capability supplied through
//!   [`DesktopCaptureStatusSource`] (PLT-012 provides it; [`linux`] also offers
//!   [`PortalCaptureStatusSource`](linux::PortalCaptureStatusSource), which
//!   reads the Screenshot portal's version and available targets). Denial is
//!   remembered only from user-triggered operations reported through
//!   [`PermissionService::report_capture_outcome`].
//! - **Accessibility**: `org.a11y.Bus.GetAddress` on the session bus, the
//!   `org.a11y.Status.IsEnabled` property and whether the registry root on the
//!   accessibility bus answers.
//! - **Microphone**: whether a PulseAudio-compatible sound server (PulseAudio or
//!   PipeWire's pulse server) answers on its native unix socket and what capture
//!   (non-monitor) sources, default source and port availability it reports,
//!   using a small built-in client of the PulseAudio native protocol (see
//!   [`pulse`]); and, only inside a sandbox, the Device portal's answer to an
//!   explicit request.
//!
//! Hot-plug and portal changes surface through [`PermissionService::subscribe`]
//! whenever a re-query ([`refresh_all`](PermissionService::refresh_all),
//! the polling [`watch`](PermissionService::watch), or any
//! [`status`](PermissionService::status) call) yields a different status.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux;
pub mod pulse;
#[cfg(test)]
mod tests;

use crate::context::ClockSource;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
    future::BoxFuture,
};
use parking_lot::Mutex;
use std::{convert::Infallible, sync::Arc, time::Duration};

/// A permission domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionDomain {
    /// Capturing another application's window.
    DesktopCapture,
    /// Reading other applications' accessibility trees.
    Accessibility,
    /// Capturing audio from a microphone.
    Microphone,
}

impl PermissionDomain {
    /// Every domain.
    pub const ALL: [PermissionDomain; 3] = [
        PermissionDomain::DesktopCapture,
        PermissionDomain::Accessibility,
        PermissionDomain::Microphone,
    ];

    fn index(self) -> usize {
        match self {
            PermissionDomain::DesktopCapture => 0,
            PermissionDomain::Accessibility => 1,
            PermissionDomain::Microphone => 2,
        }
    }
}

/// The status of one domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionStatus {
    /// A probe is running (or none has completed yet).
    Checking,
    /// Usable.
    Available,
    /// The user or the desktop's permission store refused.
    Denied,
    /// Not present on this desktop.
    Unavailable,
    /// Present but needs user setup first.
    SetupRequired,
}

/// Why a backend operation did not succeed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PermissionError {
    /// Explicitly refused (by the user, the portal or the permission store).
    #[error("denied: {0}")]
    Denied(String),
    /// The user dismissed the request without deciding.
    #[error("cancelled by the user")]
    Cancelled,
    /// The operation failed for another reason.
    #[error("failed: {0}")]
    Failed(String),
}

/// Result of the capture capability probe (PLT-012), reduced to what the
/// status needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureAvailability {
    /// Capture capability is ready.
    Ready,
    /// The portal shows its own window picker.
    UserSelection,
    /// The Screenshot portal is absent, older than version 3, or lacks window
    /// targets.
    Unavailable,
}

/// The outcome of a user-triggered portal capture operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureOperationOutcome {
    /// Pixels were delivered.
    Succeeded,
    /// The portal or permission store denied the operation.
    Denied,
    /// The user cancelled (for example the picker).
    Cancelled,
    /// Any other failure.
    Failed,
}

/// What the sound server reports about capture devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicrophoneDevices {
    /// No sound server answers.
    NoSoundServer,
    /// The server has no capture (non-monitor) source.
    NoCaptureSource,
    /// Capture sources exist but none is selected as default or all are
    /// disabled.
    NoDefaultSource,
    /// A capture source is selected as default and enabled.
    Ready,
}

/// How a user-driven request ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestOutcome {
    /// The request ran to completion (the resulting status says what it
    /// achieved). Also used when there is nothing to prompt for.
    Completed,
    /// The user dismissed the request.
    UserCancelled,
    /// The request was explicitly refused.
    Denied,
    /// The request failed.
    Failed(String),
}

/// The result of a request hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequestResult {
    /// The domain's status after the request.
    pub status: PermissionStatus,
    /// How the request ended.
    pub outcome: RequestOutcome,
}

/// A status change notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionChange {
    /// The domain whose status changed.
    pub domain: PermissionDomain,
    /// Its new status.
    pub status: PermissionStatus,
}

/// Provides the capture capability (PLT-012) to the status service.
pub trait DesktopCaptureStatusSource: Send + Sync + 'static {
    /// Runs the capability probe. Must not show a prompt.
    fn availability(&self) -> BoxFuture<'_, Result<CaptureAvailability, PermissionError>>;
}

/// Accessibility bus probes and the one permitted write.
pub trait AccessibilityStatusBackend: Send + Sync + 'static {
    /// Obtains the accessibility bus address from the session bus and connects.
    /// An error means there is no accessibility bus.
    fn bus_reachable(&self) -> BoxFuture<'_, Result<(), PermissionError>>;
    /// Whether the registry root on the accessibility bus answers.
    fn registry_answers(&self) -> BoxFuture<'_, bool>;
    /// Reads `org.a11y.Status.IsEnabled`.
    fn is_enabled(&self) -> BoxFuture<'_, Result<bool, PermissionError>>;
    /// Writes `org.a11y.Status.IsEnabled`. Only called by the explicit
    /// accessibility request hook.
    fn set_enabled(&self, enabled: bool) -> BoxFuture<'_, Result<(), PermissionError>>;
}

/// Microphone device probe and the Device portal request.
pub trait MicrophoneBackend: Send + Sync + 'static {
    /// Whether the process runs inside a sandbox with a Device portal.
    fn sandboxed(&self) -> bool;
    /// Probes sound server availability. Must not show a prompt.
    fn devices(&self) -> BoxFuture<'_, Result<MicrophoneDevices, PermissionError>>;
    /// Issues the Device portal request for `microphone`. Only called by the
    /// explicit request hook and only when [`sandboxed`](Self::sandboxed).
    fn request_device_access(&self) -> BoxFuture<'_, Result<(), PermissionError>>;
}

#[derive(Default)]
struct DomainState {
    probing: usize,
    last: Option<PermissionStatus>,
}

#[derive(Default)]
struct State {
    domains: [DomainState; 3],
    capture_last_outcome: Option<CaptureOperationOutcome>,
    capture_availability: Option<CaptureAvailability>,
    microphone_denied: bool,
    subscribers: Vec<UnboundedSender<PermissionChange>>,
}

/// The permission status service.
pub struct PermissionService {
    capture: Arc<dyn DesktopCaptureStatusSource>,
    accessibility: Arc<dyn AccessibilityStatusBackend>,
    microphone: Arc<dyn MicrophoneBackend>,
    state: Mutex<State>,
}

struct ProbeGuard<'a> {
    service: &'a PermissionService,
    domain: PermissionDomain,
}

impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        self.service.state.lock().domains[self.domain.index()].probing -= 1;
    }
}

fn capture_status(
    availability: Result<CaptureAvailability, PermissionError>,
    denied: bool,
) -> PermissionStatus {
    match availability {
        Ok(CaptureAvailability::Ready | CaptureAvailability::UserSelection) => {
            if denied {
                PermissionStatus::Denied
            } else {
                PermissionStatus::Available
            }
        }
        Ok(CaptureAvailability::Unavailable) | Err(_) => PermissionStatus::Unavailable,
    }
}

/// Precedence: unavailable devices, then an explicit refusal, then setup,
/// then available.
fn microphone_status(
    devices: Result<MicrophoneDevices, PermissionError>,
    denied: bool,
) -> PermissionStatus {
    match devices {
        Ok(MicrophoneDevices::NoSoundServer | MicrophoneDevices::NoCaptureSource) | Err(_) => {
            PermissionStatus::Unavailable
        }
        Ok(_) if denied => PermissionStatus::Denied,
        Ok(MicrophoneDevices::NoDefaultSource) => PermissionStatus::SetupRequired,
        Ok(MicrophoneDevices::Ready) => PermissionStatus::Available,
    }
}

impl PermissionService {
    /// A service over the given backends.
    pub fn new(
        capture: Arc<dyn DesktopCaptureStatusSource>,
        accessibility: Arc<dyn AccessibilityStatusBackend>,
        microphone: Arc<dyn MicrophoneBackend>,
    ) -> Self {
        Self {
            capture,
            accessibility,
            microphone,
            state: Mutex::new(State::default()),
        }
    }

    /// A service with the real Linux accessibility and microphone backends and
    /// the given capture capability source.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn linux(capture: Arc<dyn DesktopCaptureStatusSource>) -> Self {
        Self::new(
            capture,
            Arc::new(linux::AtspiStatusBackend),
            Arc::new(linux::SoundServerMicrophone::default()),
        )
    }

    /// The last known status without probing: [`PermissionStatus::Checking`]
    /// while a probe of that domain is running or before any probe finished.
    pub fn cached_status(&self, domain: PermissionDomain) -> PermissionStatus {
        let state = self.state.lock();
        let domain = &state.domains[domain.index()];
        if domain.probing > 0 {
            return PermissionStatus::Checking;
        }
        domain.last.unwrap_or(PermissionStatus::Checking)
    }

    /// Probes a domain. Has no side effects: it never prompts and never writes
    /// a property.
    pub async fn status(&self, domain: PermissionDomain) -> PermissionStatus {
        let _probe = self.begin(domain);
        let status = self.probe(domain).await;
        self.publish(domain, status);
        status
    }

    /// Probes every domain, in order.
    pub async fn refresh_all(&self) {
        for domain in PermissionDomain::ALL {
            self.status(domain).await;
        }
    }

    /// Refreshes every domain every `interval` on `clock` until dropped, to
    /// notice hot-plugged devices and portal-side changes.
    pub async fn watch(&self, interval: Duration, clock: &dyn ClockSource) -> Infallible {
        loop {
            self.refresh_all().await;
            clock.sleep(interval).await;
        }
    }

    /// Notifications of status changes: one [`PermissionChange`] each time a
    /// completed probe differs from the previous completed probe of that
    /// domain.
    pub fn subscribe(&self) -> UnboundedReceiver<PermissionChange> {
        let (sender, receiver) = unbounded();
        self.state.lock().subscribers.push(sender);
        receiver
    }

    /// Records the outcome of a user-triggered portal capture operation. The
    /// capture status is [`Denied`](PermissionStatus::Denied) exactly while the
    /// most recent such operation was denied (a later success, cancellation or
    /// failure, or an explicit [`request`](Self::request), ends it).
    pub fn report_capture_outcome(&self, outcome: CaptureOperationOutcome) {
        let (availability, denied) = {
            let mut state = self.state.lock();
            state.capture_last_outcome = Some(outcome);
            (
                state.capture_availability,
                outcome == CaptureOperationOutcome::Denied,
            )
        };
        if let Some(availability) = availability {
            self.publish(
                PermissionDomain::DesktopCapture,
                capture_status(Ok(availability), denied),
            );
        }
    }

    /// The explicit, user-driven request hook of a domain.
    ///
    /// - Desktop capture re-runs the capability probe (the portal's own
    ///   consent prompt appears at capture time, never here).
    /// - Accessibility writes `IsEnabled = true` and re-queries.
    /// - Microphone issues the Device portal request only when sandboxed;
    ///   otherwise it returns the current status without a prompt.
    pub async fn request(&self, domain: PermissionDomain) -> PermissionRequestResult {
        let _probe = self.begin(domain);
        let (outcome, status) = match domain {
            PermissionDomain::DesktopCapture => {
                let availability = self.capture.availability().await;
                let outcome = match &availability {
                    Ok(_) => RequestOutcome::Completed,
                    Err(error) => RequestOutcome::Failed(error.to_string()),
                };
                let mut state = self.state.lock();
                // An explicit new request ends a remembered denial.
                state.capture_last_outcome = None;
                state.capture_availability = availability.clone().ok();
                (outcome, capture_status(availability, false))
            }
            PermissionDomain::Accessibility => {
                let outcome = match self.accessibility.set_enabled(true).await {
                    Ok(()) => RequestOutcome::Completed,
                    Err(error) => outcome_of(error),
                };
                (outcome, self.probe(domain).await)
            }
            PermissionDomain::Microphone => {
                let outcome = if self.microphone.sandboxed() {
                    let result = self.microphone.request_device_access().await;
                    self.state.lock().microphone_denied =
                        matches!(result, Err(PermissionError::Denied(_)));
                    match result {
                        Ok(()) => RequestOutcome::Completed,
                        Err(error) => outcome_of(error),
                    }
                } else {
                    RequestOutcome::Completed
                };
                (outcome, self.probe(domain).await)
            }
        };
        self.publish(domain, status);
        PermissionRequestResult { status, outcome }
    }

    fn begin(&self, domain: PermissionDomain) -> ProbeGuard<'_> {
        self.state.lock().domains[domain.index()].probing += 1;
        ProbeGuard {
            service: self,
            domain,
        }
    }

    async fn probe(&self, domain: PermissionDomain) -> PermissionStatus {
        match domain {
            PermissionDomain::DesktopCapture => {
                let availability = self.capture.availability().await;
                let denied = {
                    let mut state = self.state.lock();
                    state.capture_availability = availability.clone().ok();
                    state.capture_last_outcome == Some(CaptureOperationOutcome::Denied)
                };
                capture_status(availability, denied)
            }
            PermissionDomain::Accessibility => {
                if self.accessibility.bus_reachable().await.is_err() {
                    return PermissionStatus::Unavailable;
                }
                if let Ok(false) = self.accessibility.is_enabled().await {
                    return PermissionStatus::SetupRequired;
                }
                if self.accessibility.registry_answers().await {
                    PermissionStatus::Available
                } else {
                    PermissionStatus::Unavailable
                }
            }
            PermissionDomain::Microphone => {
                let devices = self.microphone.devices().await;
                let denied = self.state.lock().microphone_denied;
                microphone_status(devices, denied)
            }
        }
    }

    fn publish(&self, domain: PermissionDomain, status: PermissionStatus) {
        let mut state = self.state.lock();
        if state.domains[domain.index()].last == Some(status) {
            return;
        }
        state.domains[domain.index()].last = Some(status);
        let change = PermissionChange { domain, status };
        state
            .subscribers
            .retain(|subscriber| subscriber.unbounded_send(change).is_ok());
    }
}

fn outcome_of(error: PermissionError) -> RequestOutcome {
    match error {
        PermissionError::Denied(_) => RequestOutcome::Denied,
        PermissionError::Cancelled => RequestOutcome::UserCancelled,
        PermissionError::Failed(message) => RequestOutcome::Failed(message),
    }
}

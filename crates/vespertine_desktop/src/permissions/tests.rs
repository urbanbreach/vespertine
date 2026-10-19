//! PLT-013 permission status tests (A5), over fake services.

use super::*;
use crate::context::fake::ManualClock;
use futures::{
    FutureExt, StreamExt,
    channel::oneshot,
    executor::block_on,
    future::{Either, select},
};
use std::{
    pin::pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct FakeCapture {
    result: Mutex<Result<CaptureAvailability, PermissionError>>,
    calls: AtomicUsize,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
}

impl FakeCapture {
    fn new(result: Result<CaptureAvailability, PermissionError>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(result),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
}

impl DesktopCaptureStatusSource for FakeCapture {
    fn availability(&self) -> BoxFuture<'_, Result<CaptureAvailability, PermissionError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().take();
        async move {
            if let Some(gate) = gate {
                gate.await.ok();
            }
            self.result.lock().clone()
        }
        .boxed()
    }
}

struct FakeAtspi {
    bus: Mutex<Result<(), PermissionError>>,
    registry_answers: AtomicBool,
    enabled: Mutex<Result<bool, PermissionError>>,
    set_result: Mutex<Result<(), PermissionError>>,
    set_calls: Mutex<Vec<bool>>,
}

impl FakeAtspi {
    fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            bus: Mutex::new(Ok(())),
            registry_answers: AtomicBool::new(true),
            enabled: Mutex::new(Ok(enabled)),
            set_result: Mutex::new(Ok(())),
            set_calls: Mutex::new(Vec::new()),
        })
    }
}

impl AccessibilityStatusBackend for FakeAtspi {
    fn bus_reachable(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
        async { self.bus.lock().clone() }.boxed()
    }

    fn registry_answers(&self) -> BoxFuture<'_, bool> {
        async { self.registry_answers.load(Ordering::SeqCst) }.boxed()
    }

    fn is_enabled(&self) -> BoxFuture<'_, Result<bool, PermissionError>> {
        async { self.enabled.lock().clone() }.boxed()
    }

    fn set_enabled(&self, enabled: bool) -> BoxFuture<'_, Result<(), PermissionError>> {
        async move {
            self.set_calls.lock().push(enabled);
            let result = self.set_result.lock().clone();
            if result.is_ok() {
                *self.enabled.lock() = Ok(enabled);
            }
            result
        }
        .boxed()
    }
}

struct FakeMicrophone {
    sandboxed: bool,
    devices: Mutex<Result<MicrophoneDevices, PermissionError>>,
    portal_result: Mutex<Result<(), PermissionError>>,
    portal_calls: AtomicUsize,
}

impl FakeMicrophone {
    fn new(sandboxed: bool, devices: MicrophoneDevices) -> Arc<Self> {
        Arc::new(Self {
            sandboxed,
            devices: Mutex::new(Ok(devices)),
            portal_result: Mutex::new(Ok(())),
            portal_calls: AtomicUsize::new(0),
        })
    }
}

impl MicrophoneBackend for FakeMicrophone {
    fn sandboxed(&self) -> bool {
        self.sandboxed
    }

    fn devices(&self) -> BoxFuture<'_, Result<MicrophoneDevices, PermissionError>> {
        async { self.devices.lock().clone() }.boxed()
    }

    fn request_device_access(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
        async {
            self.portal_calls.fetch_add(1, Ordering::SeqCst);
            self.portal_result.lock().clone()
        }
        .boxed()
    }
}

struct Fixture {
    capture: Arc<FakeCapture>,
    atspi: Arc<FakeAtspi>,
    microphone: Arc<FakeMicrophone>,
    service: PermissionService,
}

fn fixture(sandboxed: bool) -> Fixture {
    let capture = FakeCapture::new(Ok(CaptureAvailability::Ready));
    let atspi = FakeAtspi::new(true);
    let microphone = FakeMicrophone::new(sandboxed, MicrophoneDevices::Ready);
    let service = PermissionService::new(capture.clone(), atspi.clone(), microphone.clone());
    Fixture {
        capture,
        atspi,
        microphone,
        service,
    }
}

fn status(fx: &Fixture, domain: PermissionDomain) -> PermissionStatus {
    block_on(fx.service.status(domain))
}

fn request(fx: &Fixture, domain: PermissionDomain) -> PermissionRequestResult {
    block_on(fx.service.request(domain))
}

#[test]
fn desktop_capture_status_follows_the_capture_capability() {
    let fx = fixture(false);
    let capture = PermissionDomain::DesktopCapture;
    assert_eq!(status(&fx, capture), PermissionStatus::Available);
    *fx.capture.result.lock() = Ok(CaptureAvailability::UserSelection);
    assert_eq!(status(&fx, capture), PermissionStatus::Available);
    *fx.capture.result.lock() = Ok(CaptureAvailability::Unavailable);
    assert_eq!(status(&fx, capture), PermissionStatus::Unavailable);
    *fx.capture.result.lock() = Err(PermissionError::Failed("no portal".into()));
    assert_eq!(status(&fx, capture), PermissionStatus::Unavailable);
}

#[test]
fn desktop_capture_is_denied_only_after_a_denied_operation() {
    let fx = fixture(false);
    let capture = PermissionDomain::DesktopCapture;
    assert_eq!(status(&fx, capture), PermissionStatus::Available);

    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    assert_eq!(status(&fx, capture), PermissionStatus::Denied);
    assert_eq!(status(&fx, capture), PermissionStatus::Denied);

    // A later success ends the denial.
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Succeeded);
    assert_eq!(status(&fx, capture), PermissionStatus::Available);

    // An explicit new request ends it too, and re-runs the probe.
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    assert_eq!(status(&fx, capture), PermissionStatus::Denied);
    let calls = fx.capture.calls.load(Ordering::SeqCst);
    let result = request(&fx, capture);
    assert_eq!(fx.capture.calls.load(Ordering::SeqCst), calls + 1);
    assert_eq!(result.status, PermissionStatus::Available);
    assert_eq!(result.outcome, RequestOutcome::Completed);

    // Cancelled and failed operations are not denials.
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Cancelled);
    assert_eq!(status(&fx, capture), PermissionStatus::Available);
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Failed);
    assert_eq!(status(&fx, capture), PermissionStatus::Available);

    // An absent portal is unavailable, never denied.
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    *fx.capture.result.lock() = Ok(CaptureAvailability::Unavailable);
    assert_eq!(status(&fx, capture), PermissionStatus::Unavailable);
}

#[test]
fn desktop_capture_request_failure_is_reported_as_failed() {
    let fx = fixture(false);
    *fx.capture.result.lock() = Err(PermissionError::Failed("portal error".into()));
    let result = request(&fx, PermissionDomain::DesktopCapture);
    assert_eq!(result.status, PermissionStatus::Unavailable);
    assert!(matches!(result.outcome, RequestOutcome::Failed(_)));
}

#[test]
fn accessibility_status_mapping() {
    let fx = fixture(false);
    let domain = PermissionDomain::Accessibility;
    assert_eq!(status(&fx, domain), PermissionStatus::Available);

    *fx.atspi.enabled.lock() = Ok(false);
    assert_eq!(status(&fx, domain), PermissionStatus::SetupRequired);
    // Setup-required needs the bus to exist.
    *fx.atspi.bus.lock() = Err(PermissionError::Failed("no bus".into()));
    assert_eq!(status(&fx, domain), PermissionStatus::Unavailable);
    *fx.atspi.bus.lock() = Ok(());

    *fx.atspi.enabled.lock() = Ok(true);
    fx.atspi.registry_answers.store(false, Ordering::SeqCst);
    assert_eq!(status(&fx, domain), PermissionStatus::Unavailable);

    // Denied never occurs, whatever the backends report.
    fx.atspi.registry_answers.store(true, Ordering::SeqCst);
    *fx.atspi.enabled.lock() = Err(PermissionError::Denied("no".into()));
    assert_eq!(status(&fx, domain), PermissionStatus::Available);
    *fx.atspi.bus.lock() = Err(PermissionError::Denied("no".into()));
    assert_eq!(status(&fx, domain), PermissionStatus::Unavailable);
}

#[test]
fn microphone_status_mapping() {
    let fx = fixture(false);
    let domain = PermissionDomain::Microphone;
    for (devices, expected) in [
        (
            MicrophoneDevices::NoSoundServer,
            PermissionStatus::Unavailable,
        ),
        (
            MicrophoneDevices::NoCaptureSource,
            PermissionStatus::Unavailable,
        ),
        (
            MicrophoneDevices::NoDefaultSource,
            PermissionStatus::SetupRequired,
        ),
        (MicrophoneDevices::Ready, PermissionStatus::Available),
    ] {
        *fx.microphone.devices.lock() = Ok(devices);
        assert_eq!(status(&fx, domain), expected, "{devices:?}");
    }
    *fx.microphone.devices.lock() = Err(PermissionError::Failed("socket".into()));
    assert_eq!(status(&fx, domain), PermissionStatus::Unavailable);
}

#[test]
fn microphone_is_denied_only_after_an_explicit_device_portal_refusal() {
    let fx = fixture(true);
    let domain = PermissionDomain::Microphone;
    assert_eq!(status(&fx, domain), PermissionStatus::Available);
    *fx.microphone.portal_result.lock() = Err(PermissionError::Denied("refused".into()));
    let result = request(&fx, domain);
    assert_eq!(result.outcome, RequestOutcome::Denied);
    assert_eq!(result.status, PermissionStatus::Denied);
    assert_eq!(status(&fx, domain), PermissionStatus::Denied);
    // Without devices the microphone stays unavailable rather than denied.
    *fx.microphone.devices.lock() = Ok(MicrophoneDevices::NoCaptureSource);
    assert_eq!(status(&fx, domain), PermissionStatus::Unavailable);
    *fx.microphone.devices.lock() = Ok(MicrophoneDevices::Ready);

    // A later granted request ends the denial.
    *fx.microphone.portal_result.lock() = Ok(());
    let result = request(&fx, domain);
    assert_eq!(result.outcome, RequestOutcome::Completed);
    assert_eq!(result.status, PermissionStatus::Available);
}

#[test]
fn querying_has_no_side_effects() {
    let fx = fixture(true);
    *fx.atspi.enabled.lock() = Ok(false);
    for _ in 0..2 {
        fx.service_refresh();
    }
    assert!(fx.atspi.set_calls.lock().is_empty(), "no property write");
    assert_eq!(
        fx.microphone.portal_calls.load(Ordering::SeqCst),
        0,
        "no portal prompt"
    );
    assert_eq!(
        fx.service.cached_status(PermissionDomain::Accessibility),
        PermissionStatus::SetupRequired
    );
}

impl Fixture {
    fn service_refresh(&self) {
        block_on(self.service.refresh_all());
    }
}

#[test]
fn accessibility_request_writes_is_enabled_and_requeries() {
    let fx = fixture(false);
    let domain = PermissionDomain::Accessibility;
    *fx.atspi.enabled.lock() = Ok(false);
    assert_eq!(status(&fx, domain), PermissionStatus::SetupRequired);
    assert!(fx.atspi.set_calls.lock().is_empty());

    let result = request(&fx, domain);
    assert_eq!(*fx.atspi.set_calls.lock(), vec![true]);
    assert_eq!(result.outcome, RequestOutcome::Completed);
    assert_eq!(result.status, PermissionStatus::Available);
}

#[test]
fn request_outcomes_distinguish_denied_cancelled_and_failed() {
    let fx = fixture(true);
    *fx.atspi.enabled.lock() = Ok(false);
    for (error, expected) in [
        (PermissionError::Denied("no".into()), RequestOutcome::Denied),
        (PermissionError::Cancelled, RequestOutcome::UserCancelled),
        (
            PermissionError::Failed("boom".into()),
            RequestOutcome::Failed("boom".into()),
        ),
    ] {
        *fx.atspi.set_result.lock() = Err(error.clone());
        let result = request(&fx, PermissionDomain::Accessibility);
        assert_eq!(result.outcome, expected);
        assert_eq!(result.status, PermissionStatus::SetupRequired);

        *fx.microphone.portal_result.lock() = Err(error);
        let result = request(&fx, PermissionDomain::Microphone);
        assert_eq!(result.outcome, expected);
        // A cancelled or failed request is not a refusal.
        let status = if expected == RequestOutcome::Denied {
            PermissionStatus::Denied
        } else {
            PermissionStatus::Available
        };
        assert_eq!(result.status, status);
    }
}

#[test]
fn microphone_request_outside_a_sandbox_makes_no_portal_call() {
    let fx = fixture(false);
    *fx.microphone.devices.lock() = Ok(MicrophoneDevices::NoDefaultSource);
    let result = request(&fx, PermissionDomain::Microphone);
    assert_eq!(fx.microphone.portal_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.status, PermissionStatus::SetupRequired);
    assert_eq!(result.outcome, RequestOutcome::Completed);

    let sandboxed = fixture(true);
    let result = request(&sandboxed, PermissionDomain::Microphone);
    assert_eq!(sandboxed.microphone.portal_calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.status, PermissionStatus::Available);
}

#[test]
fn checking_is_visible_while_probing_and_changes_are_notified() {
    let fx = fixture(false);
    let domain = PermissionDomain::DesktopCapture;
    assert_eq!(fx.service.cached_status(domain), PermissionStatus::Checking);
    let mut changes = fx.service.subscribe();

    let (release, gate) = oneshot::channel();
    *fx.capture.gate.lock() = Some(gate);
    let mut probe = pin!(fx.service.status(domain));
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(probe.as_mut().poll(&mut cx).is_pending());
    assert_eq!(fx.service.cached_status(domain), PermissionStatus::Checking);
    release.send(()).unwrap();
    assert_eq!(block_on(probe), PermissionStatus::Available);
    assert_eq!(
        fx.service.cached_status(domain),
        PermissionStatus::Available
    );
    assert_eq!(
        changes.try_recv().unwrap(),
        PermissionChange {
            domain,
            status: PermissionStatus::Available
        }
    );

    // An unchanged status is not announced again; a changed one is.
    status(&fx, domain);
    assert!(changes.try_recv().is_err());
    *fx.capture.result.lock() = Ok(CaptureAvailability::Unavailable);
    status(&fx, domain);
    assert_eq!(
        changes.try_recv().unwrap(),
        PermissionChange {
            domain,
            status: PermissionStatus::Unavailable
        }
    );

    // Dropping a probe in flight does not leave the domain stuck in checking.
    let (_release, gate) = oneshot::channel::<()>();
    *fx.capture.gate.lock() = Some(gate);
    let mut probe = Box::pin(fx.service.status(domain));
    assert!(probe.as_mut().poll(&mut cx).is_pending());
    drop(probe);
    assert_eq!(
        fx.service.cached_status(domain),
        PermissionStatus::Unavailable
    );
}

#[test]
fn reporting_a_capture_outcome_notifies_subscribers() {
    let fx = fixture(false);
    status(&fx, PermissionDomain::DesktopCapture);
    let mut changes = fx.service.subscribe();
    fx.service
        .report_capture_outcome(CaptureOperationOutcome::Denied);
    assert_eq!(
        changes.try_recv().unwrap(),
        PermissionChange {
            domain: PermissionDomain::DesktopCapture,
            status: PermissionStatus::Denied
        }
    );
}

#[test]
fn watching_notices_hot_plugged_devices() {
    let fx = fixture(false);
    let clock = ManualClock::new();
    let mut changes = fx.service.subscribe();
    let microphone = fx.microphone.clone();
    let result = clock.run(async {
        let watch = pin!(fx.service.watch(Duration::from_secs(2), &clock));
        let observer = pin!(async {
            for _ in 0..3 {
                changes.next().await.unwrap();
            }
            // The microphone is unplugged.
            *microphone.devices.lock() = Ok(MicrophoneDevices::NoCaptureSource);
            changes.next().await.unwrap()
        });
        match select(watch, observer).await {
            Either::Right((change, _)) => change,
            Either::Left((never, _)) => match never {},
        }
    });
    assert_eq!(
        result,
        PermissionChange {
            domain: PermissionDomain::Microphone,
            status: PermissionStatus::Unavailable
        }
    );
    assert_eq!(clock.elapsed(), Duration::from_secs(2));
}

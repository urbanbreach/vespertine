//! PLT-012 acceptance tests A2, A3, A4 (service level) and A6.

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use super::{
    test_support::{
        CallLog, FakeClock, FakeEnricher, FakeOwnWindows, FakePortal, FakeWindow, FakeX11,
        MemoryFs, test_image,
    },
    *,
};
use crate::shortcut::{Shortcut, ShortcutService};

const SCREEN: u32 = PortalTargets::SCREEN;
const WINDOW: u32 = PortalTargets::WINDOW;
const AREA: u32 = PortalTargets::AREA;
const ACTIVE: u32 = PortalTargets::ACTIVE_WINDOW;

fn probe(version: u32, mask: u32) -> Result<PortalProbe, PortalError> {
    Ok(PortalProbe {
        version,
        targets: PortalTargets(mask),
    })
}

fn screenshot_png() -> Vec<u8> {
    budget::encode_png_rgba(2, 2, &[9; 16]).unwrap()
}

fn fs_with_screenshot() -> MemoryFs {
    MemoryFs::default().with("/tmp/shot.png", screenshot_png())
}

const SHOT_URI: &str = "file:///tmp/shot.png";

struct WaylandFixture {
    log: CallLog,
    portal: Arc<FakePortal>,
    own: Arc<FakeOwnWindows>,
    service: CaptureService,
}

fn wayland(probe: Result<PortalProbe, PortalError>, fs: MemoryFs) -> WaylandFixture {
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe);
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let own = Arc::new(FakeOwnWindows(false.into()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal: portal.clone(),
            own_windows: own.clone(),
        },
        FakeClock::new(false),
        Arc::new(fs),
    );
    WaylandFixture {
        log,
        portal,
        own,
        service,
    }
}

struct X11Fixture {
    log: CallLog,
    portal: Arc<FakePortal>,
    server: Arc<FakeX11>,
    service: CaptureService,
}

fn x11_service(
    probe: Result<PortalProbe, PortalError>,
    fs: MemoryFs,
    enricher: Option<Arc<FakeEnricher>>,
    clock: Arc<FakeClock>,
) -> X11Fixture {
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe);
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let server = FakeX11::new(&log);
    let mut service = CaptureService::with_services(
        CaptureBackend::X11 {
            portal: portal.clone(),
            server: server.clone(),
        },
        clock,
        Arc::new(fs),
    )
    .with_config(CaptureConfig {
        own_pid: 4242,
        ..CaptureConfig::default()
    });
    if let Some(enricher) = enricher {
        service = service.with_enricher(enricher);
    }
    X11Fixture {
        log,
        portal,
        server,
        service,
    }
}

fn x11_native(enricher: Option<Arc<FakeEnricher>>, clock: Arc<FakeClock>) -> X11Fixture {
    // A portal without the active-window target forces the native path.
    let fixture = x11_service(
        probe(3, SCREEN | WINDOW),
        MemoryFs::default(),
        enricher,
        clock,
    );
    fixture
        .server
        .add_window(10, FakeWindow::titled(1000, "Editor"));
    *fixture.server.active.lock() = Some(10);
    fixture
}

fn capture(service: &CaptureService, target: CaptureTarget) -> Result<CapturedImage, CaptureError> {
    smol::block_on(service.capture(target))
}

// ---------------------------------------------------------------- A2 (R2)

#[test]
fn portal_version_below_three_fails_without_a_capture_request() {
    let f = wayland(probe(2, SCREEN | WINDOW | ACTIVE), fs_with_screenshot());
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    let CaptureError::Failed(message) = error else {
        panic!("expected failure, got {error:?}");
    };
    assert!(message.contains("window-only capture"), "{message}");
    assert!(message.contains("needs an update"), "{message}");
    assert!(f.portal.requests().is_empty());
    assert_eq!(f.log.count("portal.screenshot"), 0);
}

#[test]
fn empty_available_target_set_fails_without_a_capture_request() {
    for mask in [0, SCREEN, SCREEN | AREA] {
        let f = wayland(probe(3, mask), fs_with_screenshot());
        let error = capture(&f.service, CaptureTarget::PortalWindowPicker).unwrap_err();
        assert!(matches!(error, CaptureError::Failed(_)), "{error:?}");
        assert!(f.portal.requests().is_empty(), "mask {mask}");
    }
}

#[test]
fn unreachable_portal_fails_without_a_capture_request() {
    let f = wayland(
        Err(PortalError::Unreachable("no bus".into())),
        fs_with_screenshot(),
    );
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(error.to_string().contains("no bus"), "{error}");
    assert!(f.portal.requests().is_empty());
    assert_eq!(
        smol::block_on(f.service.capability()).window_capture,
        CapabilityState::Unavailable
    );
}

#[test]
fn window_only_portal_is_user_selection_with_an_interactive_request() {
    let f = wayland(probe(3, SCREEN | WINDOW), fs_with_screenshot());
    let capability = smol::block_on(f.service.capability());
    assert_eq!(capability.window_capture, CapabilityState::UserSelection);
    assert_eq!(capability.target, CaptureTarget::PortalWindowPicker);

    let image = capture(&f.service, capability.target).unwrap();
    assert_eq!(
        f.portal.requests(),
        vec![ScreenshotRequest {
            target: WindowTarget::Window,
            interactive: true
        }]
    );
    assert_eq!(image.metadata.kind, TargetKind::PortalSelection);
}

#[test]
fn active_window_target_is_a_non_interactive_request() {
    let f = wayland(probe(3, SCREEN | WINDOW | ACTIVE), fs_with_screenshot());
    let capability = smol::block_on(f.service.capability());
    assert_eq!(capability.window_capture, CapabilityState::Ready);
    assert_eq!(capability.target, CaptureTarget::ActiveWindow);

    let image = capture(&f.service, capability.target).unwrap();
    assert_eq!(
        f.portal.requests(),
        vec![ScreenshotRequest {
            target: WindowTarget::ActiveWindow,
            interactive: false
        }]
    );
    assert_eq!(image.metadata.kind, TargetKind::PortalActiveWindow);
}

#[test]
fn a_full_screen_request_is_never_issued() {
    for mask in 0..16 {
        for version in [0, 2, 3, 4] {
            for requested in [
                CaptureTarget::ActiveWindow,
                CaptureTarget::PortalWindowPicker,
            ] {
                let f = wayland(probe(version, mask), fs_with_screenshot());
                let _ = capture(&f.service, requested);
                for request in f.portal.requests() {
                    assert!(
                        matches!(
                            request.target,
                            WindowTarget::Window | WindowTarget::ActiveWindow
                        ),
                        "mask {mask} version {version}"
                    );
                    assert!(version >= 3);
                    assert!(mask & (WINDOW | ACTIVE) != 0);
                }
            }
        }
    }
}

#[test]
fn a_target_that_disappears_before_sending_aborts_the_request() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.portal
        .script_probes(vec![probe(3, ACTIVE), probe(3, SCREEN), probe(3, SCREEN)]);
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(matches!(error, CaptureError::Failed(_)), "{error:?}");
    assert!(f.portal.requests().is_empty());
}

#[test]
fn the_portal_is_probed_before_every_capture_and_the_fresh_target_is_used() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.portal.script_probes(vec![
        probe(3, ACTIVE),
        probe(3, ACTIVE),
        probe(3, WINDOW),
        probe(3, WINDOW),
    ]);
    capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    let requests = f.portal.requests();
    assert_eq!(requests[0].target, WindowTarget::ActiveWindow);
    assert!(!requests[0].interactive);
    assert_eq!(requests[1].target, WindowTarget::Window);
    assert!(requests[1].interactive);
    assert!(f.log.count("portal.probe") >= 4);
}

#[test]
fn portal_results_carry_a_generic_label_and_an_empty_snapshot() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(image.metadata.display_name, "Selected window");
    assert_eq!(image.metadata.identifier, "portal:active-window");
    assert_eq!(image.semantic, SemanticSnapshot::empty());
    assert!(image.enrichment.is_none());
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.png, screenshot_png());
    assert!(image.id.starts_with("capture-"));
    assert_eq!(image.captured_at, FakeClock::new(false).now);
}

#[test]
fn capture_ids_are_unique() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    let a = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    let b = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_ne!(a.id, b.id);
}

#[test]
fn portal_cancel_deny_and_failure_stay_distinct() {
    let cases = [
        (PortalError::Cancelled, CaptureError::Cancelled),
        (
            PortalError::Denied("no".into()),
            CaptureError::PermissionDenied("no".into()),
        ),
    ];
    for (portal_error, expected) in cases {
        let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
        f.portal.respond_with(Err(portal_error));
        assert_eq!(
            capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err(),
            expected
        );
    }
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.portal
        .respond_with(Err(PortalError::Failed("boom".into())));
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(matches!(&error, CaptureError::Failed(reason) if reason.contains("boom")));
}

#[test]
fn denial_marks_permission_required_until_a_later_success() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.portal.respond_with(Err(PortalError::Denied("no".into())));
    capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(f.service.last_capture_denied());
    assert_eq!(
        smol::block_on(f.service.capability()).window_capture,
        CapabilityState::PermissionRequired
    );
    assert_eq!(
        smol::block_on(f.service.availability()),
        CaptureAvailability::Ready
    );
    f.portal.respond_with(Ok(SHOT_URI.to_owned()));
    capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert!(!f.service.last_capture_denied());
}

struct RecordingSink(parking_lot::Mutex<Vec<CaptureOutcomeKind>>);

impl CaptureOutcomeSink for RecordingSink {
    fn report(&self, outcome: CaptureOutcomeKind) {
        self.0.lock().push(outcome);
    }
}

#[test]
fn portal_outcomes_are_reported_to_the_sink() {
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    let sink = Arc::new(RecordingSink(Default::default()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal: portal.clone(),
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        Arc::new(fs_with_screenshot()),
    )
    .with_outcome_sink(sink.clone());
    for response in [
        Ok(SHOT_URI.to_owned()),
        Err(PortalError::Denied("x".into())),
        Err(PortalError::Cancelled),
        Err(PortalError::Failed("y".into())),
    ] {
        portal.respond_with(response);
        let _ = capture(&service, CaptureTarget::ActiveWindow);
    }
    assert_eq!(
        *sink.0.lock(),
        vec![
            CaptureOutcomeKind::Succeeded,
            CaptureOutcomeKind::Denied,
            CaptureOutcomeKind::Cancelled,
            CaptureOutcomeKind::Failed
        ]
    );
}

#[test]
fn non_file_uris_are_rejected() {
    for uri in [
        "https://example.com/shot.png",
        "file://remote-host/tmp/shot.png",
        "not a uri",
        "data:image/png;base64,AAAA",
    ] {
        let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
        f.portal.respond_with(Ok(uri.to_owned()));
        let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
        assert!(
            matches!(&error, CaptureError::Failed(reason) if reason.contains("non-file URI")),
            "{uri}: {error:?}"
        );
    }
}

// ---------------------------------------------------------------- A4 (R2.4, R6)

#[test]
fn a_file_larger_than_the_cap_is_rejected_before_opening() {
    let fs = fs_with_screenshot();
    let size = screenshot_png().len() as u64;
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let fs = Arc::new(fs);
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal,
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        fs.clone(),
    )
    .with_config(CaptureConfig {
        encoded_image_cap: size - 1,
        ..CaptureConfig::default()
    });
    let error = capture(&service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(
        matches!(&error, CaptureError::Failed(reason) if reason.contains("exceeds")),
        "{error:?}"
    );
    assert!(fs.opened.lock().is_empty(), "the file was opened");
}

#[test]
fn a_file_at_exactly_the_cap_is_accepted() {
    let size = screenshot_png().len() as u64;
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal,
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        Arc::new(fs_with_screenshot()),
    )
    .with_config(CaptureConfig {
        encoded_image_cap: size,
        ..CaptureConfig::default()
    });
    assert!(capture(&service, CaptureTarget::ActiveWindow).is_ok());
}

#[test]
fn a_file_growing_while_read_is_rejected() {
    let size = screenshot_png().len() as u64;
    let mut fs = fs_with_screenshot();
    // The length check sees the original size, the read sees more than the cap.
    let mut grown = screenshot_png();
    grown.resize(size as usize + 64, 0);
    fs.files.insert("/tmp/shot.png".into(), grown);
    fs.reported_len.insert("/tmp/shot.png".into(), size);
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal,
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        Arc::new(fs),
    )
    .with_config(CaptureConfig {
        encoded_image_cap: size,
        ..CaptureConfig::default()
    });
    let error = capture(&service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(
        matches!(&error, CaptureError::Failed(reason) if reason.contains("grew")),
        "{error:?}"
    );
}

#[test]
fn the_default_cap_is_24_mib() {
    assert_eq!(CaptureConfig::default().encoded_image_cap, 24 * 1024 * 1024);
}

#[test]
fn a_portal_file_that_is_not_a_valid_png_fails() {
    for data in [b"GIF89a.....".to_vec(), vec![0u8; 100], Vec::new()] {
        let f = wayland(
            probe(3, ACTIVE),
            MemoryFs::default().with("/tmp/shot.png", data),
        );
        let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
        assert!(matches!(error, CaptureError::Failed(_)));
    }
}

#[test]
fn a_portal_png_over_the_dimension_budget_fails_without_decoding() {
    let mut png = screenshot_png();
    png[16..20].copy_from_slice(&8193u32.to_be_bytes());
    let f = wayland(
        probe(3, ACTIVE),
        MemoryFs::default().with("/tmp/shot.png", png),
    );
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(
        matches!(&error, CaptureError::Failed(reason) if reason.contains("8193 x 2")),
        "{error:?}"
    );
}

// ---------------------------------------------------------------- R5

#[test]
fn own_active_window_is_self_capture_without_contacting_the_portal() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.own.0.store(true, Ordering::SeqCst);
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert_eq!(
        error,
        CaptureError::SelfCapture(DEFAULT_SELF_CAPTURE_MESSAGE.to_owned())
    );
    assert!(f.log.entries().is_empty(), "{:?}", f.log.entries());
}

#[test]
fn user_selection_mode_does_not_filter_own_windows() {
    let f = wayland(probe(3, WINDOW), fs_with_screenshot());
    f.own.0.store(true, Ordering::SeqCst);
    assert!(capture(&f.service, CaptureTarget::PortalWindowPicker).is_ok());
}

#[test]
fn self_capture_is_also_detected_when_the_probe_turns_the_target_automatic() {
    let f = wayland(probe(3, ACTIVE), fs_with_screenshot());
    f.own.0.store(true, Ordering::SeqCst);
    let error = capture(&f.service, CaptureTarget::PortalWindowPicker).unwrap_err();
    assert!(matches!(error, CaptureError::SelfCapture(_)));
    assert!(f.portal.requests().is_empty());
}

#[test]
fn self_capture_message_is_configurable() {
    let log = CallLog::default();
    let own = Arc::new(FakeOwnWindows(true.into()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal: FakePortal::new(&log, probe(3, ACTIVE)),
            own_windows: own,
        },
        FakeClock::new(false),
        Arc::new(MemoryFs::default()),
    )
    .with_config(CaptureConfig {
        self_capture_message: "pick another window".into(),
        ..CaptureConfig::default()
    });
    assert_eq!(
        capture(&service, CaptureTarget::ActiveWindow).unwrap_err(),
        CaptureError::SelfCapture("pick another window".into())
    );
}

// ---------------------------------------------------------------- R1 gate

#[test]
fn requests_are_refused_while_switched_off_or_recording() {
    let shortcuts = ShortcutService::new(Shortcut::parse("ctrl-alt-space").unwrap(), true);
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal: portal.clone(),
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        Arc::new(fs_with_screenshot()),
    )
    .with_shortcuts(shortcuts.clone());

    shortcuts.set_enabled(false);
    assert!(matches!(
        capture(&service, CaptureTarget::ActiveWindow),
        Err(CaptureError::ShortcutUnavailable(_))
    ));
    shortcuts.set_enabled(true);
    shortcuts.set_recording(true);
    assert!(matches!(
        capture(&service, CaptureTarget::ActiveWindow),
        Err(CaptureError::ShortcutUnavailable(_))
    ));
    assert!(matches!(
        smol::block_on(service.capture_on_activation()),
        Some(Err(CaptureError::ShortcutUnavailable(_)))
    ));
    assert!(portal.requests().is_empty());
    shortcuts.set_recording(false);
    assert!(capture(&service, CaptureTarget::ActiveWindow).is_ok());
}

#[test]
fn a_second_activation_while_a_capture_runs_is_ignored() {
    let log = CallLog::default();
    let portal = FakePortal::new(&log, probe(3, ACTIVE));
    portal.respond_with(Ok(SHOT_URI.to_owned()));
    let (release, gate) = futures::channel::oneshot::channel();
    *portal.gate.lock() = Some(gate);
    let service = CaptureService::with_services(
        CaptureBackend::Wayland {
            portal: portal.clone(),
            own_windows: Arc::new(FakeOwnWindows(false.into())),
        },
        FakeClock::new(false),
        Arc::new(fs_with_screenshot()),
    );
    let (first, second) = smol::block_on(futures::future::join(
        service.capture_on_activation(),
        async {
            // Let the first capture reach the portal and block there.
            while portal.requests().is_empty() {
                smol::Timer::at(std::time::Instant::now() + Duration::from_millis(1)).await;
            }
            let second = service.capture_on_activation().await;
            release.send(()).unwrap();
            second
        },
    ));
    assert!(second.is_none(), "second activation must be ignored");
    assert!(first.unwrap().is_ok());
    assert_eq!(portal.requests().len(), 1);
    // The guard is released afterwards.
    assert!(smol::block_on(service.capture_on_activation()).is_some());
}

// ---------------------------------------------------------------- capability

#[test]
fn x11_capability_prefers_the_portal_then_native_and_reports_application_text() {
    let enricher = FakeEnricher::new(Some(None));
    let f = x11_service(
        probe(3, ACTIVE),
        MemoryFs::default(),
        Some(enricher),
        FakeClock::new(false),
    );
    let capability = smol::block_on(f.service.capability());
    assert_eq!(capability.platform, DesktopPlatform::X11);
    assert_eq!(capability.window_capture, CapabilityState::Ready);
    assert_eq!(capability.target, CaptureTarget::ActiveWindow);
    assert_eq!(capability.application_text, CapabilityState::Ready);
    assert_eq!(capability.shortcut, CapabilityState::Unavailable);

    let native = x11_native(None, FakeClock::new(false));
    let capability = smol::block_on(native.service.capability());
    assert_eq!(capability.window_capture, CapabilityState::Ready);
    assert_eq!(capability.application_text, CapabilityState::Unavailable);
}

#[test]
fn wayland_application_text_is_unavailable() {
    let f = wayland(probe(3, ACTIVE), MemoryFs::default());
    let capability = smol::block_on(f.service.capability());
    assert_eq!(capability.platform, DesktopPlatform::Wayland);
    assert_eq!(capability.application_text, CapabilityState::Unavailable);
}

// ---------------------------------------------------------------- A2/A3 (R3)

#[test]
fn started_portal_operations_on_x11_never_fall_through_to_the_native_path() {
    let outcomes = [
        (PortalError::Cancelled, CaptureError::Cancelled),
        (
            PortalError::Denied("no".into()),
            CaptureError::PermissionDenied("no".into()),
        ),
        (
            PortalError::Failed("boom".into()),
            CaptureError::Failed("boom".into()),
        ),
    ];
    for (portal_error, expected) in outcomes {
        let f = x11_service(
            probe(3, ACTIVE | WINDOW),
            MemoryFs::default(),
            None,
            FakeClock::new(false),
        );
        f.server.add_window(10, FakeWindow::titled(1000, "Editor"));
        *f.server.active.lock() = Some(10);
        f.portal.respond_with(Err(portal_error));
        let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
        match (&error, &expected) {
            (CaptureError::Failed(reason), CaptureError::Failed(expected)) => {
                assert!(reason.contains(expected.as_str()))
            }
            _ => assert_eq!(error, expected),
        }
        assert_eq!(f.log.count("portal.screenshot"), 1);
        assert_eq!(f.log.count("x11.get_image"), 0, "native grab was invoked");
        assert_eq!(f.server.image_reads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn x11_portal_with_active_window_serves_the_capture() {
    let f = x11_service(
        probe(3, ACTIVE),
        fs_with_screenshot(),
        None,
        FakeClock::new(false),
    );
    f.server.add_window(10, FakeWindow::titled(1000, "Editor"));
    *f.server.active.lock() = Some(10);
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(image.metadata.kind, TargetKind::PortalActiveWindow);
    assert_eq!(f.log.count("x11.get_image"), 0);
}

#[test]
fn x11_without_portal_active_window_captures_natively() {
    for probe_result in [
        probe(3, SCREEN | WINDOW),
        probe(2, ACTIVE),
        Err(PortalError::Unreachable("down".into())),
    ] {
        let f = x11_service(
            probe_result,
            MemoryFs::default(),
            None,
            FakeClock::new(false),
        );
        f.server.add_window(10, FakeWindow::titled(1000, "Editor"));
        *f.server.active.lock() = Some(10);
        let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
        assert!(f.portal.requests().is_empty());
        assert_eq!(f.log.count("x11.get_image"), 1);
        assert_eq!(image.metadata.kind, TargetKind::X11Window);
    }
}

#[test]
fn native_capture_converts_pixels_to_a_png_with_metadata() {
    let fs = MemoryFs {
        home: Some("/home/u".into()),
        ..Default::default()
    }
    .with(
        "/usr/share/applications/editor.desktop",
        "[Desktop Entry]\nName=Fancy Editor\nIcon=/opt/editor.png\n",
    )
    .with(
        "/opt/editor.png",
        budget::encode_png_rgba(1, 1, &[1, 2, 3, 4]).unwrap(),
    );
    let f = x11_service(probe(3, WINDOW), fs, None, FakeClock::new(false));
    let mut window = FakeWindow::titled(1000, "Main — Editor");
    window.class = Some(b"editor\0Editor\0".to_vec());
    f.server.add_window(10, window);
    *f.server.active.lock() = Some(10);

    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!((image.width, image.height), (2, 2));
    let decoded = budget::decode_png_rgba(&image.png).unwrap();
    assert_eq!((decoded.width, decoded.height), (2, 2));
    let reference = test_image(2, 2);
    for (index, pixel) in decoded.rgba.chunks_exact(4).enumerate() {
        assert_eq!(pixel, [(index * 10) as u8, 0x20, 0x30, 255]);
    }
    assert_eq!(reference.data.len(), 16);

    assert_eq!(image.metadata.title.as_deref(), Some("Main — Editor"));
    assert_eq!(image.metadata.class.as_deref(), Some("Editor"));
    assert_eq!(image.metadata.identifier, "editor");
    assert_eq!(image.metadata.display_name, "Fancy Editor");
    assert!(image.metadata.icon_png.is_some());
    assert_eq!(image.semantic, SemanticSnapshot::empty());
}

#[test]
fn native_metadata_without_a_desktop_entry_uses_the_class() {
    let f = x11_service(
        probe(3, WINDOW),
        MemoryFs::default(),
        None,
        FakeClock::new(false),
    );
    let mut window = FakeWindow::titled(1000, "T");
    window.class = Some(b"Xterm\0".to_vec());
    f.server.add_window(10, window);
    *f.server.active.lock() = Some(10);
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(image.metadata.identifier, "linux-x11:xterm");
    assert_eq!(image.metadata.display_name, "Xterm");

    let mut anonymous = FakeWindow::titled(2000, "Anon");
    anonymous.class = None;
    f.server.add_window(11, anonymous);
    *f.server.active.lock() = Some(11);
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(image.metadata.display_name, "X11 application");
}

#[test]
fn own_pid_active_is_self_capture_before_any_portal_call_or_pixel_read() {
    let f = x11_service(
        probe(3, ACTIVE),
        fs_with_screenshot(),
        None,
        FakeClock::new(false),
    );
    f.server
        .add_window(10, FakeWindow::titled(4242, "Vespertine"));
    *f.server.active.lock() = Some(10);
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    assert!(matches!(error, CaptureError::SelfCapture(_)), "{error:?}");
    assert_eq!(f.log.count("portal.probe"), 0);
    assert_eq!(f.log.count("portal.screenshot"), 0);
    assert_eq!(f.log.count("x11.get_image"), 0);
    assert_eq!(f.log.count("x11.window_geometry"), 0);
}

#[test]
fn self_capture_check_runs_standalone_before_a_capture_starts() {
    let f = x11_native(None, FakeClock::new(false));
    assert_eq!(smol::block_on(f.service.check_self_capture()), Ok(()));
    f.server.add_window(11, FakeWindow::titled(4242, "Own"));
    *f.server.active.lock() = Some(11);
    assert!(matches!(
        smol::block_on(f.service.check_self_capture()),
        Err(CaptureError::SelfCapture(_))
    ));
}

#[test]
fn zero_sized_or_missing_active_window_is_no_eligible_window() {
    for (width, height) in [(0, 10), (10, 0), (0, 0)] {
        let f = x11_native(None, FakeClock::new(false));
        f.server.windows.lock().get_mut(&10).unwrap().geometry =
            Some(x11::WindowGeometry { width, height });
        assert_eq!(
            capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err(),
            CaptureError::NoEligibleWindow
        );
        assert_eq!(f.log.count("x11.get_image"), 0);
    }
    let f = x11_native(None, FakeClock::new(false));
    *f.server.active.lock() = None;
    assert_eq!(
        capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err(),
        CaptureError::NoEligibleWindow
    );
}

#[test]
fn a_stale_active_window_id_is_no_eligible_window() {
    let f = x11_native(None, FakeClock::new(false));
    *f.server.active.lock() = Some(99);
    assert_eq!(smol::block_on(f.service.check_self_capture()), Ok(()));
    assert_eq!(
        capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err(),
        CaptureError::NoEligibleWindow
    );
    assert_eq!(f.log.count("x11.get_image"), 0);
}

#[test]
fn the_active_window_changing_during_capture_fails() {
    let f = x11_native(None, FakeClock::new(false));
    f.server.add_window(11, FakeWindow::titled(2000, "Other"));
    *f.server.switch_active_after_image.lock() = Some(Some(11));
    let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
    let CaptureError::Failed(message) = error else {
        panic!("expected failure");
    };
    assert!(message.contains("active window changed"), "{message}");
    assert!(message.contains("try again"), "{message}");

    // Also when the active window disappears entirely.
    let f = x11_native(None, FakeClock::new(false));
    *f.server.switch_active_after_image.lock() = Some(None);
    assert!(matches!(
        capture(&f.service, CaptureTarget::ActiveWindow),
        Err(CaptureError::Failed(_))
    ));
}

#[test]
fn over_budget_native_windows_fail_naming_the_dimensions_without_reading_pixels() {
    for (width, height) in [(8193, 1), (8192, 8192), (8192, 4097)] {
        let f = x11_native(None, FakeClock::new(false));
        f.server.windows.lock().get_mut(&10).unwrap().geometry =
            Some(x11::WindowGeometry { width, height });
        let error = capture(&f.service, CaptureTarget::ActiveWindow).unwrap_err();
        let CaptureError::Failed(message) = error else {
            panic!("expected failure for {width} x {height}");
        };
        assert!(
            message.contains(&format!("{width} x {height}")),
            "{message}"
        );
        assert_eq!(f.log.count("x11.get_image"), 0, "nothing was read");
    }
}

#[test]
fn a_maximum_budget_window_passes_validation() {
    assert_eq!(validate_dimensions(8192, 4096), Ok(128 * 1024 * 1024));
}

// ------------------------------------------------- R3 step 5: verifiable identity

fn identity_server(windows: &[(WindowId, Option<u32>, &str)]) -> Arc<FakeX11> {
    let server = FakeX11::new(&CallLog::default());
    for (id, pid, title) in windows {
        let mut window = FakeWindow::titled(0, title);
        window.pid = *pid;
        server.add_window(*id, window);
    }
    server
}

#[test]
fn unique_pid_and_title_is_verifiable() {
    let server = identity_server(&[
        (1, Some(50), "Doc"),
        (2, Some(50), "Other"),
        (3, Some(51), "Doc"),
    ]);
    let verified = verify_window_identity(&*server, 1).unwrap();
    assert_eq!(
        verified,
        VerifiedWindow {
            window: 1,
            identity: WindowIdentity {
                pid: 50,
                title: "Doc".into()
            }
        }
    );
}

#[test]
fn duplicate_title_pid_zero_or_blank_title_is_not_verifiable() {
    let duplicate = identity_server(&[(1, Some(50), "Doc"), (2, Some(50), "Doc")]);
    assert!(verify_window_identity(&*duplicate, 1).is_none());
    assert!(verify_window_identity(&*duplicate, 2).is_none());

    let zero = identity_server(&[(1, Some(0), "Doc")]);
    assert!(verify_window_identity(&*zero, 1).is_none());
    let missing = identity_server(&[(1, None, "Doc")]);
    assert!(verify_window_identity(&*missing, 1).is_none());

    for blank in ["", "   ", "\t"] {
        let server = identity_server(&[(1, Some(50), blank)]);
        assert!(verify_window_identity(&*server, 1).is_none(), "{blank:?}");
    }

    // Titles differing only by whitespace are different exact titles.
    let near = identity_server(&[(1, Some(50), "Doc"), (2, Some(50), "Doc ")]);
    assert!(verify_window_identity(&*near, 1).is_some());

    // A window missing from _NET_CLIENT_LIST cannot be verified.
    let server = identity_server(&[(1, Some(50), "Doc")]);
    server.client_list.lock().clear();
    assert!(verify_window_identity(&*server, 1).is_none());
}

// ------------------------------------------------------------- A6 (R7)

fn enriched(name: &str) -> EnrichedContext {
    EnrichedContext {
        application_name: Some(name.to_owned()),
        snapshot: SemanticSnapshot {
            format_version: 1,
            content: "window: Editor\n".into(),
            truncated: false,
        },
    }
}

#[test]
fn a_hanging_enrichment_never_delays_or_changes_the_image() {
    let enricher = FakeEnricher::new(None);
    let f = x11_native(Some(enricher.clone()), FakeClock::new(false));
    let started = Instant::now();
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    // The hook was not even started by the capture.
    assert!(enricher.calls.lock().is_empty());
    assert_eq!(image.semantic, SemanticSnapshot::empty());
    let pending = image.enrichment.as_ref().expect("verifiable identity");
    assert_eq!(
        pending.identity(),
        &WindowIdentity {
            pid: 1000,
            title: "Editor".into()
        }
    );
    assert_eq!(image.metadata.display_name, "X11 application");
}

#[test]
fn enrichment_timeout_returns_none_and_the_image_is_unchanged() {
    let enricher = FakeEnricher::new(None);
    let f = x11_native(Some(enricher.clone()), FakeClock::new(true));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    let png = image.png.clone();
    let pending = image.enrichment.take().unwrap();
    assert_eq!(smol::block_on(pending.resolve()), None);
    assert_eq!(enricher.calls.lock().len(), 1);
    assert!(
        enricher.dropped.load(Ordering::SeqCst),
        "the timed-out hook future is dropped"
    );
    assert_eq!(image.png, png);
    assert_eq!(image.semantic, SemanticSnapshot::empty());
}

#[test]
fn enrichment_timeout_with_the_real_clock_is_prompt() {
    let enricher = FakeEnricher::new(None);
    let log = CallLog::default();
    let server = FakeX11::new(&log);
    server.add_window(10, FakeWindow::titled(1000, "Editor"));
    *server.active.lock() = Some(10);
    let service = CaptureService::new(CaptureBackend::X11 {
        portal: FakePortal::new(&log, probe(3, WINDOW)),
        server,
    })
    .with_config(CaptureConfig {
        own_pid: 4242,
        enrichment_budget: Duration::from_millis(50),
        ..CaptureConfig::default()
    })
    .with_enricher(enricher);
    let started = Instant::now();
    let mut image = capture(&service, CaptureTarget::ActiveWindow).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let pending = image.enrichment.take().unwrap();
    let resolved = Instant::now();
    assert_eq!(smol::block_on(pending.resolve()), None);
    let waited = resolved.elapsed();
    assert!(waited >= Duration::from_millis(40), "{waited:?}");
    assert!(waited < Duration::from_secs(2), "{waited:?}");
}

#[test]
fn cancelling_the_enrichment_drops_the_hook_future() {
    let enricher = FakeEnricher::new(None);
    let f = x11_native(Some(enricher.clone()), FakeClock::new(false));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    let pending = image.enrichment.take().unwrap();

    // Poll once so the hook future exists, then cancel by dropping.
    let mut resolve = Box::pin(pending.resolve());
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(Future::poll(resolve.as_mut(), &mut cx).is_pending());
    assert!(!enricher.dropped.load(Ordering::SeqCst));
    drop(resolve);
    assert!(enricher.dropped.load(Ordering::SeqCst));

    // Explicit cancel of a never-started enrichment is also fine.
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    image.enrichment.take().unwrap().cancel();
    assert_eq!(image.semantic, SemanticSnapshot::empty());
}

#[test]
fn a_failed_hook_yields_no_context() {
    let enricher = FakeEnricher::new(Some(None));
    let f = x11_native(Some(enricher), FakeClock::new(false));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(
        smol::block_on(image.enrichment.take().unwrap().resolve()),
        None
    );
}

#[test]
fn a_successful_hook_attaches_when_the_active_window_is_unchanged() {
    let enricher = FakeEnricher::new(Some(Some(enriched("Fancy"))));
    let f = x11_native(Some(enricher), FakeClock::new(false));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    let context = smol::block_on(image.enrichment.take().unwrap().resolve()).unwrap();
    image.apply_enrichment(context);
    assert_eq!(image.semantic.content, "window: Editor\n");
    // Display name falls back desktop Name -> semantic name -> class.
    assert_eq!(image.metadata.display_name, "Fancy");
}

#[test]
fn enrichment_is_discarded_when_the_active_window_changed_afterwards() {
    let enricher = FakeEnricher::new(Some(Some(enriched("Fancy"))));
    let f = x11_native(Some(enricher.clone()), FakeClock::new(false));
    f.server
        .add_window(11, FakeWindow::titled(2000, "Elsewhere"));
    let server = f.server.clone();
    *enricher.on_complete.lock() = Some(Box::new(move || {
        *server.active.lock() = Some(11);
    }));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(
        smol::block_on(image.enrichment.take().unwrap().resolve()),
        None
    );
}

#[test]
fn enrichment_is_discarded_when_the_identity_stopped_being_unique() {
    let enricher = FakeEnricher::new(Some(Some(enriched("Fancy"))));
    let f = x11_native(Some(enricher.clone()), FakeClock::new(false));
    let server = f.server.clone();
    *enricher.on_complete.lock() = Some(Box::new(move || {
        // A second window with the same pid and title appears.
        server.add_window(12, FakeWindow::titled(1000, "Editor"));
    }));
    let mut image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert_eq!(
        smol::block_on(image.enrichment.take().unwrap().resolve()),
        None
    );
}

#[test]
fn no_enrichment_is_requested_for_an_unverifiable_identity() {
    let enricher = FakeEnricher::new(Some(Some(enriched("Fancy"))));
    let f = x11_native(Some(enricher.clone()), FakeClock::new(false));
    f.server.add_window(11, FakeWindow::titled(1000, "Editor"));
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert!(image.enrichment.is_none());
    assert!(enricher.calls.lock().is_empty());
}

#[test]
fn portal_captures_never_request_enrichment() {
    let enricher = FakeEnricher::new(Some(Some(enriched("Fancy"))));
    let f = x11_service(
        probe(3, ACTIVE),
        fs_with_screenshot(),
        Some(enricher.clone()),
        FakeClock::new(false),
    );
    f.server.add_window(10, FakeWindow::titled(1000, "Editor"));
    *f.server.active.lock() = Some(10);
    let image = capture(&f.service, CaptureTarget::ActiveWindow).unwrap();
    assert!(image.enrichment.is_none());
    assert_eq!(image.semantic, SemanticSnapshot::empty());
    assert!(enricher.calls.lock().is_empty());
}

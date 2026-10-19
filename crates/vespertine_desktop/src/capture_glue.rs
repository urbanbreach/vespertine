//! Glue between desktop capture (PLT-012) and the accessibility context and
//! permission status services (PLT-013).
//!
//! - [`AtspiContextEnricher`] implements capture's enrichment hook with a
//!   bounded [`ContextQuery`]; the capture never awaits it and dropping the
//!   returned future cancels the query.
//! - [`CaptureService`] is the capture capability source of the
//!   [`PermissionService`].
//! - [`PermissionService`] is the outcome sink of the capture service, so that a
//!   denied portal operation is remembered until a later success or an explicit
//!   new request.

use crate::{
    capture::{
        CaptureAvailability as ProbedAvailability, CaptureOutcomeKind, CaptureOutcomeSink,
        CaptureService, ContextEnricher, EnrichedContext, SemanticSnapshot, WindowIdentity,
    },
    context::{AccessibilityBus, ClockSource, ContextQuery, ContextRequest},
    permissions::{
        CaptureAvailability, CaptureOperationOutcome, DesktopCaptureStatusSource, PermissionError,
        PermissionService,
    },
};
use futures::future::{BoxFuture, FutureExt};
use std::sync::Arc;

/// Capture's enrichment hook over the AT-SPI context query.
#[derive(Clone)]
pub struct AtspiContextEnricher {
    bus: Arc<dyn AccessibilityBus>,
    clock: Arc<dyn ClockSource>,
}

impl AtspiContextEnricher {
    /// An enricher over the real accessibility bus and the system clock.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn new() -> Self {
        Self::with_services(
            Arc::new(crate::context::ZbusAccessibilityBus),
            Arc::new(crate::context::SystemClock),
        )
    }

    /// An enricher over the given bus and clock.
    pub fn with_services(bus: Arc<dyn AccessibilityBus>, clock: Arc<dyn ClockSource>) -> Self {
        Self { bus, clock }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Default for AtspiContextEnricher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextEnricher for AtspiContextEnricher {
    fn is_available(&self) -> BoxFuture<'static, bool> {
        let connect = self.bus.connect();
        async move { connect.await.is_ok() }.boxed()
    }

    fn enrich(&self, identity: &WindowIdentity) -> BoxFuture<'static, Option<EnrichedContext>> {
        let query = ContextQuery::with_services(
            self.bus.clone(),
            self.clock.clone(),
            ContextRequest::new(identity.pid, identity.title.clone()),
        );
        async move {
            let context = query.await.ok()?;
            Some(EnrichedContext {
                application_name: Some(context.application_name),
                snapshot: SemanticSnapshot {
                    format_version: context.snapshot.version,
                    content: context.snapshot.content,
                    truncated: context.snapshot.truncated,
                },
            })
        }
        .boxed()
    }
}

fn availability_of(probed: ProbedAvailability) -> CaptureAvailability {
    match probed {
        ProbedAvailability::Ready => CaptureAvailability::Ready,
        ProbedAvailability::UserSelection => CaptureAvailability::UserSelection,
        ProbedAvailability::Unavailable => CaptureAvailability::Unavailable,
    }
}

fn operation_outcome_of(outcome: CaptureOutcomeKind) -> CaptureOperationOutcome {
    match outcome {
        CaptureOutcomeKind::Succeeded => CaptureOperationOutcome::Succeeded,
        CaptureOutcomeKind::Denied => CaptureOperationOutcome::Denied,
        CaptureOutcomeKind::Cancelled => CaptureOperationOutcome::Cancelled,
        CaptureOutcomeKind::Failed => CaptureOperationOutcome::Failed,
    }
}

impl DesktopCaptureStatusSource for CaptureService {
    fn availability(&self) -> BoxFuture<'_, Result<CaptureAvailability, PermissionError>> {
        async move { Ok(availability_of(CaptureService::availability(self).await)) }.boxed()
    }
}

impl CaptureOutcomeSink for PermissionService {
    fn report(&self, outcome: CaptureOutcomeKind) {
        self.report_capture_outcome(operation_outcome_of(outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::fake::{FakeBus, FakeNodeSpec, ManualClock},
        permissions::{
            AccessibilityStatusBackend, MicrophoneBackend, MicrophoneDevices, PermissionDomain,
            PermissionStatus,
        },
    };

    fn enricher(bus: &FakeBus, clock: &ManualClock) -> AtspiContextEnricher {
        AtspiContextEnricher::with_services(Arc::new(bus.clone()), Arc::new(clock.clone()))
    }

    fn identity(pid: u32, title: &str) -> WindowIdentity {
        WindowIdentity {
            pid,
            title: title.to_owned(),
        }
    }

    #[test]
    fn enricher_returns_the_snapshot_of_the_identified_window() {
        let clock = ManualClock::new();
        let bus = FakeBus::new(Arc::new(clock.clone()));
        let app = bus.add_application(":1.3", 99, "Editor");
        let window = bus.add_window(app, "Doc", true);
        bus.add_node(window, FakeNodeSpec::new("label").name("hello"));
        let enricher = enricher(&bus, &clock);

        let enriched = clock
            .run(enricher.enrich(&identity(99, "Doc")))
            .expect("matching window is enriched");
        assert_eq!(enriched.application_name.as_deref(), Some("Editor"));
        assert_eq!(enriched.snapshot.format_version, 1);
        assert_eq!(enriched.snapshot.content, "frame: Doc\n  label: hello\n");
        assert!(!enriched.snapshot.truncated);

        assert_eq!(clock.run(enricher.enrich(&identity(99, "Other"))), None);
        assert_eq!(clock.run(enricher.enrich(&identity(0, "Doc"))), None);
        assert!(clock.run(enricher.is_available()));
        bus.set_connect_fails(true);
        assert!(!clock.run(enricher.is_available()));
        assert_eq!(clock.run(enricher.enrich(&identity(99, "Doc"))), None);
    }

    #[test]
    fn enrichment_that_exceeds_its_budget_yields_none_without_blocking() {
        let clock = ManualClock::new();
        let bus = FakeBus::new(Arc::new(clock.clone()));
        let app = bus.add_application(":1.3", 99, "Editor");
        bus.add_window(app, "Doc", true);
        bus.set_connect_latency(std::time::Duration::from_secs(5));
        assert_eq!(
            clock.run(enricher(&bus, &clock).enrich(&identity(99, "Doc"))),
            None
        );
        assert_eq!(clock.elapsed(), std::time::Duration::from_millis(900));
    }

    #[test]
    fn capture_outcomes_flow_into_the_permission_service() {
        struct Capture;
        impl DesktopCaptureStatusSource for Capture {
            fn availability(&self) -> BoxFuture<'_, Result<CaptureAvailability, PermissionError>> {
                async { Ok(CaptureAvailability::Ready) }.boxed()
            }
        }
        struct Atspi;
        impl AccessibilityStatusBackend for Atspi {
            fn bus_reachable(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
                async { Ok(()) }.boxed()
            }
            fn registry_answers(&self) -> BoxFuture<'_, bool> {
                async { true }.boxed()
            }
            fn is_enabled(&self) -> BoxFuture<'_, Result<bool, PermissionError>> {
                async { Ok(true) }.boxed()
            }
            fn set_enabled(&self, _: bool) -> BoxFuture<'_, Result<(), PermissionError>> {
                async { Ok(()) }.boxed()
            }
        }
        struct Microphone;
        impl MicrophoneBackend for Microphone {
            fn sandboxed(&self) -> bool {
                false
            }
            fn devices(&self) -> BoxFuture<'_, Result<MicrophoneDevices, PermissionError>> {
                async { Ok(MicrophoneDevices::Ready) }.boxed()
            }
            fn request_device_access(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
                async { Ok(()) }.boxed()
            }
        }

        let service =
            PermissionService::new(Arc::new(Capture), Arc::new(Atspi), Arc::new(Microphone));
        let domain = PermissionDomain::DesktopCapture;
        let sink: &dyn CaptureOutcomeSink = &service;
        assert_eq!(
            futures::executor::block_on(service.status(domain)),
            PermissionStatus::Available
        );
        sink.report(CaptureOutcomeKind::Denied);
        assert_eq!(
            futures::executor::block_on(service.status(domain)),
            PermissionStatus::Denied
        );
        sink.report(CaptureOutcomeKind::Succeeded);
        assert_eq!(
            futures::executor::block_on(service.status(domain)),
            PermissionStatus::Available
        );
        assert_eq!(
            availability_of(ProbedAvailability::UserSelection),
            CaptureAvailability::UserSelection
        );
        assert_eq!(
            availability_of(ProbedAvailability::Unavailable),
            CaptureAvailability::Unavailable
        );
    }
}

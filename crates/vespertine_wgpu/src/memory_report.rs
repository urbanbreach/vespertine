//! Opt-in renderer memory accounting (DEV-002): a one-line report of the memory the renderer
//! holds, written directly to standard error at most once per ten seconds of rendering.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Environment variable enabling the diagnostic.
pub const ENV_VAR: &str = "VESPERTINE_RENDERER_MEMORY";
/// Minimum monotonic time between two reports of one renderer.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// Prefix identifying the renderer memory report line.
pub const REPORT_PREFIX: &str = "[vespertine renderer memory]";

const MIB: f64 = 1_048_576.0;

/// Whether `value` of the environment variable enables the diagnostic: any value that is
/// neither empty nor exactly `0`.
pub fn enabled_by(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// A once-read, cached enable decision (later environment changes are ignored).
#[derive(Default)]
pub struct EnableDecision(OnceLock<bool>);

impl EnableDecision {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Reads the decision on first use with `read` and returns the cached value afterwards.
    pub fn get_with(&self, read: impl FnOnce() -> Option<String>) -> bool {
        *self.0.get_or_init(|| enabled_by(read().as_deref()))
    }
}

static PROCESS_DECISION: EnableDecision = EnableDecision::new();

/// The process-wide decision, read from the environment on first inspection.
pub fn enabled() -> bool {
    PROCESS_DECISION.get_with(|| std::env::var(ENV_VAR).ok())
}

/// The component figures of one report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryFigures {
    /// Bytes the device reports allocated, when the backend exposes an allocator report.
    pub device_bytes: Option<u64>,
    /// Live atlas textures.
    pub atlas_textures: usize,
    /// Estimated atlas bytes: allocation size of every live texture (not occupied area).
    pub atlas_bytes: u64,
    /// Capacity in bytes of each reusable instance buffer.
    pub instance_capacities: Vec<u64>,
    /// Retained backdrop scratch bytes: snapshot, both filter targets and the presentation
    /// intermediate.
    pub backdrop_bytes: u64,
    /// Path intermediate bytes.
    pub path_bytes: u64,
}

/// Estimated bytes of the atlas: the sum over live textures of `width * height * bytes per
/// pixel`. Empty slots (`None`) contribute nothing.
pub fn atlas_figures(textures: &[Option<(u32, u32, u64)>]) -> (usize, u64) {
    textures.iter().flatten().fold(
        (0, 0),
        |(count, bytes), (width, height, bytes_per_pixel)| {
            (
                count + 1,
                bytes + u64::from(*width) * u64::from(*height) * bytes_per_pixel,
            )
        },
    )
}

/// Bytes of the path targets: the four-channel resolve texture plus, when present, the
/// multisample texture at its sample count. An absent target contributes zero.
pub fn path_figures(resolve: Option<(u32, u32)>, multisample: Option<(u32, u32, u32)>) -> u64 {
    let resolve = resolve.map_or(0, |(w, h)| u64::from(w) * u64::from(h) * 4);
    let multisample = multisample.map_or(0, |(w, h, samples)| {
        u64::from(w) * u64::from(h) * 4 * u64::from(samples)
    });
    resolve + multisample
}

fn mib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / MIB)
}

/// Formats the one-line report. The sum of the components is never presented as a device total.
pub fn format_report(figures: &MemoryFigures) -> String {
    let device = figures.device_bytes.map_or_else(
        || "unavailable".to_string(),
        |bytes| format!("{} MiB", mib(bytes)),
    );
    let capacities = figures
        .instance_capacities
        .iter()
        .map(|bytes| format!("{} MiB", mib(*bytes)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{REPORT_PREFIX} device {device}, atlas {} textures {} MiB, instance {} buffers [{capacities}], backdrop {} MiB, path {} MiB",
        figures.atlas_textures,
        mib(figures.atlas_bytes),
        figures.instance_capacities.len(),
        mib(figures.backdrop_bytes),
        mib(figures.path_bytes),
    )
}

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
pub type Sink = Box<dyn FnMut(&str)>;

/// Per-renderer report scheduling: the first completed frame, then at most once per interval,
/// evaluated only when a frame is rendered (no timers).
pub struct MemoryReporter {
    enabled: bool,
    clock: Clock,
    sink: Sink,
    last_report: Option<Instant>,
}

impl MemoryReporter {
    /// A reporter writing to standard error with the monotonic clock.
    pub fn new(enabled: bool) -> Self {
        Self::with_parts(
            enabled,
            Arc::new(Instant::now),
            Box::new(|line| eprintln!("{line}")),
        )
    }

    /// A reporter with an injected clock and output sink (tests).
    pub fn with_parts(enabled: bool, clock: Clock, sink: Sink) -> Self {
        Self {
            enabled,
            clock,
            sink,
            last_report: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Called after every completed encoded frame. `figures` is evaluated only when a report is
    /// due.
    pub fn frame_completed(&mut self, figures: impl FnOnce() -> MemoryFigures) {
        if !self.enabled {
            return;
        }
        let now = (self.clock)();
        let due = self
            .last_report
            .is_none_or(|last| now.saturating_duration_since(last) >= REPORT_INTERVAL);
        if !due {
            return;
        }
        self.last_report = Some(now);
        (self.sink)(&format_report(&figures()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;

    #[test]
    fn enable_values() {
        assert!(!enabled_by(None));
        assert!(!enabled_by(Some("")));
        assert!(!enabled_by(Some("0")));
        assert!(enabled_by(Some("1")));
        assert!(enabled_by(Some("true")));
        assert!(enabled_by(Some("false")));
        assert!(enabled_by(Some("00")));
    }

    #[test]
    fn the_decision_is_read_once_and_cached() {
        let decision = EnableDecision::new();
        assert!(!decision.get_with(|| None));
        // Later environment changes are ignored.
        assert!(!decision.get_with(|| Some("1".to_string())));
        let decision = EnableDecision::new();
        assert!(decision.get_with(|| Some("false".to_string())));
        assert!(decision.get_with(|| None));
    }

    #[test]
    fn atlas_estimate_counts_allocations_and_skips_empty_slots() {
        let (count, bytes) = atlas_figures(&[
            Some((1024, 1024, 1)),
            None,
            Some((1024, 1024, 4)),
            None,
            Some((512, 256, 4)),
        ]);
        assert_eq!(count, 3);
        assert_eq!(bytes, 1024 * 1024 + 4 * 1024 * 1024 + 512 * 256 * 4);
    }

    #[test]
    fn path_estimate_multiplies_the_multisample_target() {
        assert_eq!(path_figures(None, None), 0);
        assert_eq!(path_figures(Some((100, 50)), None), 100 * 50 * 4);
        assert_eq!(
            path_figures(Some((100, 50)), Some((100, 50, 4))),
            100 * 50 * 4 + 100 * 50 * 4 * 4
        );
    }

    #[test]
    fn mib_formatting_has_one_decimal() {
        assert_eq!(mib(1_572_864), "1.5");
        assert_eq!(mib(0), "0.0");
        assert_eq!(mib(2 * 1024 * 1024), "2.0");
    }

    #[test]
    fn the_report_line_lists_fields_in_order() {
        let line = format_report(&MemoryFigures {
            device_bytes: None,
            atlas_textures: 3,
            atlas_bytes: 1_572_864,
            instance_capacities: vec![2 * 1024 * 1024],
            backdrop_bytes: 0,
            path_bytes: 3_145_728,
        });
        assert_eq!(
            line,
            "[vespertine renderer memory] device unavailable, atlas 3 textures 1.5 MiB, instance 1 buffers [2.0 MiB], backdrop 0.0 MiB, path 3.0 MiB"
        );
        let line = format_report(&MemoryFigures {
            device_bytes: Some(10 * 1024 * 1024),
            ..Default::default()
        });
        assert!(line.contains("device 10.0 MiB"));
    }

    fn fake_clock() -> (Arc<Mutex<Instant>>, Clock) {
        let now = Arc::new(Mutex::new(Instant::now()));
        let handle = now.clone();
        (now, Arc::new(move || *handle.lock().unwrap()))
    }

    fn reporter(enabled: bool, clock: Clock) -> (MemoryReporter, Rc<RefCell<Vec<String>>>) {
        let lines = Rc::new(RefCell::new(Vec::new()));
        let sink = lines.clone();
        (
            MemoryReporter::with_parts(
                enabled,
                clock,
                Box::new(move |line| sink.borrow_mut().push(line.to_string())),
            ),
            lines,
        )
    }

    #[test]
    fn disabled_reporters_never_evaluate_or_emit() {
        let (_, clock) = fake_clock();
        let (mut reporter, lines) = reporter(false, clock);
        reporter.frame_completed(|| panic!("figures must not be computed"));
        assert!(lines.borrow().is_empty());
    }

    #[test]
    fn reports_on_the_first_frame_then_every_ten_seconds() {
        let (now, clock) = fake_clock();
        let (mut reporter, lines) = reporter(true, clock);
        reporter.frame_completed(MemoryFigures::default);
        assert_eq!(lines.borrow().len(), 1);
        *now.lock().unwrap() += Duration::from_millis(9_900);
        reporter.frame_completed(MemoryFigures::default);
        assert_eq!(lines.borrow().len(), 1, "9.9 s is too early");
        *now.lock().unwrap() += Duration::from_millis(100);
        reporter.frame_completed(MemoryFigures::default);
        assert_eq!(lines.borrow().len(), 2, "10.0 s reports");
        // Without frames nothing is emitted, however long the clock runs.
        *now.lock().unwrap() += Duration::from_secs(3600);
        assert_eq!(lines.borrow().len(), 2);
    }

    #[test]
    fn two_reporters_keep_independent_timers() {
        let (now, clock) = fake_clock();
        let (mut first, first_lines) = reporter(true, clock.clone());
        let (mut second, second_lines) = reporter(true, clock);
        first.frame_completed(MemoryFigures::default);
        *now.lock().unwrap() += Duration::from_secs(6);
        second.frame_completed(MemoryFigures::default);
        *now.lock().unwrap() += Duration::from_secs(5);
        first.frame_completed(MemoryFigures::default);
        second.frame_completed(MemoryFigures::default);
        assert_eq!(first_lines.borrow().len(), 2, "11 s since its first report");
        assert_eq!(second_lines.borrow().len(), 1, "5 s since its first report");
    }
}

//! The amplitude-envelope buffer (CMP-027 R1-R5): turns irregular scalar loudness samples into
//! one bounded amplitude per fixed 60 ms slot. No audio is retained.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Duration of one slot.
pub const SLOT: Duration = Duration::from_millis(60);
/// How far the display trails the clock, so a slot is filled before it scrolls into view.
pub const DISPLAY_LAG: Duration = Duration::from_millis(70);
/// The most slots kept.
pub const CAPACITY: usize = 320;
/// Decay of a skipped or quiet slot relative to the previous one.
pub const RELEASE: f32 = 0.62;

/// Loudness mapping (R1): 0 when not finite or `<= 1e-6`, else
/// `clamp((20 log10(rms) + 50) / 38, 0, 1)` (-50 dBFS is silence, -12 dBFS and louder is full).
pub fn normalize(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 1e-6 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 50.0) / 38.0).clamp(0.0, 1.0)
}

/// The recording clock text: minutes unpadded, a colon and two-digit seconds (`0:07`, `1:00`).
pub fn format_clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// One visible bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    /// Slots since the bar's slot reached the display head (fractional unless snapped).
    pub age: f32,
    /// Amplitude in `[0, 1]`.
    pub amplitude: f32,
}

#[derive(Clone, Copy, Debug)]
enum Clock {
    Stopped,
    Running(Instant),
    Frozen { start: Instant, end: Instant },
}

/// The envelope buffer.
#[derive(Clone, Debug)]
pub struct EnvelopeBuffer {
    clock: Clock,
    pending: f32,
    /// Absolute index of the first stored slot.
    first: usize,
    slots: VecDeque<f32>,
    /// Number of slots ever filled (absolute index of the next slot).
    filled: usize,
}

impl Default for EnvelopeBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvelopeBuffer {
    /// An empty, stopped buffer.
    pub fn new() -> Self {
        Self {
            clock: Clock::Stopped,
            pending: 0.0,
            first: 0,
            slots: VecDeque::new(),
            filled: 0,
        }
    }

    /// Clears the history and starts the clock at `now`.
    pub fn start(&mut self, now: Instant) {
        self.reset();
        self.clock = Clock::Running(now);
    }

    /// Forgets everything and stops the clock.
    pub fn reset(&mut self) {
        self.clock = Clock::Stopped;
        self.pending = 0.0;
        self.first = 0;
        self.slots.clear();
        self.filled = 0;
    }

    /// Stops the clock where capture ended: nothing more is recorded and elapsed time and the
    /// scroll hold still. Freezing a stopped or frozen buffer changes nothing.
    pub fn freeze(&mut self, now: Instant) {
        if let Clock::Running(start) = self.clock {
            self.clock = Clock::Frozen {
                start,
                end: now.max(start),
            };
        }
    }

    /// Whether the buffer is recording.
    pub fn is_running(&self) -> bool {
        matches!(self.clock, Clock::Running(_))
    }

    /// Whether the buffer is frozen.
    pub fn is_frozen(&self) -> bool {
        matches!(self.clock, Clock::Frozen { .. })
    }

    /// Time since start, or from start to the freeze point once frozen; zero when stopped.
    pub fn elapsed(&self, now: Instant) -> Duration {
        match self.clock {
            Clock::Stopped => Duration::ZERO,
            Clock::Running(start) => now.saturating_duration_since(start),
            Clock::Frozen { start, end } => end.saturating_duration_since(start),
        }
    }

    /// Records a raw linear loudness sample (R1 mapping applied). Returns whether a new slot
    /// was stored.
    pub fn record(&mut self, now: Instant, raw: f32) -> bool {
        self.record_normalized(now, normalize(raw))
    }

    /// Records an already normalized amplitude (clamped to `[0, 1]`).
    pub fn record_normalized(&mut self, now: Instant, amplitude: f32) -> bool {
        let Clock::Running(start) = self.clock else {
            return false;
        };
        let amplitude = if amplitude.is_finite() {
            amplitude.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.pending = self.pending.max(amplitude);
        let elapsed = now.saturating_duration_since(start);
        let due = (elapsed.as_nanos() / SLOT.as_nanos()) as usize + 1;
        if self.filled >= due {
            return false;
        }
        while self.filled < due {
            let previous = self.slots.back().copied().unwrap_or(0.0);
            self.slots.push_back(self.pending.max(previous * RELEASE));
            if self.slots.len() > CAPACITY {
                self.slots.pop_front();
                self.first += 1;
            }
            self.filled += 1;
        }
        self.pending = 0.0;
        true
    }

    /// Number of slots stored (at most [`CAPACITY`]).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether no slot is stored.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The visible head position, as a fractional slot number (R3): `(min(now, freeze) - start
    /// - 70 ms) / 60 ms`, `None` while negative.
    fn head(&self, now: Instant) -> Option<f32> {
        let elapsed = self.elapsed(now);
        let lagged = elapsed.checked_sub(DISPLAY_LAG)?;
        if matches!(self.clock, Clock::Stopped) {
            return None;
        }
        Some(lagged.as_secs_f32() / SLOT.as_secs_f32())
    }

    /// The visible bars, newest first. `continuous` keeps fractional ages; without it (reduced
    /// motion) ages are floored so the scroll snaps to whole slots.
    pub fn bars(&self, now: Instant, continuous: bool) -> Vec<Bar> {
        let Some(head) = self.head(now) else {
            return Vec::new();
        };
        let mut bars = Vec::new();
        for (offset, amplitude) in self.slots.iter().enumerate().rev() {
            let index = (self.first + offset) as f32;
            let mut age = head - index;
            if age < 0.0 {
                continue;
            }
            if !continuous {
                age = age.floor();
            }
            bars.push(Bar {
                age,
                amplitude: *amplitude,
            });
        }
        bars
    }

    /// The current level for a glow or meter (R4).
    pub fn level(&self, now: Instant) -> f32 {
        let bars = self.bars(now, true);
        match bars.as_slice() {
            [] => 0.0,
            [only] => only.amplitude * only.age.clamp(0.0, 1.0),
            [newest, previous, ..] => {
                previous.amplitude
                    + (newest.amplitude - previous.amplitude) * newest.age.clamp(0.0, 1.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a2_normalization() {
        assert_eq!(normalize(0.0), 0.0);
        assert_eq!(normalize(f32::NAN), 0.0);
        assert_eq!(normalize(f32::INFINITY), 0.0);
        assert_eq!(normalize(-1.0), 0.0);
        assert_eq!(normalize(0.003162278), 0.0); // -50 dBFS
        assert_eq!(normalize(1.0), 1.0);
        let middle = normalize(0.02);
        assert!((0.2..0.8).contains(&middle), "{middle}");
    }

    #[test]
    fn a11_clock_text() {
        assert_eq!(format_clock(Duration::from_secs_f32(7.9)), "0:07");
        assert_eq!(format_clock(Duration::from_secs(60)), "1:00");
        assert_eq!(format_clock(Duration::from_secs(605)), "10:05");
    }

    #[test]
    fn a3_irregular_polls_fill_every_due_slot() {
        let base = Instant::now();
        let mut buffer = EnvelopeBuffer::new();
        buffer.start(base);
        assert!(buffer.record(ms(base, 5), 1.0));
        assert!(!buffer.record(ms(base, 30), 0.0));
        assert!(buffer.record(ms(base, 181), 0.0));
        assert_eq!(buffer.len(), 4);
        let bars = buffer.bars(ms(base, 181 + 200), true);
        assert_eq!(bars.len(), 4);
        let oldest_first: Vec<Bar> = bars.iter().rev().copied().collect();
        for pair in oldest_first.windows(2) {
            assert!((pair[0].age - pair[1].age - 1.0).abs() < 1e-4);
        }
        let amplitudes: Vec<f32> = oldest_first.iter().map(|bar| bar.amplitude).collect();
        let expected = [1.0, RELEASE, RELEASE * RELEASE, RELEASE * RELEASE * RELEASE];
        for (actual, expected) in amplitudes.iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-5, "{amplitudes:?}");
        }
    }

    #[test]
    fn a4_a_peak_between_slots_is_kept_for_the_next_slot() {
        let base = Instant::now();
        let mut buffer = EnvelopeBuffer::new();
        buffer.start(base);
        assert!(buffer.record_normalized(ms(base, 1), 0.1));
        assert!(!buffer.record_normalized(ms(base, 20), 0.9));
        assert!(buffer.record_normalized(ms(base, 61), 0.0));
        let slots: Vec<f32> = buffer.slots.iter().copied().collect();
        assert_eq!(slots[1], 0.9);
    }

    #[test]
    fn a5_history_is_bounded_and_freezing_holds_everything_still() {
        let base = Instant::now();
        let mut buffer = EnvelopeBuffer::new();
        buffer.start(base);
        for slot in 0..640u64 {
            buffer.record_normalized(ms(base, slot * 60 + 1), 0.5);
        }
        assert_eq!(buffer.len(), CAPACITY);
        let now = ms(base, 640 * 60 + 1);
        buffer.record_normalized(now, 0.5);
        assert!(buffer.bars(now, true)[0].age < 1.0);
        buffer.freeze(now);
        let frozen_bars = buffer.bars(now, true);
        let frozen_elapsed = buffer.elapsed(now);
        let later = now + Duration::from_secs(5);
        assert!(!buffer.record_normalized(later, 1.0));
        assert_eq!(buffer.bars(later, true), frozen_bars);
        assert_eq!(buffer.elapsed(later), frozen_elapsed);
        assert_eq!(buffer.len(), CAPACITY);
    }

    #[test]
    fn a6_level_glides_and_reduced_motion_snaps_ages() {
        let base = Instant::now();
        let mut buffer = EnvelopeBuffer::new();
        buffer.start(base);
        buffer.record_normalized(ms(base, 1), 0.2);
        buffer.record_normalized(ms(base, 61), 1.0);
        // The newest slot (index 1) reaches the display head at 60 + 70 ms.
        let level = |at: u64| buffer.level(ms(base, at));
        assert!((level(130) - 0.2).abs() < 0.02, "{}", level(130));
        assert!((level(160) - 0.6).abs() < 0.02, "{}", level(160));
        assert!((level(190) - 1.0).abs() < 0.02, "{}", level(190));
        assert!((level(500) - 1.0).abs() < 1e-5);
        let snapped = buffer.bars(ms(base, 175), false);
        assert!(snapped.iter().all(|bar| bar.age.fract() == 0.0));
        // No bars before the lag has passed; level is 0 with no bars.
        assert!(buffer.bars(ms(base, 69), true).is_empty());
        assert_eq!(buffer.level(ms(base, 69)), 0.0);
    }

    #[test]
    fn a_stopped_buffer_records_nothing() {
        let base = Instant::now();
        let mut buffer = EnvelopeBuffer::new();
        assert!(!buffer.record(base, 1.0));
        buffer.start(base);
        buffer.record(base, 1.0);
        buffer.reset();
        assert!(buffer.is_empty());
        assert_eq!(
            buffer.elapsed(base + Duration::from_secs(1)),
            Duration::ZERO
        );
    }
}

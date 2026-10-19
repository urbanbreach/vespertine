//! Lockstep verification (DEV-003 R6): a retained window and a full-render window are drawn
//! through identical steps and compared every frame.
//!
//! The checks compare the expanded scene primitives (paint order, clipping, geometry, color and
//! text layout, with what each sprite shows resolved from the atlas so packing identities are
//! ignored), hit-test results, and the positions observed by listeners. Accessibility-tree and
//! GPU pixel checks are available as opt-in checks; anything else plugs in by implementing
//! [`FrameCheck`] and adding it with [`Lockstep::with_check`] (PRF-004's direct-versus-tile
//! pixel comparison is such a check).

use crate::harness::{FaultKind, Harness, RetentionMode, WINDOW_SIZE};
use crate::metrics::Meter;
use crate::runner::RunOptions;
use crate::workload::Workload;
use anyhow::{Result, anyhow};
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::Arc,
};
use vespertine::{PlatformTextSystem, point, px};

/// The outcome of one check on one frame.
pub struct Comparison {
    /// A digest of what the retained window showed, for reproducibility checks.
    pub digest: u64,
    /// A description of the first difference, if the windows differ.
    pub mismatch: Option<String>,
}

impl Comparison {
    /// Compares two observations given as text, line by line.
    pub fn of_texts(retained: &str, full: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        retained.hash(&mut hasher);
        let mismatch = (retained != full).then(|| {
            let mut retained_lines = retained.lines();
            let mut full_lines = full.lines();
            let mut line = 0;
            loop {
                match (retained_lines.next(), full_lines.next()) {
                    (Some(a), Some(b)) if a == b => line += 1,
                    (a, b) => {
                        break format!(
                            "first difference at entry {line}:\n  retained: {}\n  full:     {}",
                            a.unwrap_or("<missing>"),
                            b.unwrap_or("<missing>")
                        );
                    }
                }
            }
        });
        Self {
            digest: hasher.finish(),
            mismatch,
        }
    }
}

/// One comparison between the retained and the full-render window, made after both drew the same
/// frame.
pub trait FrameCheck {
    /// Name used in reports.
    fn name(&self) -> &'static str;

    /// Compares the frame both windows just drew.
    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison>;
}

/// The expanded scene primitives of the last frame. Scroll layers (PRF-004) are expanded into
/// the primitives their tiles bake; primitives that do not overlap may differ in position
/// (tiles bake them in another order than direct drawing paints them), every two overlapping
/// ones keep their paint order.
pub struct PrimitivesCheck;

impl FrameCheck for PrimitivesCheck {
    fn name(&self) -> &'static str {
        "scene primitives"
    }

    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison> {
        let content =
            |harness: &mut Harness| harness.read_window(|window, _| window.painted_content());
        let (shown, expected) = (content(retained)?, content(full)?);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (text, _) in &expected {
            text.hash(&mut hasher);
        }
        Ok(Comparison {
            digest: hasher.finish(),
            mismatch: vespertine::painted_content_agrees(&expected, &shown).err(),
        })
    }
}

/// Hit-test results on a grid over the window.
pub struct HitTestCheck;

impl FrameCheck for HitTestCheck {
    fn name(&self) -> &'static str {
        "hit tests"
    }

    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison> {
        let hits = |harness: &mut Harness| {
            harness.read_window(|window, _| {
                let mut out = String::new();
                let (width, height) = WINDOW_SIZE;
                let mut y = 8.0;
                while y < height {
                    let mut x = 8.0;
                    while x < width {
                        let bounds = window.hit_test_bounds(point(px(x), px(y)));
                        out.push_str(&format!("{x},{y}: {bounds:?}\n"));
                        x += 88.0;
                    }
                    y += 48.0;
                }
                out
            })
        };
        Ok(Comparison::of_texts(&hits(retained)?, &hits(full)?))
    }
}

/// The positions listeners observed while the frame's step ran.
pub struct ListenerCheck;

impl FrameCheck for ListenerCheck {
    fn name(&self) -> &'static str {
        "listener positions"
    }

    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison> {
        let drain = |harness: &mut Harness| {
            harness
                .log()
                .borrow_mut()
                .drain(..)
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(Comparison::of_texts(&drain(retained), &drain(full)))
    }
}

/// The accessibility tree both windows deliver to an assistive technology (A11Y-002). Opt-in:
/// it connects a simulated assistive technology to both windows, which forces accessibility
/// frames.
pub struct AccessibilityCheck {
    connected: bool,
}

impl AccessibilityCheck {
    /// A check that connects the assistive technology on first use.
    pub fn new() -> Self {
        Self { connected: false }
    }
}

impl Default for AccessibilityCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCheck for AccessibilityCheck {
    fn name(&self) -> &'static str {
        "accessibility tree"
    }

    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison> {
        let tree = |harness: &mut Harness, connect: bool| -> Result<String> {
            let window = harness.window_handle()?;
            if connect && !harness.app().simulate_accessibility_activation(window)? {
                return Err(anyhow!(
                    "the window registered no accessibility adapter, so its tree cannot be compared"
                ));
            }
            let update = harness.app().last_accessibility_update(window)?;
            Ok(format!("{update:#?}"))
        };
        let connect = !self.connected;
        let (retained, full) = (tree(retained, connect)?, tree(full, connect)?);
        self.connected = true;
        Ok(Comparison::of_texts(&retained, &full))
    }
}

/// The GPU-rendered pixels of both windows. Needs windows opened with a renderer
/// ([`LockstepOptions::pixels`]); one differing channel value fails the frame.
pub struct PixelCheck;

impl FrameCheck for PixelCheck {
    fn name(&self) -> &'static str {
        "pixels"
    }

    fn compare(&mut self, retained: &mut Harness, full: &mut Harness) -> Result<Comparison> {
        let (a, b) = (retained.screenshot()?, full.screenshot()?);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        a.as_raw().hash(&mut hasher);
        let mismatch = if a.dimensions() != b.dimensions() {
            Some(format!(
                "sizes differ: {:?} against {:?}",
                a.dimensions(),
                b.dimensions()
            ))
        } else {
            a.enumerate_pixels()
                .zip(b.pixels())
                .find(|((_, _, x), y)| x != y)
                .map(|((px_x, px_y, x), y)| {
                    format!(
                        "pixel ({px_x}, {px_y}) is {:?} retained and {:?} full",
                        x.0, y.0
                    )
                })
        };
        Ok(Comparison {
            digest: hasher.finish(),
            mismatch,
        })
    }
}

/// A fault to place in the retained window's probe at one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultPlan {
    /// The frame whose step shows the fault.
    pub frame: u64,
    /// The one-pixel difference.
    pub kind: FaultKind,
}

impl FaultPlan {
    /// A fault whose frame (in the second half of `frames`) and kind follow from `seed`.
    pub fn seeded(seed: u64, frames: u64) -> Self {
        let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        let half = (frames / 2).max(1);
        Self {
            frame: frames - half + next() % half,
            kind: FaultKind::ALL[(next() % FaultKind::ALL.len() as u64) as usize],
        }
    }
}

/// How to verify.
#[derive(Clone, Debug, Default)]
pub struct LockstepOptions {
    /// Steps (warm-up and measured alike) to draw and compare; `None` runs warm-up plus the
    /// workload's measured frames.
    pub frames: Option<u64>,
    /// Also compare accessibility trees.
    pub accessibility: bool,
    /// Also compare GPU-rendered pixels (needs a GPU test backend; fails loudly without one).
    pub pixels: bool,
    /// Place a seeded one-pixel fault in the retained window.
    pub fault: Option<FaultPlan>,
    /// Give the retained window a different atlas packing history than the full-render window
    /// before the run; the comparison must not notice.
    pub perturb_atlas: bool,
}

/// The first frame whose windows differed.
#[derive(Clone, Debug)]
pub struct Mismatch {
    /// The frame (absolute step index).
    pub frame: u64,
    /// The check that failed.
    pub check: &'static str,
    /// What differed.
    pub detail: String,
}

/// The result of verifying one workload.
#[derive(Clone, Debug)]
pub struct VerifyReport {
    /// Workload name.
    pub workload: &'static str,
    /// Frames compared (all of them unless a mismatch stopped the run).
    pub frames_compared: u64,
    /// The checks that ran, in order.
    pub checks: Vec<&'static str>,
    /// The first mismatch, or `None` when the windows agreed on every frame.
    pub mismatch: Option<Mismatch>,
    /// A digest of everything the retained window showed on every compared frame.
    pub digest: u64,
    /// Natural draws of the retained window over the run.
    pub natural_draws: u64,
    /// Forced draws of the retained window over the run.
    pub forced_draws: u64,
}

impl VerifyReport {
    /// Whether the windows agreed on every frame.
    pub fn passed(&self) -> bool {
        self.mismatch.is_none()
    }
}

/// A lockstep run: the checks to make every frame.
pub struct Lockstep {
    checks: Vec<Box<dyn FrameCheck>>,
}

impl Lockstep {
    /// The checks that always run: primitives, hit tests and listener positions.
    pub fn new() -> Self {
        Self {
            checks: vec![
                Box::new(PrimitivesCheck),
                Box::new(HitTestCheck),
                Box::new(ListenerCheck),
            ],
        }
    }

    /// No built-in checks: only what [`Self::with_check`] and the options add.
    pub fn empty() -> Self {
        Self { checks: Vec::new() }
    }

    /// Adds a check (accessibility, pixels, or anything else that implements [`FrameCheck`]).
    pub fn with_check(mut self, check: Box<dyn FrameCheck>) -> Self {
        self.checks.push(check);
        self
    }

    /// Draws `workload` in a retained and a full-render window, from fresh state, through the
    /// same steps, comparing every frame.
    pub fn run(
        mut self,
        text_system: &Arc<dyn PlatformTextSystem>,
        workload: &Workload,
        options: &LockstepOptions,
        run: &RunOptions,
    ) -> Result<VerifyReport> {
        let meter = Rc::new(RefCell::new(Meter::with_instructions(
            crate::metrics::Instructions::Unavailable("not measured while verifying".to_owned()),
        )));
        let frames = options.frames.unwrap_or_else(|| {
            run.warmup.unwrap_or(workload.warmup) + run.frames.unwrap_or(workload.frames)
        });
        if options.accessibility {
            self.checks.push(Box::new(AccessibilityCheck::new()));
        }
        if options.pixels {
            self.checks.push(Box::new(PixelCheck));
        }
        let open = |mode: RetentionMode| -> Result<(Harness, Box<dyn crate::harness::Driver>)> {
            let mut harness =
                Harness::with_renderer(text_system.clone(), meter.clone(), true, options.pixels);
            if options.perturb_atlas && mode == RetentionMode::On {
                harness.perturb_atlas_on_open();
            }
            let driver = (workload.setup)(&mut harness)?;
            harness.set_retention(mode)?;
            harness.settle()?;
            Ok((harness, driver))
        };
        let (mut retained, mut retained_driver) = open(RetentionMode::On)?;
        let (mut full, mut full_driver) = open(RetentionMode::Off)?;

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let (mut natural, mut forced) = (0, 0);
        let mut mismatch = None;
        let mut compared = 0;
        'frames: for frame in 0..frames {
            if let Some(fault) = options.fault.filter(|fault| fault.frame == frame) {
                retained.inject_fault(Some(fault.kind))?;
            }
            let outcome = retained.run_frame(frame, &mut *retained_driver)?;
            full.run_frame(frame, &mut *full_driver)?;
            natural += u64::from(outcome.natural_draws);
            forced += u64::from(outcome.forced_draws);
            compared += 1;
            for check in &mut self.checks {
                let comparison = check.compare(&mut retained, &mut full)?;
                comparison.digest.hash(&mut hasher);
                if let Some(detail) = comparison.mismatch {
                    mismatch = Some(Mismatch {
                        frame,
                        check: check.name(),
                        detail,
                    });
                    break 'frames;
                }
            }
        }
        Ok(VerifyReport {
            workload: workload.name,
            frames_compared: compared,
            checks: self.checks.iter().map(|check| check.name()).collect(),
            mismatch,
            digest: hasher.finish(),
            natural_draws: natural,
            forced_draws: forced,
        })
    }
}

impl Default for Lockstep {
    fn default() -> Self {
        Self::new()
    }
}

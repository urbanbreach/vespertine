//! The headless window a workload runs in: a real [`Window`] on the deterministic test platform
//! with real text shaping, driven one step at a time (DEV-003 R2).

use crate::metrics::{Cost, Meter};
use anyhow::{Result, anyhow};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};
use vespertine::{
    AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, HeadlessAppContext,
    IntoElement, ParentElement as _, PlatformInput, PlatformTextSystem, Render,
    RequestFrameOptions, Styled as _, Window, div, px, rgb, size,
};

/// Normalized reproduction: logical window size.
pub const WINDOW_SIZE: (f32, f32) = (880.0, 480.0);
/// Normalized reproduction: scale factor.
pub const SCALE_FACTOR: f32 = 2.0;
/// Normalized reproduction: refresh rate.
pub const REFRESH_HZ: f32 = 60.0;
/// The most natural draws one step may take before the window is declared not to park.
const MAX_DRAWS_PER_STEP: u32 = 16;

/// Positions and other observations made by input listeners, one entry per observation. Lockstep
/// verification compares the log of every frame between the two windows.
pub type EventLog = Rc<RefCell<Vec<String>>>;

/// The text system with real shaping used by every headless run.
pub fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        vespertine_platform::current_platform(true).text_system()
    }
}

/// How a window draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionMode {
    /// Retained frames on (the default behavior).
    On,
    /// Retention off: every frame constructs every view.
    Off,
}

impl RetentionMode {
    /// Label used in reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "retention-on",
            Self::Off => "retention-off",
        }
    }
}

/// A deliberate one-device-pixel difference placed in the verification probe, to prove that
/// lockstep verification fails on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// The probe is one device pixel off its place.
    Geometry,
    /// One color channel of the probe differs by one unit.
    Color,
    /// Two overlapping probe quads are painted in the other order.
    Order,
}

impl FaultKind {
    /// Every fault kind.
    pub const ALL: [Self; 3] = [Self::Geometry, Self::Color, Self::Order];
}

/// A small element at the bottom-right corner of the window, present only when a window is
/// opened for verification. It paints two overlapping squares; a [`FaultKind`] perturbs them.
struct Probe {
    fault: Option<FaultKind>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // 0.5 logical px is one device pixel at the normalized scale factor of 2.
        let offset = if self.fault == Some(FaultKind::Geometry) {
            3.5
        } else {
            3.0
        };
        let blue = if self.fault == Some(FaultKind::Color) {
            0x66
        } else {
            0x67
        };
        let square = |color: u32, inset: f32| {
            div()
                .absolute()
                .right(px(offset + inset))
                .bottom(px(3.0 + inset))
                .size(px(6.0))
                .bg(rgb(color))
        };
        let first = square(0x336600 | blue, 0.0);
        let second = square(0xcc3300, 3.0);
        let order_fault = self.fault == Some(FaultKind::Order);
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .children(if order_fault {
                [second, first]
            } else {
                [first, second]
            })
    }
}

struct HarnessRoot {
    content: AnyView,
    probe: Option<Entity<Probe>>,
    /// Text shown instead of the content while the atlas is being disturbed.
    noise: Option<&'static str>,
}

impl Render for HarnessRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(noise) = self.noise {
            return div()
                .size_full()
                .text_size(px(21.0))
                .child(noise)
                .into_any_element();
        }
        div()
            .size_full()
            .relative()
            .child(self.content.clone())
            .children(self.probe.clone())
            .into_any_element()
    }
}

/// Glyphs no workload shows, drawn first when the atlas is perturbed.
const ATLAS_NOISE: &str = "Qxz jwv KZX 0123456789 {}[]<> ~^|";

/// What one step cost and how many draws it took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOutcome {
    /// The complete resulting frame (clock tick, step side effects, observers, subscriptions,
    /// tasks, every draw) with the step's own cost subtracted.
    pub frame: Cost,
    /// The model mutation the step made through [`Stage::mutate`].
    pub step: Cost,
    /// Draws the window made because the step (or its consequences) invalidated it.
    pub natural_draws: u32,
    /// Draws the runner forced because the step left the window clean (zero or one).
    pub forced_draws: u32,
}

/// The surface a [`crate::Driver`] applies one step through.
pub struct Stage<'a> {
    app: &'a mut App,
    window: AnyWindowHandle,
    meter: &'a RefCell<Meter>,
    step_cost: &'a mut Cost,
    log: &'a EventLog,
}

impl<'a> Stage<'a> {
    /// A stage over `app` for one step: `meter` times mutations, whose cost accumulates into
    /// `step_cost`; `log` receives listener observations.
    pub fn new(
        app: &'a mut App,
        window: AnyWindowHandle,
        meter: &'a RefCell<Meter>,
        step_cost: &'a mut Cost,
        log: &'a EventLog,
    ) -> Self {
        Self {
            app,
            window,
            meter,
            step_cost,
            log,
        }
    }

    /// Applies a model mutation. Its own CPU time, wall time and allocations are measured and
    /// subtracted from the frame; what it triggers afterwards (observers, subscriptions,
    /// re-rendering) is charged to the frame.
    pub fn mutate<R>(&mut self, mutation: impl FnOnce(&mut App) -> R) -> R {
        let start = self.meter.borrow_mut().now();
        let result = mutation(self.app);
        let end = self.meter.borrow_mut().now();
        *self.step_cost += end.since(&start);
        result
    }

    /// Delivers a platform input event to the window the way the platform would. Listener work
    /// is the frame's, not the step's.
    pub fn dispatch(&mut self, event: PlatformInput) {
        let window = self.window;
        self.app
            .update_window(window, |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("the benchmark window is open");
    }

    /// The application, for reads (a read is not a step; mutate through [`Self::mutate`]).
    pub fn app(&mut self) -> &mut App {
        self.app
    }

    /// The benchmark window.
    pub fn window(&self) -> AnyWindowHandle {
        self.window
    }

    /// The log input listeners write their observations to.
    pub fn log(&self) -> &EventLog {
        self.log
    }
}

/// A scripted application step: the model or input change of one frame, a pure function of the
/// absolute frame index (warm-up frames included).
pub trait Driver {
    /// Applies the step of `frame`.
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>);
}

/// Where a workload builds its window: the headless [`Harness`].
pub trait Host {
    /// Opens the benchmark window around the content `build` makes, at the normalized size.
    fn open_boxed(
        &mut self,
        build: Box<dyn FnOnce(&mut Window, &mut App) -> AnyView + '_>,
    ) -> Result<AnyWindowHandle>;

    /// Runs `f` with the application (to install globals before the window opens).
    fn with_app(&mut self, f: &mut dyn FnMut(&mut App));

    /// Runs `f` with the benchmark window.
    fn with_window(&mut self, f: &mut dyn FnMut(&mut Window, &mut App)) -> Result<()>;

    /// The log input listeners write their observations to.
    fn event_log(&self) -> EventLog;

    /// Lets the window lay out and draw until parked, without measuring.
    fn settle_window(&mut self) -> Result<()>;
}

impl dyn Host + '_ {
    /// Opens the benchmark window at the normalized size around `content`.
    pub fn open_window(
        &mut self,
        content: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) -> Result<AnyWindowHandle> {
        self.open_boxed(Box::new(content))
    }

    /// Reads or updates the benchmark window.
    pub fn read_window<R>(&mut self, read: impl FnOnce(&mut Window, &mut App) -> R) -> Result<R> {
        let mut read = Some(read);
        let mut result = None;
        self.with_window(&mut |window, cx| {
            result = read.take().map(|read| read(window, cx));
        })?;
        result.ok_or_else(|| anyhow!("the window callback did not run"))
    }

    /// Updates the application.
    pub fn update_app(&mut self, update: impl FnOnce(&mut App)) {
        let mut update = Some(update);
        self.with_app(&mut |cx| {
            if let Some(update) = update.take() {
                update(cx);
            }
        });
    }

    /// The log input listeners write their observations to.
    pub fn log(&self) -> EventLog {
        self.event_log()
    }

    /// Lets the window lay out and draw until parked, without measuring.
    pub fn settle(&mut self) -> Result<()> {
        self.settle_window()
    }
}

impl Host for Harness {
    fn open_boxed(
        &mut self,
        build: Box<dyn FnOnce(&mut Window, &mut App) -> AnyView + '_>,
    ) -> Result<AnyWindowHandle> {
        self.open_window_sized(WINDOW_SIZE, build)
    }

    fn with_app(&mut self, f: &mut dyn FnMut(&mut App)) {
        self.cx.update(|cx| f(cx));
    }

    fn with_window(&mut self, f: &mut dyn FnMut(&mut Window, &mut App)) -> Result<()> {
        self.read_window(|window, cx| f(window, cx))
    }

    fn event_log(&self) -> EventLog {
        self.log.clone()
    }

    fn settle_window(&mut self) -> Result<()> {
        self.settle().map(|_| ())
    }
}

/// A headless application with one benchmark window.
pub struct Harness {
    cx: HeadlessAppContext,
    window: Option<AnyWindowHandle>,
    meter: Rc<RefCell<Meter>>,
    log: EventLog,
    probe: Option<Entity<Probe>>,
    root: Option<Entity<HarnessRoot>>,
    atlas_noise: bool,
    verifying: bool,
}

impl Harness {
    /// A fresh application sharing `text_system`. With `verifying`, windows carry the
    /// verification probe.
    pub fn new(
        text_system: Arc<dyn PlatformTextSystem>,
        meter: Rc<RefCell<Meter>>,
        verifying: bool,
    ) -> Self {
        Self::with_renderer(text_system, meter, verifying, false)
    }

    /// [`Self::new`], optionally with the platform's GPU test renderer so that
    /// [`Self::screenshot`] works. Measured runs never use a renderer.
    pub fn with_renderer(
        text_system: Arc<dyn PlatformTextSystem>,
        meter: Rc<RefCell<Meter>>,
        verifying: bool,
        renderer: bool,
    ) -> Self {
        let factory = move || {
            if renderer {
                vespertine_platform::current_headless_renderer()
            } else {
                Ok(None)
            }
        };
        Self {
            cx: HeadlessAppContext::with_platform(text_system, Arc::new(()), factory),
            window: None,
            meter,
            log: EventLog::default(),
            probe: None,
            root: None,
            atlas_noise: false,
            verifying,
        }
    }

    /// The benchmark window.
    pub fn window_handle(&self) -> Result<AnyWindowHandle> {
        self.window()
    }

    /// The listener log (clone it into elements that observe input).
    pub fn log(&self) -> EventLog {
        self.log.clone()
    }

    /// The application, for building models before the window opens.
    pub fn app(&mut self) -> &mut HeadlessAppContext {
        &mut self.cx
    }

    /// Opens the benchmark window at the normalized size and scale factor around `content`,
    /// and draws its first frame.
    pub fn open_window(
        &mut self,
        content: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) -> Result<AnyWindowHandle> {
        self.open_window_sized(WINDOW_SIZE, content)
    }

    /// [`Self::open_window`] with an explicit logical size.
    pub fn open_window_sized(
        &mut self,
        (width, height): (f32, f32),
        content: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) -> Result<AnyWindowHandle> {
        if self.window.is_some() {
            return Err(anyhow!("the benchmark window is already open"));
        }
        let verifying = self.verifying;
        let atlas_noise = self.atlas_noise;
        let mut probe = None;
        let mut root_entity = None;
        let handle = self
            .cx
            .open_window(size(px(width), px(height)), |window, cx| {
                let content = content(window, cx);
                let probe_entity = verifying.then(|| cx.new(|_| Probe { fault: None }));
                probe = probe_entity.clone();
                let root = cx.new(|_| HarnessRoot {
                    content,
                    probe: probe_entity,
                    noise: atlas_noise.then_some(ATLAS_NOISE),
                });
                root_entity = Some(root.clone());
                root
            })?;
        let handle: AnyWindowHandle = handle.into();
        self.cx.simulate_window_scale_factor(handle, SCALE_FACTOR)?;
        self.probe = probe;
        self.root = root_entity;
        self.window = Some(handle);
        self.settle()?;
        if atlas_noise && let Some(root) = self.root.clone() {
            self.cx.update(|cx| {
                root.update(cx, |root, cx| {
                    root.noise = None;
                    cx.notify();
                })
            });
            self.settle()?;
        }
        Ok(handle)
    }

    fn window(&self) -> Result<AnyWindowHandle> {
        self.window
            .ok_or_else(|| anyhow!("the benchmark window is not open"))
    }

    /// Enables or disables retained frames for the benchmark window.
    pub fn set_retention(&mut self, mode: RetentionMode) -> Result<()> {
        let window = self.window()?;
        self.cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(mode == RetentionMode::On)
        })
    }

    /// The GPU-rendered pixels of the last drawn frame (requires [`Self::with_renderer`] with a
    /// working GPU test backend).
    pub fn screenshot(&mut self) -> Result<image::RgbaImage> {
        let window = self.window()?;
        self.cx.capture_screenshot(window)
    }

    /// Reads the window.
    pub fn read_window<R>(&mut self, read: impl FnOnce(&mut Window, &mut App) -> R) -> Result<R> {
        let window = self.window()?;
        self.cx
            .update_window(window, |_, window, cx| read(window, cx))
    }

    /// Makes the next window draw glyphs the workload never shows before its content, so that
    /// every tile the workload then allocates gets a different identity than it would in a
    /// fresh window. What is painted afterwards is unchanged; only the packing history differs.
    pub fn perturb_atlas_on_open(&mut self) {
        self.atlas_noise = true;
    }

    /// Makes the verification probe show `fault` (or none). Requires a verifying harness.
    pub fn inject_fault(&mut self, fault: Option<FaultKind>) -> Result<()> {
        let probe = self
            .probe
            .clone()
            .ok_or_else(|| anyhow!("fault injection needs a harness opened for verification"))?;
        self.cx.update(|cx| {
            probe.update(cx, |probe, cx| {
                probe.fault = fault;
                cx.notify();
            })
        });
        Ok(())
    }

    /// Drains tasks and effects and draws until the window is parked, without measuring.
    pub fn settle(&mut self) -> Result<u32> {
        let before = self.window_draws()?;
        let (natural, _) = self.drain_and_draw(false, before)?;
        Ok(natural)
    }

    /// Applies one step and draws its frame, measuring as [`FrameOutcome`] describes. Exactly
    /// the draws needed to park the window are made; when the step leaves the window clean one
    /// draw is forced and reported separately.
    pub fn run_frame(&mut self, frame: u64, driver: &mut dyn Driver) -> Result<FrameOutcome> {
        let window = self.window()?;
        let meter = self.meter.clone();
        let log = self.log.clone();
        let draws_before = self.window_draws()?;
        let start = meter.borrow_mut().now();
        let mut step_cost = Cost {
            instructions: meter
                .borrow()
                .instructions_unavailable()
                .is_none()
                .then_some(0),
            ..Cost::default()
        };
        self.cx
            .advance_clock(Duration::from_secs_f64(1.0 / f64::from(REFRESH_HZ)));
        self.cx.update(|app| {
            driver.step(
                frame,
                &mut Stage {
                    app,
                    window,
                    meter: &meter,
                    step_cost: &mut step_cost,
                    log: &log,
                },
            );
        });
        let (natural_draws, forced_draws) = self.drain_and_draw(true, draws_before)?;
        let end = meter.borrow_mut().now();
        Ok(FrameOutcome {
            frame: end.since(&start) - step_cost,
            step: step_cost,
            natural_draws,
            forced_draws,
        })
    }

    /// Runs tasks until parked, drawing while the window is invalid. The platform delivers
    /// frame requests as the executor runs, so most draws happen inside `run_until_parked`; the
    /// window's own draw count is the account of natural draws. With `force`, one draw is forced
    /// when the step produced none. `draws_before` is the window's draw count before the step.
    /// Returns (natural, forced) draw counts.
    fn drain_and_draw(&mut self, force: bool, draws_before: u64) -> Result<(u32, u32)> {
        let window = self.window()?;
        let mut requested = 0;
        loop {
            self.cx.run_until_parked();
            if !self.cx.window_needs_draw(window)? {
                break;
            }
            if requested >= MAX_DRAWS_PER_STEP {
                return Err(anyhow!(
                    "the window did not park after {MAX_DRAWS_PER_STEP} draws of one step"
                ));
            }
            self.cx
                .request_frame(window, RequestFrameOptions::default())?;
            requested += 1;
        }
        let natural = self.window_draws()? - draws_before;
        let mut forced = 0;
        if force && natural == 0 {
            self.cx
                .update_window(window, |_, window, cx| window.draw(cx).clear(cx))?;
            forced = 1;
            // A forced draw that invalidates the window again leads to natural draws.
            let after_forced = self.window_draws()?;
            let (more, _) = self.drain_and_draw(false, after_forced)?;
            return Ok((more, forced));
        }
        Ok((natural as u32, forced))
    }

    fn window_draws(&mut self) -> Result<u64> {
        self.read_window(|window, _| window.frame_stats().draws)
    }

    /// The frame statistics the window accumulated.
    pub fn frame_stats(&mut self) -> Result<vespertine::FrameStats> {
        self.read_window(|window, _| window.frame_stats())
    }

    /// Resets the window's frame statistics and sets whether phase timers run.
    pub fn reset_frame_stats(&mut self, timing: bool) -> Result<()> {
        self.read_window(|window, _| {
            window.reset_frame_stats();
            window.set_frame_timing_enabled(timing);
        })
    }
}

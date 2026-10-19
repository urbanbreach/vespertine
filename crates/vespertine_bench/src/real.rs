//! Automatic real-window mode (DEV-003 R2/R7): the same workloads and steps, in a real window on
//! the desktop session, drawn by the platform's own frame callbacks at the display's rate.
//!
//! Wall time here includes waiting for the display, so the figure to read is the UI thread's CPU
//! time per frame; allocations are process-wide and include the renderer's threads. Scenarios
//! run one after another in one application, each with a fresh window and fresh models.

use crate::harness::FrameOutcome;
use crate::harness::{Driver, EventLog, Host, REFRESH_HZ, RetentionMode, Stage, WINDOW_SIZE};
use crate::metrics::{Cost, Meter};
use crate::platform_report::{PlatformReport, RunKind};
use crate::report;
use crate::runner::{FrameSample, RunOptions, ScenarioReport};
use crate::workload::Workload;
use anyhow::{Result, anyhow};
use std::{cell::RefCell, process::ExitCode, rc::Rc, time::Duration};
use vespertine::{
    AnyView, AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, IntoElement,
    ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div, point, px,
    size,
};

struct RealRoot {
    content: AnyView,
}

impl Render for RealRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.content.clone())
    }
}

struct RealHost<'a> {
    app: &'a mut App,
    window: Option<AnyWindowHandle>,
    log: EventLog,
}

impl Host for RealHost<'_> {
    fn open_boxed(
        &mut self,
        build: Box<dyn FnOnce(&mut Window, &mut App) -> AnyView + '_>,
    ) -> Result<AnyWindowHandle> {
        let (width, height) = WINDOW_SIZE;
        let bounds = Bounds::new(point(px(40.0), px(40.0)), size(px(width), px(height)));
        let handle = self.app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..WindowOptions::default()
            },
            |window, cx| {
                let content = build(window, cx);
                cx.new(|_| RealRoot { content })
            },
        )?;
        let handle: AnyWindowHandle = handle.into();
        self.window = Some(handle);
        Ok(handle)
    }

    fn with_app(&mut self, f: &mut dyn FnMut(&mut App)) {
        f(self.app);
    }

    fn with_window(&mut self, f: &mut dyn FnMut(&mut Window, &mut App)) -> Result<()> {
        let window = self
            .window
            .ok_or_else(|| anyhow!("the benchmark window is not open"))?;
        self.app
            .update_window(window, |_, window, cx| f(window, cx))
    }

    fn event_log(&self) -> EventLog {
        self.log.clone()
    }

    fn settle_window(&mut self) -> Result<()> {
        // A real window draws on the display's own frame callbacks; the runner waits for them.
        Ok(())
    }
}

/// Runs `workloads` in real windows, one scenario after another.
pub fn run(
    workloads: &[Workload],
    options: &RunOptions,
    modes: &[RetentionMode],
    json: bool,
) -> Result<ExitCode> {
    let workloads = workloads.to_vec();
    let options = options.clone();
    let modes = modes.to_vec();
    let failure: Rc<RefCell<Option<String>>> = Rc::default();
    let failure_out = failure.clone();
    vespertine_platform::application().run(move |cx: &mut App| {
        let failure = failure_out;
        cx.spawn(async move |cx| {
            if let Err(error) = run_all(cx, &workloads, &options, &modes, json).await {
                *failure.borrow_mut() = Some(format!("{error:#}"));
            }
            cx.update(|app| app.quit());
        })
        .detach();
    });
    match failure.borrow().as_deref() {
        None => Ok(ExitCode::SUCCESS),
        Some(message) => Err(anyhow!("{message}")),
    }
}

async fn run_all(
    cx: &mut AsyncApp,
    workloads: &[Workload],
    options: &RunOptions,
    modes: &[RetentionMode],
    json: bool,
) -> Result<()> {
    let meter = Rc::new(RefCell::new(Meter::new()));
    let mut results = Vec::new();
    let mut platform_printed = false;
    for workload in workloads {
        for &mode in modes {
            let report = run_one(
                cx,
                &meter,
                workload,
                mode,
                options,
                &mut platform_printed,
                json,
            )
            .await?;
            if json {
                results.push(report::scenario_json(&report));
            } else {
                println!("{}", report::scenario_text(&report));
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }
    Ok(())
}

async fn run_one(
    cx: &mut AsyncApp,
    meter: &Rc<RefCell<Meter>>,
    workload: &Workload,
    mode: RetentionMode,
    options: &RunOptions,
    platform_printed: &mut bool,
    json: bool,
) -> Result<ScenarioReport> {
    let warmup = options.warmup.unwrap_or(workload.warmup);
    let frames = options.frames.unwrap_or(workload.frames);
    let log = EventLog::default();
    let (mut driver, window, display) = cx.update(|app| -> Result<_> {
        let mut host = RealHost {
            app,
            window: None,
            log: log.clone(),
        };
        let driver: Box<dyn Driver> = (workload.setup)(&mut host)?;
        let window = host
            .window
            .ok_or_else(|| anyhow!("the workload opened no window"))?;
        let facts = app.update_window(window, |_, window, cx| {
            window.set_retention_enabled(mode == RetentionMode::On);
            (
                window.scale_factor(),
                window
                    .refresh_interval(cx)
                    .map(|interval| 1.0 / interval.as_secs_f64()),
            )
        })?;
        Ok((driver, window, facts))
    })?;
    if !*platform_printed && !json {
        *platform_printed = true;
        let mut platform = PlatformReport::collect(
            RunKind::RealWindow,
            meter
                .borrow()
                .instructions_unavailable()
                .map_or(Ok(()), Err),
        );
        platform.scale_factor = display.0;
        platform.refresh = display.1.map_or_else(
            || "not reported by the display".to_owned(),
            |hz| format!("{hz:.1} Hz (display)"),
        );
        print!("{}", platform.text());
    }
    let interval = Duration::from_secs_f64(1.0 / f64::from(REFRESH_HZ));
    // Let the window map and draw its first frames.
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;

    let mut samples = Vec::with_capacity(frames as usize);
    let (mut natural_draws, mut forced_draws) = (0, 0);
    for frame in 0..warmup + frames {
        if frame == warmup {
            cx.update(|app| {
                app.update_window(window, |_, window, _| {
                    window.reset_frame_stats();
                    window.set_frame_timing_enabled(options.phase_times);
                })
            })?;
        }
        let draws_before = window_draws(cx, window)?;
        let start = meter.borrow_mut().now();
        let mut step_cost = Cost {
            instructions: meter
                .borrow()
                .instructions_unavailable()
                .is_none()
                .then_some(0),
            ..Cost::default()
        };
        cx.update(|app| {
            driver.step(
                frame,
                &mut Stage::new(app, window, meter, &mut step_cost, &log),
            );
        });
        let mut forced = 0;
        if !cx.update(|app| app.update_window(window, |_, window, _| window.needs_draw()))? {
            cx.update(|app| app.update_window(window, |_, window, cx| window.draw(cx).clear(cx)))?;
            forced = 1;
        }
        // Wait for the display's own frame callback to draw the step.
        let mut waited = 0;
        while forced == 0 && window_draws(cx, window)? == draws_before {
            if waited >= 200 {
                return Err(anyhow!(
                    "{} frame {frame}: the window never drew",
                    workload.name
                ));
            }
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            waited += 1;
        }
        let end = meter.borrow_mut().now();
        let natural = (window_draws(cx, window)? - draws_before).saturating_sub(forced);
        if frame >= warmup {
            natural_draws += natural;
            forced_draws += forced;
            samples.push(FrameSample {
                frame,
                outcome: FrameOutcome {
                    frame: end.since(&start) - step_cost,
                    step: step_cost,
                    natural_draws: natural as u32,
                    forced_draws: forced as u32,
                },
            });
        }
        // Keep to the display's cadence: idle between frames is not CPU.
        cx.background_executor().timer(interval).await;
    }
    let stats = cx.update(|app| app.update_window(window, |_, window, _| window.frame_stats()))?;
    cx.update(|app| app.update_window(window, |_, window, _| window.remove_window()))?;
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
    Ok(ScenarioReport {
        name: workload.name,
        mode,
        warmup,
        frames,
        samples,
        natural_draws,
        forced_draws,
        stats,
        instructions_unavailable: meter.borrow().instructions_unavailable().map(str::to_owned),
    })
}

fn window_draws(cx: &mut AsyncApp, window: AnyWindowHandle) -> Result<u64> {
    cx.update(|app| app.update_window(window, |_, window, _| window.frame_stats().draws))
}

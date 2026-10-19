//! DEV-003 R7: a real-window showcase of retained frames.
//!
//! Independent controls switch retention and data refresh on and off while the window runs;
//! automatic runs (idle, sidebar scroll, page changes, table scroll, data refresh) drive the
//! window with a scripted step per frame; the status line shows frame and phase metrics and
//! refreshes every 500 ms, not every frame. "Compare" runs the chosen automatic run twice in
//! this very process, retention off and then on, on the same scene and build, so the only
//! difference between the two numbers is the optimization.
//!
//! ```sh
//! cargo run -p vespertine_bench --example retention_showcase            # interactive
//! cargo run -p vespertine_bench --example retention_showcase -- --auto  # every run, both modes, then exit
//! ```
//!
//! Comparisons with the upstream implementation come from the benchmark scenes measured by the
//! driver against a separately built baseline; they are different baselines and are never
//! mixed with this same-build toggle (DEV-003 R8).

use std::time::{Duration, Instant};
use vespertine::{
    AnyWindowHandle, App, AppContext as _, Bounds, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowOptions,
    div, point, prelude::FluentBuilder as _, px, rgb, size,
};
use vespertine_bench::metrics::thread_cpu_ns;
use vespertine_platform::application;

const ROWS: usize = 200;
const SIDEBAR_ENTRIES: usize = 80;
const PAGES: usize = 3;
const RUN_FRAMES: u64 = 240;
const STATUS_INTERVAL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_micros(16_667);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RunKind {
    Idle,
    Sidebar,
    Page,
    Table,
    Refresh,
}

impl RunKind {
    const ALL: [Self; 5] = [
        Self::Idle,
        Self::Sidebar,
        Self::Page,
        Self::Table,
        Self::Refresh,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Sidebar => "sidebar scroll",
            Self::Page => "page changes",
            Self::Table => "table scroll",
            Self::Refresh => "data refresh",
        }
    }
}

/// One automatic run in progress.
struct Run {
    kind: RunKind,
    frame: u64,
    started: Instant,
    cpu_start: u64,
    draws_start: u64,
    frame_time_start: Duration,
    /// For a comparison: the retention setting to restore and the finished off-run result.
    compare: Option<Comparison>,
}

struct Comparison {
    restore_retention: bool,
    off: Option<String>,
}

#[derive(Default)]
struct Settings {
    retention: bool,
    refresh: u8,
    run: Option<Run>,
    page: usize,
    status: SharedString,
    results: Vec<String>,
    quotes: Vec<f64>,
    previous: Vec<f64>,
    tick: u64,
}

fn refresh_label(level: u8) -> &'static str {
    match level {
        0 => "off",
        1 => "2% of rows",
        _ => "50% of rows",
    }
}

struct Quote {
    row: usize,
    settings: Entity<Settings>,
}

impl Render for Quote {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.read(cx);
        let (price, previous) = (settings.quotes[self.row], settings.previous[self.row]);
        let change = (price - previous) / previous * 100.0;
        div()
            .h(px(24.0))
            .px(px(12.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(24.0))
            .when(self.row % 2 == 1, |row| row.bg(rgb(0xf6f8fa)))
            .child(div().w(px(80.0)).child(format!("SYM{:03}", self.row)))
            .child(div().w(px(80.0)).child(format!("{price:.2}")))
            .child(
                div()
                    .text_color(rgb(if change >= 0.0 { 0x1a7f37 } else { 0xcf222e }))
                    .child(format!("{change:+.2}%")),
            )
    }
}

struct Sidebar {
    scroll: ScrollHandle,
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar")
            .w(px(180.0))
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(rgb(0xf6f8fa))
            .children((0..SIDEBAR_ENTRIES).map(|entry| {
                div()
                    .h(px(28.0))
                    .px(px(12.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(format!("Section {entry}"))
            }))
    }
}

struct Page {
    settings: Entity<Settings>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.settings.read(cx).page;
        div()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Page {}", page + 1)),
            )
            .children((0..6).map(|line| {
                div().child(format!(
                    "Page {} line {line}: retained frames reuse what did not change",
                    page + 1
                ))
            }))
    }
}

struct Table {
    scroll: ScrollHandle,
    rows: Vec<Entity<Quote>>,
}

impl Render for Table {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("table")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(self.rows.iter().cloned())
    }
}

struct Status {
    settings: Entity<Settings>,
}

impl Render for Status {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.read(cx);
        div()
            .p(px(8.0))
            .flex()
            .flex_col()
            .bg(rgb(0x0d1117))
            .text_color(rgb(0xe6edf3))
            .text_size(px(12.0))
            .child(settings.status.clone())
            .children(
                settings
                    .results
                    .iter()
                    .rev()
                    .take(4)
                    .cloned()
                    .map(|line| div().child(line)),
            )
    }
}

struct Controls {
    settings: Entity<Settings>,
}

fn button(id: &'static str, label: String, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.0))
        .h(px(26.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(0xd0d7de))
        .bg(rgb(if active { 0xddf4ff } else { 0xffffff }))
        .cursor_pointer()
        .child(label)
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.read(cx);
        let (retention, refresh, running) =
            (settings.retention, settings.refresh, settings.run.is_some());
        let settings_entity = self.settings.clone();
        let on_click = |action: fn(&mut Settings)| {
            let settings = settings_entity.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    action(settings);
                    cx.notify();
                });
            }
        };
        div()
            .p(px(8.0))
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .items_center()
            .child(
                button(
                    "retention",
                    format!("Retention: {}", if retention { "on" } else { "off" }),
                    retention,
                )
                .on_click(on_click(|settings| {
                    settings.retention = !settings.retention
                })),
            )
            .child(
                button(
                    "refresh",
                    format!("Data refresh: {}", refresh_label(refresh)),
                    refresh > 0,
                )
                .on_click(on_click(|settings| {
                    settings.refresh = (settings.refresh + 1) % 3
                })),
            )
            .child(div().w(px(12.0)))
            .child(
                button("run-idle", "Run: idle".into(), running)
                    .on_click(on_click(|s| s.start(RunKind::Idle, None))),
            )
            .child(
                button("run-sidebar", "Run: sidebar".into(), running)
                    .on_click(on_click(|s| s.start(RunKind::Sidebar, None))),
            )
            .child(
                button("run-page", "Run: page".into(), running)
                    .on_click(on_click(|s| s.start(RunKind::Page, None))),
            )
            .child(
                button("run-table", "Run: table".into(), running)
                    .on_click(on_click(|s| s.start(RunKind::Table, None))),
            )
            .child(
                button("run-refresh", "Run: refresh".into(), running)
                    .on_click(on_click(|s| s.start(RunKind::Refresh, None))),
            )
            .child(
                button("compare", "Compare off/on (table scroll)".into(), running)
                    .on_click(on_click(|s| s.compare(RunKind::Table))),
            )
    }
}

impl Settings {
    fn start(&mut self, kind: RunKind, compare: Option<Comparison>) {
        self.run = Some(Run {
            kind,
            frame: 0,
            started: Instant::now(),
            cpu_start: thread_cpu_ns().unwrap_or(0),
            draws_start: 0,
            frame_time_start: Duration::ZERO,
            compare,
        });
    }

    /// Runs `kind` with retention off, then on, from the same state.
    fn compare(&mut self, kind: RunKind) {
        let restore = self.retention;
        self.retention = false;
        self.start(
            kind,
            Some(Comparison {
                restore_retention: restore,
                off: None,
            }),
        );
    }
}

struct Showcase {
    controls: Entity<Controls>,
    sidebar: Entity<Sidebar>,
    page: Entity<Page>,
    table: Entity<Table>,
    status: Entity<Status>,
}

impl Render for Showcase {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex_col()
            .flex()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x1f2328))
            .text_size(px(13.0))
            .child(self.controls.clone())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .child(self.sidebar.clone())
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(self.page.clone())
                            .child(self.table.clone()),
                    ),
            )
            .child(self.status.clone())
    }
}

/// The scroll handles and rows the scripted steps drive.
struct Stage {
    window: AnyWindowHandle,
    settings: Entity<Settings>,
    sidebar: Entity<Sidebar>,
    sidebar_scroll: ScrollHandle,
    table: Entity<Table>,
    table_scroll: ScrollHandle,
    page: Entity<Page>,
    rows: Vec<Entity<Quote>>,
    last_status: Instant,
    status_draws: u64,
    status_cpu: u64,
    status_frame_time: Duration,
}

impl Stage {
    fn tick(&mut self, cx: &mut App) {
        let (retention_wanted, run) = self.settings.update(cx, |settings, _| {
            settings.tick += 1;
            (
                settings.retention,
                settings.run.as_ref().map(|run| (run.kind, run.frame)),
            )
        });
        // The retention switch applies to the window itself.
        let window = self.window;
        cx.update_window(window, |_, window, _| {
            if window.retention_enabled() != retention_wanted {
                window.set_retention_enabled(retention_wanted);
            }
        })
        .ok();

        if let Some((kind, frame)) = run {
            if frame == 0 {
                self.begin_run(cx);
            }
            self.step(kind, frame, cx);
            self.settings.update(cx, |settings, cx| {
                if let Some(run) = &mut settings.run {
                    run.frame += 1;
                }
                cx.notify();
            });
            self.finish_run_if_done(cx);
        }
        let refresh = self.settings.read(cx).refresh;
        if refresh > 0 {
            self.refresh_rows(refresh, self.settings.read(cx).tick, cx);
        }
        self.refresh_status(cx);
    }

    /// Records the counters the finished run's figures are differences from.
    fn begin_run(&mut self, cx: &mut App) {
        let window = self.window;
        let stats = cx
            .update_window(window, |_, window, _| window.frame_stats())
            .unwrap_or_default();
        self.settings.update(cx, |settings, _| {
            if let Some(run) = &mut settings.run {
                run.draws_start = stats.draws;
                run.frame_time_start = stats.frame_time;
                run.cpu_start = thread_cpu_ns().unwrap_or(0);
                run.started = Instant::now();
            }
        });
    }

    fn step(&mut self, kind: RunKind, frame: u64, cx: &mut App) {
        match kind {
            RunKind::Idle => {}
            RunKind::Sidebar => {
                let offset = zigzag(frame, 40.0, 28.0 * SIDEBAR_ENTRIES as f32 - 300.0);
                self.sidebar_scroll.set_offset(point(px(0.0), px(-offset)));
                self.sidebar.update(cx, |_, cx| cx.notify());
            }
            RunKind::Page => {
                if frame.is_multiple_of(4) {
                    self.settings.update(cx, |settings, cx| {
                        settings.page = (frame as usize / 4) % PAGES;
                        cx.notify();
                    });
                    self.page.update(cx, |_, cx| cx.notify());
                }
            }
            RunKind::Table => {
                let offset = zigzag(frame, 37.0, 24.0 * ROWS as f32 - 200.0);
                self.table_scroll.set_offset(point(px(0.0), px(-offset)));
                self.table.update(cx, |_, cx| cx.notify());
            }
            RunKind::Refresh => self.refresh_rows(1, frame, cx),
        }
    }

    fn refresh_rows(&mut self, level: u8, tick: u64, cx: &mut App) {
        let rows: Vec<usize> = if level == 1 {
            (0..4)
                .map(|j| ((3 * tick + 50 * j) % ROWS as u64) as usize)
                .collect()
        } else {
            (0..ROWS)
                .filter(|row| (*row as u64 + tick).is_multiple_of(2))
                .collect()
        };
        self.settings.update(cx, |settings, _| {
            for &row in &rows {
                settings.previous[row] = settings.quotes[row];
                settings.quotes[row] =
                    10.0 + ((37 * row as u64 + 101 * tick) % 9000) as f64 / 100.0;
            }
        });
        for row in rows {
            self.rows[row].update(cx, |_, cx| cx.notify());
        }
    }

    fn finish_run_if_done(&mut self, cx: &mut App) {
        let window = self.window;
        let (frames, started, cpu_start, draws_start, frame_time_start, kind) =
            match self.settings.read(cx).run.as_ref() {
                Some(run) if run.frame >= RUN_FRAMES => (
                    run.frame,
                    run.started,
                    run.cpu_start,
                    run.draws_start,
                    run.frame_time_start,
                    run.kind,
                ),
                _ => return,
            };
        let stats = cx
            .update_window(window, |_, window, _| window.frame_stats())
            .unwrap_or_default();
        let draws = stats.draws.saturating_sub(draws_start);
        let retention = if self.settings.read(cx).retention {
            "retention on"
        } else {
            "retention off"
        };
        let line = if draws == 0 {
            format!(
                "{} {retention}: no frame was drawn during the run; the compositor sends no frame callbacks to a window that is not visible",
                kind.label()
            )
        } else {
            format!(
                "{} {retention}: {:.3} ms/frame in the draw over {draws} draws, {:.3} ms thread CPU per step, wall {:.0} ms for {frames} steps",
                kind.label(),
                (stats.frame_time - frame_time_start).as_secs_f64() * 1e3 / draws as f64,
                (thread_cpu_ns().unwrap_or(0) - cpu_start) as f64 / 1e6 / frames as f64,
                started.elapsed().as_secs_f64() * 1e3,
            )
        };
        println!("{line}");
        self.settings.update(cx, |settings, cx| {
            let run = settings.run.take().expect("a run finished");
            settings.results.push(line.clone());
            if let Some(compare) = run.compare {
                match compare.off {
                    None => {
                        // The off run is done: run the same thing with retention on.
                        settings.retention = true;
                        settings.start(
                            run.kind,
                            Some(Comparison { restore_retention: compare.restore_retention, off: Some(line) }),
                        );
                    }
                    Some(off) => {
                        settings.retention = compare.restore_retention;
                        settings.results.push(format!(
                            "controlled comparison (same build, same scene, only retention differs): [{off}] versus [{line}]"
                        ));
                    }
                }
            }
            cx.notify();
        });
    }

    /// Frame and phase metrics, refreshed every 500 ms rather than every frame.
    fn refresh_status(&mut self, cx: &mut App) {
        if self.last_status.elapsed() < STATUS_INTERVAL {
            return;
        }
        let elapsed = self.last_status.elapsed().as_secs_f64();
        self.last_status = Instant::now();
        let window = self.window;
        let stats = cx
            .update_window(window, |_, window, _| window.frame_stats())
            .unwrap_or_default();
        let cpu = thread_cpu_ns().unwrap_or(0);
        let draws = stats.draws.saturating_sub(self.status_draws);
        let per_draw = |total: Duration, before: Duration| {
            (total.saturating_sub(before)).as_secs_f64() * 1e3 / draws.max(1) as f64
        };
        let text = format!(
            "{:.0} draws/s | frame {:.3} ms | thread CPU {:.1}% | views built {} reused {} | layout nodes reused {} | lines shaped {}",
            draws as f64 / elapsed,
            per_draw(stats.frame_time, self.status_frame_time),
            (cpu - self.status_cpu) as f64 / 1e9 / elapsed * 100.0,
            stats.views_built,
            stats.views_reused,
            stats.nodes_reused,
            stats.lines_shaped,
        );
        self.status_draws = stats.draws;
        self.status_frame_time = stats.frame_time;
        self.status_cpu = cpu;
        self.settings.update(cx, |settings, cx| {
            settings.status = text.into();
            cx.notify();
        });
    }
}

/// A triangle wave over `0..=limit` advancing `step` per frame.
fn zigzag(frame: u64, step: f32, limit: f32) -> f32 {
    let period = (limit / step).max(1.0) as u64;
    let phase = frame % (2 * period);
    let position = if phase < period {
        phase
    } else {
        2 * period - phase
    };
    position as f32 * step
}

fn main() {
    let auto = std::env::args().any(|argument| argument == "--auto");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(880.0), px(560.0)), cx);
        let mut stage = None;
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..WindowOptions::default()
                },
                |window, cx| {
                    window.set_frame_timing_enabled(true);
                    let settings = cx.new(|_| Settings {
                        retention: true,
                        quotes: (0..ROWS)
                            .map(|row| 10.0 + (37 * row % 9000) as f64 / 100.0)
                            .collect(),
                        previous: (0..ROWS)
                            .map(|row| 10.0 + (37 * row % 9000) as f64 / 100.0)
                            .collect(),
                        status: "collecting metrics...".into(),
                        ..Settings::default()
                    });
                    let sidebar_scroll = ScrollHandle::new();
                    let table_scroll = ScrollHandle::new();
                    let rows: Vec<Entity<Quote>> = (0..ROWS)
                        .map(|row| {
                            let settings = settings.clone();
                            cx.new(|_| Quote { row, settings })
                        })
                        .collect();
                    let controls = cx.new(|_| Controls {
                        settings: settings.clone(),
                    });
                    let sidebar = cx.new(|_| Sidebar {
                        scroll: sidebar_scroll.clone(),
                    });
                    let page = cx.new(|_| Page {
                        settings: settings.clone(),
                    });
                    let table = cx.new(|_| Table {
                        scroll: table_scroll.clone(),
                        rows: rows.clone(),
                    });
                    let status = cx.new(|_| Status {
                        settings: settings.clone(),
                    });
                    stage = Some(Stage {
                        window: window.window_handle(),
                        settings,
                        sidebar: sidebar.clone(),
                        sidebar_scroll,
                        table: table.clone(),
                        table_scroll,
                        page: page.clone(),
                        rows,
                        last_status: Instant::now(),
                        status_draws: 0,
                        status_cpu: 0,
                        status_frame_time: Duration::ZERO,
                    });
                    cx.new(|_| Showcase {
                        controls,
                        sidebar,
                        page,
                        table,
                        status,
                    })
                },
            )
            .expect("open the showcase window");
        let _ = window;
        let mut stage = stage.expect("the window built its state");
        cx.activate(true);
        cx.spawn(async move |cx| {
            if auto {
                // Every automatic run, retention off then on, then exit.
                for kind in RunKind::ALL {
                    cx.update(|app| {
                        stage.settings.update(app, |settings, _| {
                            settings.retention = false;
                            settings.start(kind, None);
                        })
                    });
                    wait_for_run(cx, &mut stage).await;
                    cx.update(|app| {
                        stage.settings.update(app, |settings, _| {
                            settings.retention = true;
                            settings.start(kind, None);
                        })
                    });
                    wait_for_run(cx, &mut stage).await;
                }
                cx.update(|app| app.quit());
                return;
            }
            loop {
                cx.background_executor().timer(TICK).await;
                cx.update(|app| stage.tick(app));
            }
        })
        .detach();
    });
}

async fn wait_for_run(cx: &mut vespertine::AsyncApp, stage: &mut Stage) {
    loop {
        cx.background_executor().timer(TICK).await;
        let running = cx.update(|app| {
            stage.tick(app);
            stage.settings.read(app).run.is_some()
        });
        if !running {
            return;
        }
    }
}

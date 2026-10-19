//! Framework-agnostic scene runner implementing the runner protocol: command line, window
//! setup, perf / idle / capture modes, frame counting, CPU accounting and the result file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::scene::{Scene, create_scene};
use vespertine::{
    AppContext, Application, Bounds, Context, IntoElement, Render, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions, div,
    point, px, size,
};

const APP_ID: &str = "vsp-scene";
const DEFAULT_WARMUP: u64 = 120;
const DEFAULT_FRAMES: u64 = 600;
const CAPTURE_FRAMES: u64 = 60;
const CAPTURE_SETTLE: Duration = Duration::from_millis(500);
const CAPTURE_LIFETIME: Duration = Duration::from_secs(30);
/// Idle mode records `t0`/`c0` this long after the last warmup frame was requested, so that the
/// last frame's paint and present are not charged to the idle interval.
const IDLE_SETTLE: Duration = Duration::from_millis(250);
const IDLE_SLOT_HZ: u64 = 60;

const EXIT_UNSUPPORTED: u8 = 3;
const EXIT_CAPTURE_TIMEOUT: i32 = 4;

enum Mode {
    Perf,
    Idle,
    Capture,
}

impl Clone for Mode {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for Mode {}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Perf => "perf",
            Mode::Idle => "idle",
            Mode::Capture => "capture",
        }
    }
}

struct Config {
    scene: String,
    mode: Mode,
    out: PathBuf,
    warmup: u64,
    frames: u64,
    ready: Option<PathBuf>,
    label: String,
}

fn usage() -> &'static str {
    "usage: <runner> --scene <SCENE-ID> --mode <perf|idle|capture> --out <result.json> \
     [--warmup <N>=120] [--frames <N>=600] [--ready <file>] [--label <framework-label>]"
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Config, String> {
    let mut scene = None;
    let mut mode = None;
    let mut out = None;
    let mut warmup = DEFAULT_WARMUP;
    let mut frames = DEFAULT_FRAMES;
    let mut ready = None;
    let mut label = String::new();

    let mut args = args;
    while let Some(flag) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {name}"))
        };
        match flag.as_str() {
            "--scene" => scene = Some(value("--scene")?),
            "--mode" => {
                mode = Some(match value("--mode")?.as_str() {
                    "perf" => Mode::Perf,
                    "idle" => Mode::Idle,
                    "capture" => Mode::Capture,
                    other => return Err(format!("unknown mode `{other}`")),
                })
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--warmup" => {
                warmup = value("--warmup")?
                    .parse()
                    .map_err(|e| format!("invalid --warmup: {e}"))?
            }
            "--frames" => {
                frames = value("--frames")?
                    .parse()
                    .map_err(|e| format!("invalid --frames: {e}"))?
            }
            "--ready" => ready = Some(PathBuf::from(value("--ready")?)),
            "--label" => label = value("--label")?,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    let mode = mode.ok_or("missing --mode")?;
    let is_capture = matches!(mode, Mode::Capture);
    if is_capture && ready.is_none() {
        return Err("capture mode requires --ready".into());
    }
    if frames == 0 || (!is_capture && warmup == 0) {
        return Err("--frames and --warmup must be positive".into());
    }
    Ok(Config {
        scene: scene.ok_or("missing --scene")?,
        mode,
        out: out.ok_or("missing --out")?,
        warmup,
        frames,
        ready,
        label,
    })
}

/// A point on the monotonic clock together with the process CPU time (user + system, all threads).
struct Sample {
    wall_ns: u64,
    cpu_ns: u64,
}

impl Clone for Sample {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for Sample {}

impl Sample {
    fn now() -> Self {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: both out-pointers are valid for the duration of the calls.
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
            libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        }
        let timeval_ns =
            |tv: libc::timeval| tv.tv_sec as u64 * 1_000_000_000 + tv.tv_usec as u64 * 1_000;
        Sample {
            wall_ns: ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64,
            cpu_ns: timeval_ns(usage.ru_utime) + timeval_ns(usage.ru_stime),
        }
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes the result file atomically (temporary file in the same directory, then rename).
fn write_result(config: &Config, frames: u64, cpu_ns: u64, wall_ns: u64) -> std::io::Result<()> {
    let json = format!(
        "{{\"scene\": {}, \"mode\": {}, \"label\": {}, \"frames\": {frames}, \"cpu_ns\": {cpu_ns}, \"wall_ns\": {wall_ns}}}\n",
        json_string(&config.scene),
        json_string(config.mode.name()),
        json_string(&config.label),
    );
    let mut temp = config.out.clone().into_os_string();
    temp.push(format!(".tmp{}", std::process::id()));
    let temp = PathBuf::from(temp);
    fs::write(&temp, json)?;
    fs::rename(&temp, &config.out)
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

/// Root view: counts frames, applies the mode's schedule and delegates content to the scene.
struct Harness {
    scene: Box<dyn Scene>,
    config: std::rc::Rc<Config>,
    /// Number of invocations of the per-frame render so far.
    rendered: u64,
    start: Option<Sample>,
    finished: bool,
}

impl Harness {
    fn finish_perf(&mut self, end: Sample, cx: &mut Context<Self>) {
        let Some(start) = self.start else {
            fail("measurement ended before it started");
        };
        let result = write_result(
            &self.config,
            self.config.frames,
            end.cpu_ns - start.cpu_ns,
            end.wall_ns - start.wall_ns,
        );
        if let Err(error) = result {
            fail(format_args!(
                "writing {}: {error}",
                self.config.out.display()
            ));
        }
        self.finished = true;
        cx.quit();
    }

    fn schedule_idle_measurement(&self, cx: &mut Context<Self>) {
        let config = self.config.clone();
        cx.spawn(async move |_this, cx| {
            let executor = cx.background_executor().clone();
            executor.timer(IDLE_SETTLE).await;
            let start = Sample::now();
            let slots = Duration::from_nanos(config.frames * 1_000_000_000 / IDLE_SLOT_HZ);
            executor.timer(slots).await;
            let end = Sample::now();
            if let Err(error) = write_result(
                &config,
                config.frames,
                end.cpu_ns - start.cpu_ns,
                end.wall_ns - start.wall_ns,
            ) {
                fail(format_args!("writing {}: {error}", config.out.display()));
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    fn schedule_capture_ready(&self, cx: &mut Context<Self>) {
        let config = self.config.clone();
        let origin = self.start.unwrap_or_else(Sample::now);
        cx.spawn(async move |_this, cx| {
            let executor = cx.background_executor().clone();
            executor.timer(CAPTURE_SETTLE).await;
            let now = Sample::now();
            if let Err(error) = write_result(
                &config,
                CAPTURE_FRAMES,
                now.cpu_ns - origin.cpu_ns,
                now.wall_ns - origin.wall_ns,
            ) {
                fail(format_args!("writing {}: {error}", config.out.display()));
            }
            let ready: &Path = config.ready.as_deref().expect("checked in parse_args");
            if let Err(error) = fs::write(ready, b"") {
                fail(format_args!("creating {}: {error}", ready.display()));
            }
            executor.timer(CAPTURE_LIFETIME).await;
            std::process::exit(EXIT_CAPTURE_TIMEOUT)
        })
        .detach();
    }
}

impl Render for Harness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let f = self.rendered;
        self.rendered += 1;
        if self.finished {
            return div().into_any_element();
        }
        let config = self.config.clone();
        match config.mode {
            Mode::Perf => {
                if f == config.warmup {
                    self.start = Some(Sample::now());
                }
                if f == config.warmup + config.frames {
                    self.finish_perf(Sample::now(), cx);
                    return div().into_any_element();
                }
                let element = self.scene.frame(f, window, cx);
                window.request_animation_frame();
                element
            }
            Mode::Idle => {
                // Frames after the warmup (for example compositor-driven redraws) show the last
                // warmup frame again and never request another one.
                let shown = f.min(config.warmup - 1);
                let element = self.scene.frame(shown, window, cx);
                if f + 1 < config.warmup {
                    window.request_animation_frame();
                } else if f + 1 == config.warmup {
                    self.schedule_idle_measurement(cx);
                }
                element
            }
            Mode::Capture => {
                if f == 0 {
                    self.start = Some(Sample::now());
                }
                let element = self.scene.frame(self.scene.capture_frame(), window, cx);
                if f + 1 < CAPTURE_FRAMES {
                    window.request_animation_frame();
                } else if f + 1 == CAPTURE_FRAMES {
                    self.schedule_capture_ready(cx);
                }
                element
            }
        }
    }
}

/// Runs the runner protocol. `make_application` constructs the framework's platform
/// application; it is called only after the command line and scene have been validated.
pub fn run(make_application: impl FnOnce() -> Application) -> ExitCode {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}\n{}", usage());
            return ExitCode::from(2);
        }
    };
    let Some(scene) = create_scene(&config.scene) else {
        eprintln!("unsupported scene `{}`", config.scene);
        return ExitCode::from(EXIT_UNSUPPORTED);
    };

    let config = std::rc::Rc::new(config);
    let (width, height) = scene.size();
    let title = format!("vsp-scene:{}", config.scene);
    let mut scene = Some(scene);

    make_application().run(move |cx| {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(width), px(height)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            focus: false,
            show: true,
            kind: WindowKind::Normal,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            // Frames must be produced at the display rate even though the window is never focused.
            inactive_frame_interval: None,
            window_background: WindowBackgroundAppearance::Opaque,
            app_id: Some(APP_ID.into()),
            window_decorations: Some(WindowDecorations::Server),
            ..Default::default()
        };
        let scene = scene.take().expect("launch callback runs once");
        let opened = cx.open_window(options, move |_window, cx| {
            cx.new(|_cx| Harness {
                scene,
                config,
                rendered: 0,
                start: None,
                finished: false,
            })
        });
        if let Err(error) = opened {
            fail(format_args!("opening window: {error}"));
        }
    });
    ExitCode::SUCCESS
}

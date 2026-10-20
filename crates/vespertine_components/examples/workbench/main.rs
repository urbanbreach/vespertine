//! A desktop agent workbench built only from Vespertine components: a session sidebar, tabs
//! (CMP-017) and resizable panes (LAY-004) around a streaming Markdown transcript with code blocks
//! (CMP-011, CMP-014) that follows the bottom while it streams (CMP-029), a multi-line input
//! (CMP-008) and theme tokens throughout.
//!
//! ```text
//! cargo run --release --example workbench                      # interactive
//! cargo run --release --example workbench -- --scripted 60     # deterministic script, 60 s
//! ```
//!
//! `--scripted N` drives the window with a fixed timeline for N seconds (a full cycle always
//! runs): it streams replies, types into the input and submits, switches tabs, drags a pane seam
//! and scrolls the history, asserting the invariants along the way (pinned to the bottom while
//! streaming, input cleared on submit, the active tab, no panics). It exits 0 when every
//! assertion held and 1, with the failing assertion on stderr, otherwise.

mod app;
mod content;
mod script;

use std::time::{Duration, Instant};

use vespertine::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Window, WindowBounds, WindowOptions,
    px, size,
};
use vespertine_platform::application;

use app::Workbench;
use script::{Script, Stage, TICK_MS};

/// The live window as a stage: every step runs inside the window on the main thread.
struct LiveStage {
    cx: AsyncApp,
    window: AnyWindowHandle,
}

impl Stage for LiveStage {
    fn run(&mut self, f: &mut dyn FnMut(&mut Window, &mut App)) {
        self.cx
            .update_window(self.window, |_, window, cx| f(window, cx))
            .expect("the workbench window is open");
    }
}

fn scripted_seconds() -> Option<u64> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--scripted=") {
            return Some(parse_seconds(value));
        }
        if argument == "--scripted" {
            let value = args
                .next()
                .unwrap_or_else(|| usage("--scripted needs seconds"));
            return Some(parse_seconds(&value));
        }
    }
    None
}

fn parse_seconds(value: &str) -> u64 {
    value
        .parse()
        .unwrap_or_else(|_| usage(&format!("{value:?} is not a number of seconds")))
}

fn usage(problem: &str) -> ! {
    eprintln!("workbench: {problem}\nusage: workbench [--scripted SECONDS]");
    std::process::exit(2);
}

fn main() {
    let scripted = scripted_seconds();
    if scripted.is_some() {
        script::watch_panics();
    }
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workbench::new(window, cx)),
            )
            .expect("open the workbench window");
        cx.activate(true);
        let Some(seconds) = scripted else {
            window
                .update(cx, |workbench, window, cx| {
                    workbench.focus_input(window, cx);
                    workbench.start_stream_clock(window, cx);
                })
                .expect("the workbench window is open");
            return;
        };
        cx.spawn(async move |cx| {
            let workbench = window
                .entity(cx)
                .expect("the workbench window is open");
            let mut script = Script::new(workbench, seconds);
            let mut stage = LiveStage {
                cx: cx.clone(),
                window: window.into(),
            };
            // Let the window map and paint once so the script can find its geometry.
            cx.background_executor().timer(Duration::from_secs(1)).await;
            loop {
                let started = Instant::now();
                match script.tick(&mut stage) {
                    Err(message) => {
                        eprintln!("workbench: FAILED: {message}");
                        std::process::exit(1);
                    }
                    Ok(true) => break,
                    Ok(false) => {}
                }
                let spent = started.elapsed();
                if let Some(rest) = Duration::from_millis(TICK_MS).checked_sub(spent) {
                    cx.background_executor().timer(rest).await;
                }
            }
            println!(
                "workbench: ok ({} checks over {} ticks; worst distance from the end while streaming {:.1} px; drove: {})",
                script.checks(),
                script.ticks(),
                script.worst_lag(),
                script.exercised().iter().copied().collect::<Vec<_>>().join(", ")
            );
            std::process::exit(0);
        })
        .detach();
    });
}

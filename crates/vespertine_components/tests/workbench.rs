//! The workbench example's scripted run, headless on the production test host.
//!
//! The example's view and script are compiled into this test (the same source files the example
//! uses), so the run here is the run `cargo run --release --example workbench -- --scripted N`
//! performs: streaming transcript, typing and submit, tabs, a pane seam drag and history
//! scrolling, with the same assertions. Only the stage differs: the host's deterministic
//! executor time and motion clock step 50 ms per tick instead of the wall clock.

#[allow(dead_code)]
#[path = "../examples/workbench/app.rs"]
mod app;
#[allow(dead_code)]
#[path = "../examples/workbench/content.rs"]
mod content;
#[allow(dead_code)]
#[path = "../examples/workbench/script.rs"]
mod script;

use std::{cell::RefCell, rc::Rc};

use vespertine::{App, AppContext as _, Entity, Window};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

use app::Workbench;
use script::{Script, Stage, TICK_MS};

struct HostStage<'a> {
    host: &'a mut TestHost,
}

impl Stage for HostStage<'_> {
    fn run(&mut self, f: &mut dyn FnMut(&mut Window, &mut App)) {
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| f(window, cx))
            .expect("the workbench window is open");
    }
}

/// Runs a script of `seconds` on a fresh host and returns the finished script.
fn run(seconds: u64) -> Script {
    let slot: Rc<RefCell<Option<Entity<Workbench>>>> = Rc::default();
    let captured = slot.clone();
    let mut host = TestHost::with_view(
        HostOptions {
            scale_factor: 1.0,
            ..HostOptions::with_size(1280.0, 820.0)
        },
        move |window, cx| {
            let workbench = cx.new(|cx| Workbench::new(window, cx));
            *captured.borrow_mut() = Some(workbench.clone());
            workbench
        },
    )
    .expect("the test host");
    let workbench = slot.borrow().clone().expect("the workbench view");
    host.motion_pause();
    host.flush();
    let mut script = Script::new(workbench, seconds);
    loop {
        let finished = script
            .tick(&mut HostStage { host: &mut host })
            .unwrap_or_else(|message| panic!("workbench script failed: {message}"));
        host.flush();
        host.advance_executor_time(TICK_MS as f64).expect("time");
        host.motion_fast_forward(TICK_MS as f64).expect("motion");
        if finished {
            return script;
        }
    }
}

#[test]
fn the_scripted_workbench_run_passes_headless() {
    require_backend!();
    let script = run(1);
    println!(
        "{} checks over {} ticks, worst lag {:.1} px",
        script.checks(),
        script.ticks(),
        script.worst_lag()
    );
    assert!(script.checks() >= script.ticks());
    assert!(script.worst_lag().is_finite());
    let expected = [
        "sidebar click",
        "tab click",
        "tab cycling",
        "editor click",
        "typing",
        "line break",
        "submit",
        "token streaming",
        "history scroll",
        "jump button click",
        "seam drag",
    ];
    for kind in expected {
        assert!(
            script.exercised().contains(kind),
            "the script never drove {kind}"
        );
    }
}

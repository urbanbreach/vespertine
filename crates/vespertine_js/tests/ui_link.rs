//! APP-005 R2/R3 (A3): result-returning commands never hang, and fire-and-forget commands
//! fail only when the UI loop is not running.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use vespertine_js::{HostError, HostRenderer, RESPONSE_TIMEOUT, UiLink, UiTask};

fn renderer_with_silent_loop() -> (
    HostRenderer,
    async_channel::Receiver<UiTask>,
    Arc<AtomicBool>,
) {
    let renderer = HostRenderer::new(None);
    let (link, tasks) = UiLink::<UiTask>::channel();
    let alive = Arc::new(AtomicBool::new(true));
    renderer.attach_link(link, alive.clone()).unwrap();
    (renderer, tasks, alive)
}

#[test]
fn a3_an_unresponsive_ui_loop_times_out_after_two_seconds() {
    assert_eq!(RESPONSE_TIMEOUT, Duration::from_secs(2));
    // The receiver is held (the loop is "alive") but never executes a task.
    let (renderer, _tasks, _alive) = renderer_with_silent_loop();
    let started = Instant::now();
    let error = renderer.window_size().unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(
        error.to_string(),
        "Timed out after 2 seconds waiting for window size"
    );
    assert!(
        elapsed >= Duration::from_millis(1900) && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
    assert!(matches!(error, HostError::TimedOut { .. }));
}

#[test]
fn a3_a_loop_that_stops_while_the_caller_waits_reports_the_stopped_error() {
    let (renderer, tasks, _alive) = renderer_with_silent_loop();
    let stopper = thread::spawn(move || {
        // Take the command off the queue and drop it unanswered, as a UI loop that exits does.
        let task = tasks.recv_blocking().unwrap();
        drop(task);
        drop(tasks);
    });
    let started = Instant::now();
    let error = renderer.window_insets().unwrap_err();
    stopper.join().unwrap();
    assert_eq!(
        error.to_string(),
        "The GPUI UI thread stopped during window insets"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_command_after_the_loop_is_gone_fails_with_the_not_running_error() {
    let (renderer, tasks, _alive) = renderer_with_silent_loop();
    drop(tasks);
    for result in [
        renderer.activate_window(),
        renderer.minimize_window(),
        renderer.zoom_window(),
        renderer.toggle_fullscreen(),
        renderer.set_window_title("t".into()),
        renderer.request_repaint(),
    ] {
        assert_eq!(
            result.unwrap_err().to_string(),
            "The GPUI UI thread is not running"
        );
    }
    assert_eq!(
        renderer.window_size().unwrap_err().to_string(),
        "The GPUI UI thread is not running"
    );
}

#[test]
fn commands_before_initialization_fail_with_the_uninitialized_error() {
    let renderer = HostRenderer::new(None);
    assert!(!renderer.is_initialized());
    assert_eq!(
        renderer.window_size().unwrap_err(),
        HostError::NotInitialized
    );
    assert_eq!(
        renderer.activate_window().unwrap_err(),
        HostError::NotInitialized
    );
    assert!(renderer.prompt_for_paths(&Default::default()).is_err());
}

#[test]
fn tick_before_initialization_tells_the_caller_to_initialize_first() {
    let renderer = HostRenderer::new(None);
    let error = renderer.tick().unwrap_err();
    assert!(
        error.to_string().contains("call init() before tick()"),
        "{error}"
    );
}

#[test]
fn tick_reports_liveness_and_a_second_attachment_is_rejected() {
    let (renderer, _tasks, alive) = renderer_with_silent_loop();
    assert!(renderer.is_initialized());
    assert_eq!(renderer.tick(), Ok(true));
    let (other, _rx) = UiLink::<UiTask>::channel();
    assert_eq!(
        renderer.attach_link(other, Arc::new(AtomicBool::new(true))),
        Err(HostError::AlreadyInitialized)
    );
    alive.store(false, Ordering::SeqCst);
    assert_eq!(renderer.tick(), Ok(false));
    // The channel is released: later commands fail with the not-running error.
    assert_eq!(renderer.activate_window(), Err(HostError::NotRunning));
    assert_eq!(renderer.tick(), Ok(false));
    assert!(renderer.is_initialized());
}

#[test]
fn queued_commands_run_in_submission_order() {
    let (renderer, tasks, _alive) = renderer_with_silent_loop();
    renderer.activate_window().unwrap();
    renderer.set_window_title("a".into()).unwrap();
    renderer.request_repaint().unwrap();
    assert_eq!(tasks.len(), 3);
}

#[test]
fn a_prompt_whose_loop_stops_rejects_with_the_stopped_error() {
    let (renderer, tasks, _alive) = renderer_with_silent_loop();
    let answer = renderer.prompt_for_paths(&Default::default()).unwrap();
    drop(tasks.recv_blocking().unwrap());
    let error = futures::executor::block_on(answer.wait()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "The GPUI UI thread stopped during path prompt"
    );
}

#[test]
fn txt003_a6_font_registration_with_an_unresponsive_ui_loop_errors_after_the_timeout() {
    let (renderer, _tasks, _alive) = renderer_with_silent_loop();
    let started = Instant::now();
    let error = renderer.register_font(vec![1, 2, 3, 4]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Timed out after 2 seconds waiting for font registration"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

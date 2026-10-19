//! The UI-thread side of APP-005 (R3, R6, R8, A6), driven against the test platform: the
//! tasks a renderer queues are executed exactly as the UI loop executes them, without a
//! display server.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

use vespertine::{AppContext, TestAppContext, WindowOptions, px, size};
use vespertine_js::{
    HostError, HostRenderer, HostWindow, PromptConfig, UiLink, UiTask, WindowInsets, WindowSize,
};

struct Rig {
    cx: TestAppContext,
    window: HostWindow,
    tasks: async_channel::Receiver<UiTask>,
    renderer: Arc<HostRenderer>,
}

fn rig() -> Rig {
    let cx = TestAppContext::single();
    let renderer = Arc::new(HostRenderer::new(None));
    let (link, tasks) = UiLink::<UiTask>::channel();
    renderer
        .attach_link(link, Arc::new(AtomicBool::new(true)))
        .unwrap();
    let renderer_view = renderer.new_view();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(vespertine::WindowBounds::Windowed(vespertine::Bounds {
                    origin: vespertine::point(px(0.), px(0.)),
                    size: size(px(320.), px(240.)),
                })),
                ..Default::default()
            },
            |_, cx| cx.new(|_| renderer_view),
        )
        .unwrap()
    });
    Rig {
        cx,
        window,
        tasks,
        renderer,
    }
}

impl Rig {
    /// Runs every queued task on this thread, as the UI loop does.
    fn pump(&self) -> usize {
        let mut ran = 0;
        while let Ok(task) = self.tasks.try_recv() {
            self.cx.update(|cx| task(cx, &self.window));
            ran += 1;
        }
        ran
    }

    /// Runs `call` on a helper thread (it blocks on a result) while this thread serves tasks.
    fn serve<T: Send>(&self, call: impl FnOnce(&HostRenderer) -> T + Send) -> T {
        thread::scope(|scope| {
            let renderer = self.renderer.clone();
            let handle = scope.spawn(move || call(&renderer));
            while !handle.is_finished() {
                self.pump();
                thread::yield_now();
            }
            handle.join().unwrap()
        })
    }
}

#[test]
fn the_size_query_returns_the_viewport_in_logical_pixels() {
    let rig = rig();
    let size = rig.serve(|renderer| renderer.window_size().unwrap());
    assert_eq!(
        size,
        WindowSize {
            width: 320.0,
            height: 240.0
        }
    );
}

#[test]
fn the_insets_query_reports_zero_safe_area_ime_and_effective_insets() {
    let rig = rig();
    let insets = rig.serve(|renderer| renderer.window_insets().unwrap());
    assert_eq!(insets, WindowInsets::default());
}

#[test]
fn fire_and_forget_commands_run_in_order_on_the_ui_thread() {
    let rig = rig();
    rig.renderer.set_window_title("first".into()).unwrap();
    rig.renderer.set_window_title("second".into()).unwrap();
    rig.renderer.toggle_fullscreen().unwrap();
    rig.renderer.activate_window().unwrap();
    rig.renderer.request_repaint().unwrap();
    // minimize and zoom are not implemented by the test platform; the live
    // `app_lifecycle` example issues them against real windows.
    assert_eq!(rig.pump(), 5);
    let fullscreen = rig
        .cx
        .update(|cx| rig.window.update(cx, |_, window, _| window.is_fullscreen()))
        .unwrap();
    let mut visual = vespertine::VisualTestContext::from_window(rig.window.into(), &rig.cx);
    assert_eq!(visual.window_title().as_deref(), Some("second"));
    assert!(fullscreen);
}

#[test]
fn a_batch_mounts_and_repaints_through_the_window_view() {
    let rig = rig();
    rig.renderer
        .apply_batch(
            r##"[["createElement",1,"div"],["setStyle",1,{"backgroundColor":"#ff0000","width":40,"height":20}],
                ["createElement",2,"text"],["setText",2,"hello"],["appendChild",1,2],["setRoot",1]]"##,
        )
        .unwrap();
    assert_eq!(rig.pump(), 1, "the changing batch queued one repaint");
    rig.cx.run_until_parked();
    rig.renderer
        .apply_batch(r##"[["setStyle",1,{"backgroundColor":"#ff0000","height":20,"width":40}]]"##)
        .unwrap();
    assert_eq!(rig.pump(), 0, "an equivalent resend queues no repaint");
}

fn prompt(rig: &Rig, config: PromptConfig) -> vespertine_js::PromptAnswer {
    let answer = rig.renderer.prompt_for_paths(&config).unwrap();
    rig.pump();
    answer
}

#[test]
fn a_cancelled_prompt_resolves_to_null() {
    let rig = rig();
    let answer = prompt(&rig, PromptConfig::default());
    assert!(rig.cx.did_prompt_for_paths());
    rig.cx.simulate_path_prompt_response(|_| None);
    rig.cx.run_until_parked();
    assert_eq!(futures::executor::block_on(answer.wait()), Ok(None));
}

#[test]
fn an_empty_selection_resolves_to_null() {
    let rig = rig();
    let answer = prompt(&rig, PromptConfig::default());
    rig.cx.simulate_path_prompt_response(|_| Some(vec![]));
    rig.cx.run_until_parked();
    assert_eq!(futures::executor::block_on(answer.wait()), Ok(None));
}

#[test]
fn selected_paths_resolve_to_an_array_of_strings_and_options_reach_the_platform() {
    let rig = rig();
    let answer = prompt(
        &rig,
        PromptConfig {
            directories: Some(true),
            multiple: Some(true),
            prompt: Some("Pick".into()),
            ..Default::default()
        },
    );
    rig.cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && options.multiple);
        assert_eq!(options.prompt.as_deref(), Some("Pick"));
        Some(vec!["/tmp/a".into(), "/tmp/b".into()])
    });
    rig.cx.run_until_parked();
    assert_eq!(
        futures::executor::block_on(answer.wait()),
        Ok(Some(vec!["/tmp/a".to_string(), "/tmp/b".to_string()]))
    );
}

#[test]
fn invalid_prompt_options_reject_without_opening_a_chooser() {
    let rig = rig();
    let answer = prompt(
        &rig,
        PromptConfig {
            files: Some(false),
            directories: Some(false),
            ..Default::default()
        },
    );
    rig.cx.run_until_parked();
    let error = futures::executor::block_on(answer.wait()).unwrap_err();
    assert!(
        error.to_string().contains("requires files or directories"),
        "{error}"
    );
    assert!(!rig.cx.did_prompt_for_paths());
}

#[test]
fn files_and_directories_together_reject_where_the_platform_cannot_mix_them() {
    let rig = rig();
    let mixed_supported = rig.cx.update(|cx| cx.can_select_mixed_files_and_dirs());
    let answer = prompt(
        &rig,
        PromptConfig {
            files: Some(true),
            directories: Some(true),
            ..Default::default()
        },
    );
    rig.cx.run_until_parked();
    if mixed_supported {
        assert!(rig.cx.did_prompt_for_paths());
    } else {
        let error = futures::executor::block_on(answer.wait()).unwrap_err();
        assert!(matches!(error, HostError::Invalid(_)), "{error}");
    }
}

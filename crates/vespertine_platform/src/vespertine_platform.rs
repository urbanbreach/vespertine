//! Convenience crate that re-exports VESPERTINE's platform traits and the
//! `current_platform` constructor for Linux, macOS, and Windows desktop applications.

pub use vespertine::Platform;

/// The reference solid-color producer for native surfaces (PLT-001).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use vespertine_linux::{SolidChild, SolidProducer};

use std::rc::Rc;

/// Returns a background executor for the current platform.
pub fn background_executor() -> vespertine::BackgroundExecutor {
    current_platform(true).background_executor()
}

/// The user agent of the HTTP client every desktop application installs at startup.
const HTTP_USER_AGENT: &str = concat!("vespertine/", env!("CARGO_PKG_VERSION"));

/// Installs the desktop HTTP client (ELM-004 R7) for image fetching. The framework default
/// client fails every request; this client lives as long as the application. When it cannot
/// be created the error is logged, the application still starts, and network image sources
/// show their load-failed placeholder.
fn with_desktop_http_client(app: vespertine::Application) -> vespertine::Application {
    match vespertine_reqwest_client::desktop_http_client(HTTP_USER_AGENT) {
        Some(client) => app.with_http_client(client),
        None => app,
    }
}

/// Returns the windowed desktop application for the current OS, with the desktop HTTP client
/// installed.
pub fn application() -> vespertine::Application {
    with_desktop_http_client(vespertine::Application::with_platform(current_platform(
        false,
    )))
}

pub fn headless() -> vespertine::Application {
    vespertine::Application::with_platform(current_platform(true))
}

/// Returns a Linux app that may switch among `allowed_modes`.
///
/// It starts windowed in the process's own environment, or headless if that names no allowed
/// display server. Set another initial mode with [`vespertine::Application::with_windowing`], and
/// switch later with [`vespertine::App::request_windowing`]. Like [`application`], it installs
/// the desktop HTTP client.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn linux(allowed_modes: vespertine::WindowingModes) -> vespertine::Application {
    with_desktop_http_client(vespertine::Application::with_platform(
        vespertine_linux::linux_platform(allowed_modes),
    ))
}

/// Returns the default [`Platform`] for the current OS.
pub fn current_platform(headless: bool) -> Rc<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Rc::new(vespertine_macos::MacPlatform::new(headless))
    }

    #[cfg(target_os = "windows")]
    {
        Rc::new(
            vespertine_windows::WindowsPlatform::new(headless)
                .expect("failed to initialize Windows platform"),
        )
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        vespertine_linux::current_platform(headless)
    }
}

/// Returns the current platform's text system for benchmark contexts, built once per
/// thread and shared by every context on it.
///
/// `#[vespertine::bench]` creates a context for every Criterion routine call. Building a whole
/// platform each time only to take its text system leaks the platform's threads on Linux
/// (a 20-worker dispatcher and a timer thread per call), which slows every later
/// benchmark in the process.
#[cfg(feature = "bench-support")]
pub fn bench_text_system() -> std::sync::Arc<dyn vespertine::PlatformTextSystem> {
    thread_local! {
        static TEXT_SYSTEM: std::sync::Arc<dyn vespertine::PlatformTextSystem> =
            current_platform(true).text_system();
    }
    TEXT_SYSTEM.with(|text_system| text_system.clone())
}

/// Returns a new [`HeadlessRenderer`] for the current platform, if available.
#[cfg(any(feature = "bench-support", feature = "test-support"))]
pub fn current_headless_renderer()
-> anyhow::Result<Option<Box<dyn vespertine::PlatformHeadlessRenderer>>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Some(Box::new(
            vespertine_macos::metal_renderer::MetalHeadlessRenderer::new(),
        )))
    }

    #[cfg(target_os = "linux")]
    {
        vespertine_wgpu::WgpuHeadlessRenderer::new().map(|renderer| {
            Some(Box::new(renderer) as Box<dyn vespertine::PlatformHeadlessRenderer>)
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(None)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;
    use vespertine::{AppContext, Empty, VisualTestAppContext};

    // Note: All VisualTestAppContext tests are ignored by default because they require
    // the macOS main thread. Standard Rust tests run on worker threads, which causes
    // SIGABRT when interacting with macOS AppKit/Cocoa APIs.
    //
    // To run these tests, use:
    // cargo test -p vespertine visual_test_context -- --ignored --test-threads=1

    #[test]
    #[ignore] // Requires macOS main thread
    fn test_foreground_tasks_run_with_run_until_parked() {
        let mut cx = VisualTestAppContext::new(current_platform(false));

        let task_ran = Rc::new(RefCell::new(false));

        // Spawn a foreground task via the App's spawn method
        // This should use our TestDispatcher, not the MacDispatcher
        {
            let task_ran = task_ran.clone();
            cx.update(|cx| {
                cx.spawn(async move |_| {
                    *task_ran.borrow_mut() = true;
                })
                .detach();
            });
        }

        // The task should not have run yet
        assert!(!*task_ran.borrow());

        // Run until parked should execute the foreground task
        cx.run_until_parked();

        // Now the task should have run
        assert!(*task_ran.borrow());
    }

    #[test]
    #[ignore] // Requires macOS main thread
    fn test_advance_clock_triggers_delayed_tasks() {
        let mut cx = VisualTestAppContext::new(current_platform(false));

        let task_ran = Rc::new(RefCell::new(false));

        // Spawn a task that waits for a timer
        {
            let task_ran = task_ran.clone();
            let executor = cx.background_executor.clone();
            cx.update(|cx| {
                cx.spawn(async move |_| {
                    executor.timer(Duration::from_millis(500)).await;
                    *task_ran.borrow_mut() = true;
                })
                .detach();
            });
        }

        // Run until parked - the task should be waiting on the timer
        cx.run_until_parked();
        assert!(!*task_ran.borrow());

        // Advance clock past the timer duration
        cx.advance_clock(Duration::from_millis(600));

        // Now the task should have completed
        assert!(*task_ran.borrow());
    }

    #[test]
    #[ignore] // Requires macOS main thread - window creation fails on test threads
    fn test_window_spawn_uses_test_dispatcher() {
        let mut cx = VisualTestAppContext::new(current_platform(false));

        let task_ran = Rc::new(RefCell::new(false));

        let window = cx
            .open_offscreen_window_default(|_, cx| cx.new(|_| Empty))
            .expect("Failed to open window");

        // Spawn a task via window.spawn - this is the critical test case
        // for tooltip behavior, as tooltips use window.spawn for delayed show
        {
            let task_ran = task_ran.clone();
            cx.update_window(window.into(), |_, window, cx| {
                window
                    .spawn(cx, async move |_| {
                        *task_ran.borrow_mut() = true;
                    })
                    .detach();
            })
            .ok();
        }

        // The task should not have run yet
        assert!(!*task_ran.borrow());

        // Run until parked should execute the foreground task spawned via window
        cx.run_until_parked();

        // Now the task should have run
        assert!(*task_ran.borrow());
    }
}

#[cfg(test)]
mod http_client_tests {
    use super::*;
    use std::cell::RefCell;

    /// ELM-004 R7: an application built through this crate carries a working desktop client
    /// (identified by its user agent) instead of the framework default that fails every request.
    #[test]
    fn desktop_applications_install_the_http_client_at_startup() {
        let seen = Rc::new(RefCell::new(None));
        let sink = seen.clone();
        with_desktop_http_client(vespertine::Application::with_platform(current_platform(
            true,
        )))
        .run(move |cx| {
            cx.spawn(async move |cx| {
                cx.update(|cx| {
                    *sink.borrow_mut() = Some(
                        cx.http_client()
                            .user_agent()
                            .map(|agent| agent.to_str().unwrap().to_owned()),
                    );
                    cx.quit();
                });
            })
            .detach();
        });
        assert_eq!(
            seen.borrow().clone(),
            Some(Some(HTTP_USER_AGENT.to_owned())),
            "the startup callback ran with the desktop client installed"
        );
    }
}

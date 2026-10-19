//! The renderer: the native object a JavaScript process constructs, initializes and drives
//! (APP-005, APP-006 R3/R7 boundary).

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;

use futures::channel::oneshot;
use serde::Serialize;
use serde_json::Value;
use vespertine::{App, AppContext, Pixels, QuitMode, WindowHandle};
use vespertine::HostStyle;
use vespertine_host_tree::{HostTree, TreeOptions};

use crate::error::HostError;
use crate::link::{Reply, UiLink};
use crate::options::{ResolvedWindow, WindowConfig};
use crate::prompt::{PromptConfig, paths_to_strings};
use crate::view::{HostView, SharedTree};

/// Called from the UI thread with an event payload or an error message; it must not block.
pub type EventSink = Arc<dyn Fn(Result<Value, String>) + Send + Sync>;

/// The window a UI task operates on.
pub type HostWindow = WindowHandle<HostView>;

/// A unit of work executed on the UI thread, in submission order.
pub type UiTask = Box<dyn FnOnce(&mut App, &HostWindow) + Send>;

/// Per-edge insets in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct EdgeInsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// Safe-area, IME and effective insets (APP-005 R6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInsets {
    pub safe_area: EdgeInsets,
    pub ime: EdgeInsets,
    pub effective: EdgeInsets,
}

/// Paintable viewport in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

/// What `apply_batch` reports for an applied batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Identities destroyed, concatenated in operation order.
    pub destroyed: Vec<u64>,
    /// Whether the batch changed anything a frame would draw (a repaint was requested).
    pub repainted: bool,
}

enum Lifecycle {
    Uninitialized,
    Running(Running),
    /// The UI loop ended (the last window closed); the command channel is released.
    Stopped,
}

struct Running {
    link: UiLink<UiTask>,
    alive: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

type PromptSender = oneshot::Sender<Result<Option<Vec<String>>, HostError>>;

/// Prompts that are open and not yet answered. When the UI loop ends they are rejected, so no
/// promise is left waiting on a dialog nobody can answer.
type PendingPrompts = Arc<Mutex<Vec<std::sync::Weak<Mutex<Option<PromptSender>>>>>>;

fn answer(slot: &Mutex<Option<PromptSender>>, result: Result<Option<Vec<String>>, HostError>) {
    if let Some(sender) = lock(slot).take() {
        let _ = sender.send(result);
    }
}

/// The pending answer of a path prompt.
pub struct PromptAnswer(oneshot::Receiver<Result<Option<Vec<String>>, HostError>>);

impl PromptAnswer {
    /// Resolves to the selected absolute paths, or `None` when the user cancelled.
    pub async fn wait(self) -> Result<Option<Vec<String>>, HostError> {
        self.0
            .await
            .unwrap_or(Err(HostError::Stopped("path prompt")))
    }
}

pub struct HostRenderer {
    tree: SharedTree,
    renders: Arc<AtomicU64>,
    /// The identity window-level key events carry (the generation of the mounted root).
    generation: Arc<AtomicU64>,
    prompts: PendingPrompts,
    sink: Option<EventSink>,
    lifecycle: Mutex<Lifecycle>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

impl HostRenderer {
    pub fn new(sink: Option<EventSink>) -> Self {
        Self {
            tree: Arc::new(Mutex::new(HostTree::with_options(TreeOptions {
                style_validator: validate_style,
                ..TreeOptions::default()
            }))),
            renders: Arc::new(AtomicU64::new(0)),
            generation: Arc::new(AtomicU64::new(0)),
            prompts: PendingPrompts::default(),
            sink,
            lifecycle: Mutex::new(Lifecycle::Uninitialized),
        }
    }

    /// Attaches an externally driven UI loop in place of the native window thread. The
    /// receiver side of `link` is owned by the caller, which executes (or ignores) the tasks;
    /// `alive` is the liveness flag `tick` reports. Used to simulate UI loops in tests.
    pub fn attach_link(
        &self,
        link: UiLink<UiTask>,
        alive: Arc<AtomicBool>,
    ) -> Result<(), HostError> {
        let mut lifecycle = lock(&self.lifecycle);
        if !matches!(*lifecycle, Lifecycle::Uninitialized) {
            return Err(HostError::AlreadyInitialized);
        }
        *lifecycle = Lifecycle::Running(Running {
            link,
            alive,
            thread: None,
        });
        Ok(())
    }

    pub fn is_initialized(&self) -> bool {
        !matches!(*lock(&self.lifecycle), Lifecycle::Uninitialized)
    }

    /// Opens the window on a dedicated UI thread and waits for it to report that the window
    /// opened. At most one initialization ever succeeds.
    pub fn init(&self, config: &WindowConfig) -> Result<(), HostError> {
        let mut lifecycle = lock(&self.lifecycle);
        if !matches!(*lifecycle, Lifecycle::Uninitialized) {
            return Err(HostError::AlreadyInitialized);
        }
        let window = config
            .resolve()
            .map_err(|error| HostError::Invalid(error.0))?;

        let (link, tasks) = UiLink::<UiTask>::channel();
        let (startup_tx, startup_rx) = mpsc::channel::<Result<(), String>>();
        let alive = Arc::new(AtomicBool::new(true));
        let thread = {
            let view = (
                self.tree.clone(),
                self.renders.clone(),
                self.generation.clone(),
            );
            let sink = self.sink.clone();
            let ui_sink = self.sink.clone();
            let alive = alive.clone();
            let prompts = self.prompts.clone();
            std::thread::Builder::new()
                .name("vespertine-ui".into())
                .spawn(move || {
                    let startup = startup_tx.clone();
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        ui_main(window, view, ui_sink, tasks, startup_tx);
                    }));
                    if let Err(payload) = outcome {
                        let message = format!("The UI thread panicked: {}", panic_text(&*payload));
                        if startup.send(Err(message.clone())).is_err()
                            && let Some(sink) = &sink
                        {
                            sink(Err(message));
                        }
                    }
                    for slot in lock(&prompts).drain(..).filter_map(|slot| slot.upgrade()) {
                        answer(&slot, Err(HostError::Stopped("path prompt")));
                    }
                    alive.store(false, Ordering::SeqCst);
                })
                .map_err(|error| {
                    HostError::Failed(format!("failed to start the UI thread: {error}"))
                })?
        };

        match startup_rx.recv() {
            Ok(Ok(())) => {
                *lifecycle = Lifecycle::Running(Running {
                    link,
                    alive,
                    thread: Some(thread),
                });
                Ok(())
            }
            Ok(Err(message)) => {
                let _ = thread.join();
                *lifecycle = Lifecycle::Stopped;
                Err(HostError::Failed(message))
            }
            Err(_) => {
                let _ = thread.join();
                *lifecycle = Lifecycle::Stopped;
                Err(HostError::Failed(
                    "The UI loop exited before startup completed".into(),
                ))
            }
        }
    }

    /// Reports whether the UI loop is still running; after the last window closed it returns
    /// `false` and releases the command channel.
    pub fn tick(&self) -> Result<bool, HostError> {
        let mut lifecycle = lock(&self.lifecycle);
        match &mut *lifecycle {
            Lifecycle::Uninitialized => Err(HostError::Failed(
                "The renderer is not initialized; call init() before tick()".into(),
            )),
            Lifecycle::Stopped => Ok(false),
            Lifecycle::Running(running) => {
                if running.alive.load(Ordering::SeqCst) {
                    return Ok(true);
                }
                if let Some(thread) = running.thread.take() {
                    let _ = thread.join();
                }
                *lifecycle = Lifecycle::Stopped;
                Ok(false)
            }
        }
    }

    fn link(&self) -> Result<UiLink<UiTask>, HostError> {
        match &*lock(&self.lifecycle) {
            Lifecycle::Uninitialized => Err(HostError::NotInitialized),
            Lifecycle::Stopped => Err(HostError::NotRunning),
            Lifecycle::Running(running) => Ok(running.link.clone()),
        }
    }

    /// Queues a fire-and-forget task on the UI thread.
    pub fn post(
        &self,
        task: impl FnOnce(&mut App, &HostWindow) + Send + 'static,
    ) -> Result<(), HostError> {
        self.link()?.send(Box::new(task))
    }

    /// Runs `task` on the UI thread and returns its result, bounded by the response timeout.
    pub fn query<T: Send + 'static>(
        &self,
        operation: &'static str,
        task: impl FnOnce(&mut App, &HostWindow) -> T + Send + 'static,
    ) -> Result<T, HostError> {
        self.link()?.request(operation, move |reply: Reply<T>| {
            Box::new(move |cx: &mut App, window: &HostWindow| reply.send(task(cx, window)))
        })
    }

    pub fn window_size(&self) -> Result<WindowSize, HostError> {
        self.query("window size", |cx, window| {
            window
                .update(cx, |_, window, _| {
                    let size = window.viewport_size();
                    WindowSize {
                        width: size.width.into(),
                        height: size.height.into(),
                    }
                })
                .unwrap_or_default()
        })
    }

    pub fn window_insets(&self) -> Result<WindowInsets, HostError> {
        fn edges(edges: vespertine::Edges<Pixels>) -> EdgeInsets {
            EdgeInsets {
                top: edges.top.into(),
                right: edges.right.into(),
                bottom: edges.bottom.into(),
                left: edges.left.into(),
            }
        }
        self.query("window insets", |cx, window| {
            window
                .update(cx, |_, window, _| {
                    let insets = window.insets();
                    WindowInsets {
                        safe_area: edges(insets.safe_area),
                        ime: edges(insets.ime),
                        effective: edges(insets.effective()),
                    }
                })
                .unwrap_or_default()
        })
    }

    pub fn activate_window(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        })
    }

    pub fn minimize_window(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        })
    }

    pub fn zoom_window(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        })
    }

    pub fn toggle_fullscreen(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, window, _| window.toggle_fullscreen());
        })
    }

    /// Closes the window; with the last window closed the UI loop ends and `tick` reports
    /// `false`. An addition to the specified window controls, so programs can exit cleanly.
    pub fn close_window(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        })
    }

    pub fn set_window_title(&self, title: String) -> Result<(), HostError> {
        self.post(move |cx, window| {
            let _ = window.update(cx, |_, window, _| window.set_window_title(&title));
        })
    }

    /// Registers TrueType/OpenType font bytes with the window's text system (TXT-003). The bytes
    /// are validated before they reach the text backend; the font is then addressable by the
    /// family name declared in the font and the window is refreshed. Waits for the UI thread
    /// like every result-returning command, so an unresponsive loop yields the timeout error.
    pub fn register_font(&self, data: Vec<u8>) -> Result<(), HostError> {
        self.query("font registration", move |cx, _| {
            cx.register_font(&data)
                .map_err(|error| HostError::Failed(error.to_string()))
        })?
    }

    /// Asks the window to rebuild its elements and repaint.
    pub fn request_repaint(&self) -> Result<(), HostError> {
        self.post(|cx, window| {
            let _ = window.update(cx, |_, _, cx| cx.notify());
        })
    }

    /// Opens the platform path chooser. Validation errors and a stopped UI loop are reported
    /// here or through the answer; the UI thread is never blocked.
    pub fn prompt_for_paths(&self, config: &PromptConfig) -> Result<PromptAnswer, HostError> {
        let link = self.link()?;
        let config = config.clone();
        let (answer_tx, answer_rx) = oneshot::channel();
        let slot = Arc::new(Mutex::new(Some(answer_tx)));
        {
            let mut prompts = lock(&self.prompts);
            prompts.retain(|slot| slot.strong_count() > 0);
            prompts.push(Arc::downgrade(&slot));
        }
        link.send(Box::new(move |cx: &mut App, _window: &HostWindow| {
            let options = match config.validate(cx.can_select_mixed_files_and_dirs()) {
                Ok(options) => options,
                Err(error) => {
                    answer(&slot, Err(error));
                    return;
                }
            };
            let selection = cx.prompt_for_paths(options);
            cx.spawn(async move |_| {
                let result = match selection.await {
                    Ok(Ok(paths)) => paths_to_strings(paths),
                    Ok(Err(error)) => Err(HostError::Failed(format!("{error:#}"))),
                    Err(_) => Err(HostError::Stopped("path prompt")),
                };
                answer(&slot, result);
            })
            .detach();
        }))?;
        Ok(PromptAnswer(answer_rx))
    }

    /// Runs `operation` on the window's view on the UI thread.
    fn with_scene<T: Send + 'static>(
        &self,
        name: &'static str,
        operation: impl FnOnce(&mut HostView, &mut vespertine::Window, &mut vespertine::Context<HostView>) -> T + Send + 'static,
    ) -> Result<Option<T>, HostError> {
        self.query(name, move |cx, window| window.update(cx, operation).ok())
    }

    /// Focuses an element. Returns whether the element exists and is focusable.
    pub fn focus_element(&self, id: u64) -> Result<bool, HostError> {
        Ok(self
            .with_scene("focus", move |view, window, cx| view.scene().focus(id, window, cx))?
            .unwrap_or(false))
    }

    /// Removes focus from an element that holds it.
    pub fn blur_element(&self, id: u64) -> Result<(), HostError> {
        self.with_scene("blur", move |view, window, cx| view.scene().blur(id, window, cx))?;
        Ok(())
    }

    /// The element that holds keyboard focus, if any.
    pub fn focused_element(&self) -> Result<Option<u64>, HostError> {
        Ok(self
            .with_scene("focused element", |view, window, cx| view.scene().focused(window, cx))?
            .flatten())
    }

    /// Moves focus to the next or previous tab stop, optionally within the subtree of `scope`.
    pub fn focus_traverse(&self, scope: Option<u64>, forward: bool) -> Result<(), HostError> {
        self.with_scene("focus traversal", move |view, window, cx| {
            let tree = view.tree().clone();
            let tree = tree.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            view.scene().focus_traverse(&tree, scope, forward, window, cx);
        })?;
        Ok(())
    }

    /// The scroll offset `(x, y)` of a scrolling element (zero at the start, negative toward the
    /// far side), `None` for elements that do not scroll.
    pub fn scroll_offset(&self, id: u64) -> Result<Option<(f32, f32)>, HostError> {
        Ok(self
            .with_scene("scroll offset", move |view, _, _| view.scene().scroll_offset(id))?
            .flatten())
    }

    /// Scrolls a scrolling element to an absolute offset (positive scrolls toward the far side).
    pub fn scroll_to(&self, id: u64, x: f32, y: f32) -> Result<bool, HostError> {
        Ok(self
            .with_scene("scrolling", move |view, window, _| view.scene().scroll_to(id, x, y, window))?
            .unwrap_or(false))
    }

    /// Scrolls the nearest scrolling ancestor of an element so that the element is visible.
    pub fn scroll_into_view(&self, id: u64) -> Result<bool, HostError> {
        Ok(self
            .with_scene("scrolling into view", move |view, window, _| {
                let tree = view.tree().clone();
                let tree = tree.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                view.scene().scroll_into_view(&tree, id, window)
            })?
            .unwrap_or(false))
    }

    /// Replaces an image element's image with packed pixels.
    pub fn set_image_pixels(&self, id: u64, width: f64, height: f64, bytes: Vec<u8>, format: Option<String>) -> Result<(), HostError> {
        let result = self.with_scene("setting image pixels", move |view, window, _| {
            view.scene().set_image_pixels(id, width, height, &bytes, format.as_deref(), window)
        })?;
        result.unwrap_or(Ok(())).map_err(HostError::Failed)
    }

    /// Replaces an image element's image with encoded bytes.
    pub fn set_image(&self, id: u64, bytes: Vec<u8>) -> Result<(), HostError> {
        let result = self.with_scene("setting the image", move |view, window, cx| {
            view.scene().set_image(id, &bytes, window, cx)
        })?;
        result.unwrap_or(Ok(())).map_err(HostError::Failed)
    }

    /// Applies a mutation batch (APP-006) to the retained tree. When it changed anything a
    /// frame would draw, a repaint request is queued; if that request cannot be queued (the
    /// renderer is uninitialized or the UI loop stopped) the batch stays applied and the
    /// error is reported as [`HostError::RepaintFailed`].
    pub fn apply_batch(&self, batch: &str) -> Result<BatchOutcome, HostError> {
        let result = lock(&self.tree)
            .apply_batch(batch)
            .map_err(|error| HostError::Batch(error.to_string()))?;
        if result.redraw {
            self.request_repaint()
                .map_err(|error| HostError::RepaintFailed(Box::new(error)))?;
        }
        Ok(BatchOutcome {
            destroyed: result.destroyed,
            repainted: result.redraw,
        })
    }

    /// How many frames the window's view has built its elements for since construction.
    pub fn render_count(&self) -> u64 {
        self.renders.load(Ordering::Relaxed)
    }

    /// A root view drawing this renderer's retained tree (for embedders and tests that
    /// supply their own window).
    pub fn new_view(&self) -> HostView {
        HostView::new(
            self.tree.clone(),
            self.renders.clone(),
            self.sink.clone(),
            self.generation.clone(),
        )
    }

    /// Announces the generation of the mounted root: window-level key events carry it as their
    /// identity so events of a replaced root are rejected by the JavaScript side.
    pub fn set_window_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::SeqCst);
    }

    /// The retained tree shared with the window's view.
    pub fn tree(&self) -> SharedTree {
        self.tree.clone()
    }

    /// Reads the retained tree.
    pub fn with_tree<T>(&self, read: impl FnOnce(&HostTree) -> T) -> T {
        read(&lock(&self.tree))
    }
}

/// The UI thread: creates the application, opens the window, reports startup and serves
/// queued tasks until the last window closes.
fn ui_main(
    window: ResolvedWindow,
    (tree, renders, generation): (SharedTree, Arc<AtomicU64>, Arc<AtomicU64>),
    sink: Option<EventSink>,
    tasks: async_channel::Receiver<UiTask>,
    startup: mpsc::Sender<Result<(), String>>,
) {
    let app = vespertine_platform::application().with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx| {
        let options = window.to_window_options(cx);
        let handle = match cx.open_window(options, |_, cx| cx.new(|_| HostView::new(tree, renders, sink, generation)))
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = startup.send(Err(format!("failed to open the window: {error:#}")));
                cx.quit();
                return;
            }
        };
        if window.focus {
            cx.activate(true);
        }
        cx.spawn(async move |cx| {
            while let Ok(task) = tasks.recv().await {
                let _ = cx.update(|cx| task(cx, &handle));
            }
        })
        .detach();
        let _ = startup.send(Ok(()));
    });
}

/// The style schema of the retained tree: the whole payload must decode as a [`HostStyle`].
fn validate_style(value: &Value) -> Result<(), String> {
    HostStyle::from_json(value, "setStyle")
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

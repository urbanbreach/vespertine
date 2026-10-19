//! The offscreen test host: construction, frames, executor time and the motion clock.

use crate::{
    HostError, HostResult, PaintLog,
    events::HostEvents,
    root::{Content, ElementView, HostRoot},
    tree::RetainedHost,
    types::{NativeEvent, Viewport, logical_size},
};
use std::{
    path::Path,
    rc::Rc,
    sync::{Arc, LazyLock},
    time::Duration,
};
use vespertine::{
    AnyView, AnyWindowHandle, App, AppContext as _, AssetSource, Bounds, Entity,
    HeadlessAppContext, MotionClock, PaintObserver, PlatformHeadlessRenderer, PlatformTextSystem,
    Render, RequestFrameOptions, Scene, Window, WindowBounds, WindowOptions, point, px,
};

/// Default viewport width in logical pixels when none is given.
pub const DEFAULT_WIDTH: f64 = 1280.0;
/// Default viewport height in logical pixels when none is given.
pub const DEFAULT_HEIGHT: f64 = 800.0;
/// Native placement of the offscreen window, far outside any visible output.
pub const OFFSCREEN_ORIGIN: f64 = -10000.0;
/// The default window scale factor of the test platform.
pub const DEFAULT_SCALE_FACTOR: f32 = 2.0;
/// The most frames one [`TestHost::flush`] will draw while the window keeps invalidating itself
/// (for example under a perpetual animation).
pub const MAX_FRAMES_PER_FLUSH: usize = 8;

/// Creates the renderer a host window renders with.
pub type RendererFactory =
    Box<dyn Fn() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>>>;

/// Receives every native event the host drains after an input.
pub type EventDispatcher = Box<dyn FnMut(NativeEvent)>;

/// Observes the scene of every frame the host draws, as handed to the renderer.
pub type SceneObserver = Box<dyn FnMut(&Scene)>;

/// Wraps a renderer so that every scene it is given is shown to an observer first.
struct ObservingRenderer {
    inner: Box<dyn PlatformHeadlessRenderer>,
    observer: SceneObserver,
}

impl PlatformHeadlessRenderer for ObservingRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: vespertine::Size<vespertine::DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.inner.render_scene_to_image(scene, size)
    }

    fn render_scene(
        &mut self,
        scene: &Scene,
        size: vespertine::Size<vespertine::DevicePixels>,
    ) -> anyhow::Result<()> {
        (self.observer)(scene);
        self.inner.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn vespertine::PlatformAtlas> {
        self.inner.sprite_atlas()
    }
}

/// Construction options of a [`TestHost`].
pub struct HostOptions {
    /// Window width in logical pixels; a positive finite number that fits in 32-bit signed
    /// geometry. [`DEFAULT_WIDTH`] when `None`.
    pub width: Option<f64>,
    /// Window height in logical pixels, with the same constraints. [`DEFAULT_HEIGHT`] when
    /// `None`.
    pub height: Option<f64>,
    /// Window scale factor; [`DEFAULT_SCALE_FACTOR`] by default.
    pub scale_factor: f32,
    /// Where drained native events go after every input. Without one, events are drained and
    /// dropped by input helpers; set one with [`TestHost::set_event_dispatcher`].
    pub event_dispatcher: Option<EventDispatcher>,
    /// A custom renderer factory. By default the host renders with the platform's production
    /// GPU renderer; a factory that fails makes construction fail.
    pub renderer: Option<RendererFactory>,
    /// Called with the scene of every frame the host draws (quads, sprites, paint operations),
    /// before it is rendered.
    pub scene_observer: Option<SceneObserver>,
    /// An asset source for the application (icons, images).
    pub assets: Option<Arc<dyn AssetSource>>,
    /// Bypass retained-view reuse for every host-drawn frame, so that every element painted in
    /// the frame reports its geometry and text into the paint log. On by default.
    pub complete_frames: bool,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            scale_factor: DEFAULT_SCALE_FACTOR,
            event_dispatcher: None,
            renderer: None,
            scene_observer: None,
            assets: None,
            complete_frames: true,
        }
    }
}

impl HostOptions {
    /// Options with a viewport size in logical pixels.
    pub fn with_size(width: f64, height: f64) -> Self {
        Self {
            width: Some(width),
            height: Some(height),
            ..Self::default()
        }
    }
}

fn validate_dimension(
    dimension: &'static str,
    value: Option<f64>,
    default: f64,
) -> HostResult<f64> {
    match value {
        None => Ok(default),
        Some(value) if value.is_finite() && value > 0.0 && value <= f64::from(i32::MAX) => {
            Ok(value)
        }
        Some(value) => Err(HostError::InvalidSize { dimension, value }),
    }
}

fn default_renderer() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
    vespertine_platform::current_headless_renderer()
}

fn text_system() -> Arc<dyn PlatformTextSystem> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(vespertine_wgpu::CosmicTextSystem::new("IBM Plex Sans"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        vespertine_platform::current_platform(true).text_system()
    }
}

static PROBE: LazyLock<Result<(), String>> = LazyLock::new(|| match default_renderer() {
    Ok(Some(_renderer)) => Ok(()),
    Ok(None) => Err("no GPU test backend is built for this platform".to_owned()),
    Err(error) => Err(format!("{error:#}")),
});

/// Whether a working GPU test backend was built and can render on this machine. Querying it does
/// not construct a host. When it is `false`, [`TestHost`] construction fails with
/// [`HostError::BackendUnavailable`]; host-dependent tests should skip (see
/// [`require_backend!`](crate::require_backend)), never fake a result.
pub fn is_available() -> bool {
    PROBE.is_ok()
}

/// Why the GPU test backend is unavailable, or `None` when it is available.
pub fn unavailable_reason() -> Option<String> {
    PROBE.as_ref().err().cloned()
}

/// An offscreen window driven through the production retained-tree, layout, hit-testing,
/// event-dispatch and GPU painting paths. See the [crate documentation](crate).
pub struct TestHost {
    // Field order is drop order: the root entity handle is released before the application
    // context that owns it (see `Drop`).
    pub(crate) root: Entity<HostRoot>,
    pub(crate) cx: HeadlessAppContext,
    pub(crate) window: AnyWindowHandle,
    pub(crate) log: PaintLog,
    pub(crate) events: HostEvents,
    pub(crate) dispatcher: Option<EventDispatcher>,
}

fn window_error(error: impl std::fmt::Display) -> HostError {
    HostError::Window(format!("{error:#}"))
}

impl TestHost {
    /// A host with an empty root view. Use [`TestHost::with_tree`] or [`TestHost::with_view`]
    /// to host content, or this to test construction and geometry.
    pub fn new(width: Option<f64>, height: Option<f64>) -> HostResult<Self> {
        Self::build(
            HostOptions {
                width,
                height,
                ..HostOptions::default()
            },
            |_window, _cx| None,
        )
    }

    /// A host rendering a retained host tree.
    pub fn with_tree(options: HostOptions, tree: impl RetainedHost) -> HostResult<Self> {
        Self::build(options, move |_window, _cx| {
            Some(Content::Tree(Box::new(tree)))
        })
    }

    /// A host rendering a native view. The view is built inside the offscreen window.
    pub fn with_view<V: Render + 'static>(
        options: HostOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> HostResult<Self> {
        Self::build(options, move |window, cx| {
            Some(Content::View(AnyView::from(build(window, cx))))
        })
    }

    /// A host rendering an element built by `element` on every frame, for pixel tests of
    /// element trees that need no view state.
    pub fn with_element(
        options: HostOptions,
        element: impl Fn(&mut Window, &mut App) -> vespertine::AnyElement + 'static,
    ) -> HostResult<Self> {
        Self::with_view(options, move |_window, cx| {
            cx.new(|_| ElementView(Box::new(element)))
        })
    }

    fn build(
        options: HostOptions,
        content: impl FnOnce(&mut Window, &mut App) -> Option<Content>,
    ) -> HostResult<Self> {
        let width = validate_dimension("width", options.width, DEFAULT_WIDTH)?;
        let height = validate_dimension("height", options.height, DEFAULT_HEIGHT)?;

        let factory = options
            .renderer
            .unwrap_or_else(|| Box::new(default_renderer));
        let observer = std::cell::RefCell::new(options.scene_observer);
        let factory = move || match factory() {
            Ok(Some(renderer)) => Ok(Some(match observer.borrow_mut().take() {
                Some(observer) => Box::new(ObservingRenderer {
                    inner: renderer,
                    observer,
                }) as Box<dyn PlatformHeadlessRenderer>,
                None => renderer,
            })),
            Ok(None) => Err(anyhow::anyhow!(
                "no GPU test backend is built for this platform"
            )),
            Err(error) => Err(error),
        };
        let mut cx = HeadlessAppContext::with_platform(
            text_system(),
            options.assets.unwrap_or_else(|| Arc::new(())),
            factory,
        );

        let log = PaintLog::default();
        let events = HostEvents::default();
        cx.update(|cx| {
            cx.set_global(log.clone());
            cx.set_global(events.clone());
            cx.set_global(MotionClock::default());
            // The log is cleared exactly once at the start of every frame the window draws, by
            // whoever triggers the draw (this host, the effect loop of the test platform or the
            // initial paint of a new window); with `complete_frames` the frame also bypasses
            // retained paint state so every painted element reports itself.
            let text_log = log.clone();
            let frame_log = log.clone();
            let complete_frames = options.complete_frames;
            cx.set_global(PaintObserver {
                on_text: Rc::new(move |painted| text_log.push_text(&painted.text)),
                on_frame_start: Some(Rc::new(move |window, _cx| {
                    frame_log.begin_frame();
                    if complete_frames {
                        window.refresh();
                    }
                })),
            });
        });

        let bounds = Bounds {
            origin: point(px(OFFSCREEN_ORIGIN as f32), px(OFFSCREEN_ORIGIN as f32)),
            size: logical_size(width, height),
        };
        let window = cx
            .open_window_with_options(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: true,
                    show: false,
                    ..WindowOptions::default()
                },
                |window, cx| {
                    let content = content(window, cx);
                    cx.new(|_| HostRoot { content })
                },
            )
            .map_err(|error| HostError::BackendUnavailable(format!("{error:#}")))?;
        let root = window.root(&mut cx).map_err(window_error)?;
        let window: AnyWindowHandle = window.into();
        cx.update_window(window, |_, window, _| window.activate_window())
            .map_err(window_error)?;
        if (options.scale_factor - DEFAULT_SCALE_FACTOR).abs() > f32::EPSILON {
            cx.simulate_window_scale_factor(window, options.scale_factor)
                .map_err(window_error)?;
        }

        let mut host = Self {
            root,
            cx,
            window,
            log,
            events,
            dispatcher: options.event_dispatcher,
        };
        host.draw_frame()?;
        host.flush();
        Ok(host)
    }

    /// The actual viewport in logical pixels.
    pub fn viewport(&mut self) -> Viewport {
        let size = self
            .cx
            .update_window(self.window, |_, window, _| window.viewport_size())
            .expect("the host window is open for the host's lifetime");
        Viewport {
            width: f64::from(f32::from(size.width)),
            height: f64::from(f32::from(size.height)),
        }
    }

    /// The window scale factor.
    pub fn scale_factor(&mut self) -> f32 {
        self.cx
            .window_scale_factor(self.window)
            .expect("the host window is open for the host's lifetime")
    }

    /// The application context, for tests that build entities, set globals or read application
    /// state beside the host.
    pub fn app(&mut self) -> &mut HeadlessAppContext {
        &mut self.cx
    }

    /// The handle of the offscreen window.
    pub fn window(&self) -> AnyWindowHandle {
        self.window
    }

    /// The paint log of the last drawn frame.
    pub fn paint_log(&self) -> &PaintLog {
        &self.log
    }

    /// Replaces the dispatcher that receives drained native events.
    pub fn set_event_dispatcher(&mut self, dispatcher: Option<EventDispatcher>) {
        self.dispatcher = dispatcher;
    }

    /// Runs `f` with the root view's retained tree and the window.
    pub(crate) fn on_tree<R>(
        &mut self,
        f: impl FnOnce(&mut dyn RetainedHost, &mut Window, &mut App) -> R,
    ) -> HostResult<R> {
        let root = self.root.clone();
        self.cx
            .update_window(self.window, move |_, window, cx| {
                root.update(cx, |root, cx| match root.content.as_mut() {
                    Some(Content::Tree(tree)) => Ok(f(tree.as_mut(), window, cx)),
                    _ => Err(HostError::Unsupported(
                        "this host has no retained tree".to_owned(),
                    )),
                })
            })
            .map_err(window_error)?
    }

    /// Applies one serialized batch to the retained tree exactly as a live window would, and
    /// returns the identities of the nodes the batch destroyed so the framework adapter can
    /// unregister their handlers. The next frame is requested; call [`TestHost::flush`] (or any
    /// input or capture helper) to lay out and paint it.
    pub fn apply_batch(&mut self, batch: &str) -> HostResult<Vec<u64>> {
        let destroyed = self
            .on_tree(|tree, _window, _cx| tree.apply_batch(batch))?
            .map_err(HostError::Batch)?;
        self.invalidate();
        Ok(destroyed)
    }

    /// Marks the root view changed so the next frame rebuilds and repaints the retained tree.
    pub fn invalidate(&mut self) {
        let root = self.root.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                root.update(cx, |_, cx| cx.notify());
                window.refresh();
            })
            .expect("the host window is open for the host's lifetime");
    }

    /// Forces one frame to be laid out and painted even if nothing invalidated the window, then
    /// drains the executor. Selection listeners (and other paint-scoped listeners) are installed
    /// during paint, so interaction helpers paint a frame between gesture steps with this.
    pub fn paint_frame(&mut self) {
        self.invalidate();
        self.flush();
    }

    /// Draws one frame through the window's production frame callback: the window lays out,
    /// paints and presents to the GPU renderer. The paint log is cleared by the frame-start
    /// hook installed at construction.
    fn draw_frame(&mut self) -> HostResult<()> {
        self.cx
            .request_frame(
                self.window,
                RequestFrameOptions {
                    require_presentation: true,
                    force_render: false,
                    ..RequestFrameOptions::default()
                },
            )
            .map_err(window_error)
    }

    /// Requests a render of the retained tree and drains the deterministic executor.
    ///
    /// Runs every foreground task and timer that is due at the current deterministic time, then
    /// draws a frame if the window was invalidated, repeating until nothing is runnable and the
    /// window is clean (at most [`MAX_FRAMES_PER_FLUSH`] frames).
    pub fn flush(&mut self) {
        for _ in 0..MAX_FRAMES_PER_FLUSH {
            self.cx.run_until_parked();
            let dirty = self
                .cx
                .window_needs_draw(self.window)
                .expect("the host window is open for the host's lifetime");
            if !dirty {
                return;
            }
            self.draw_frame()
                .expect("the host window is open for the host's lifetime");
        }
        self.cx.run_until_parked();
    }

    /// Takes the native events produced since the last drain, in emission order.
    pub fn drain_events(&mut self) -> Vec<NativeEvent> {
        self.events.drain()
    }

    /// Drains the native events and hands each to the active dispatcher, in order. Returns the
    /// number of events dispatched; without a dispatcher the events are dropped and counted.
    pub fn dispatch_native_events(&mut self) -> usize {
        let events = self.drain_events();
        let count = events.len();
        if let Some(dispatcher) = self.dispatcher.as_mut() {
            for event in events {
                dispatcher(event);
            }
        }
        count
    }

    /// Reads back the GPU-rendered frame of the offscreen window after a flush. The image is
    /// `viewport * scale factor` pixels. Fails explicitly if readback is impossible.
    pub fn screenshot_image(&mut self) -> HostResult<image::RgbaImage> {
        self.flush();
        self.cx
            .capture_screenshot(self.window)
            .map_err(|error| HostError::Screenshot(format!("{error:#}")))
    }

    /// Flushes, reads back the GPU-rendered frame and saves it as a PNG at `path`.
    pub fn screenshot(&mut self, path: impl AsRef<Path>) -> HostResult<()> {
        let image = self.screenshot_image()?;
        let path = path.as_ref();
        image
            .save_with_format(path, image::ImageFormat::Png)
            .map_err(|error| HostError::Screenshot(format!("{}: {error}", path.display())))
    }

    /// Freezes the motion clock at the current instant and returns it in milliseconds.
    pub fn motion_pause(&mut self) -> f64 {
        self.cx.update(MotionClock::pause)
    }

    /// Freezes the motion clock at the absolute elapsed time `ms`.
    pub fn motion_set(&mut self, ms: f64) -> HostResult<f64> {
        check_duration(ms)?;
        Ok(self.cx.update(|cx| MotionClock::set(cx, ms)))
    }

    /// Adds `delta_ms` to the motion clock's current instant and freezes it there.
    pub fn motion_fast_forward(&mut self, delta_ms: f64) -> HostResult<f64> {
        check_duration(delta_ms)?;
        Ok(self.cx.update(|cx| MotionClock::fast_forward(cx, delta_ms)))
    }

    /// Resumes the motion clock from its frozen value.
    pub fn motion_resume(&mut self) -> f64 {
        self.cx.update(MotionClock::resume)
    }

    /// The motion clock's current reading in milliseconds.
    pub fn motion_now(&mut self) -> f64 {
        self.cx.update(|cx| MotionClock::now_ms(cx))
    }

    /// Advances deterministic executor time by `ms` milliseconds, running the timers and
    /// delayed tasks that become due. This is independent of the motion clock, which it never
    /// changes. Rejects negative, NaN and infinite values.
    pub fn advance_executor_time(&mut self, ms: f64) -> HostResult<()> {
        check_duration(ms)?;
        let duration =
            Duration::try_from_secs_f64(ms / 1000.0).map_err(|_| HostError::InvalidDuration(ms))?;
        self.cx.advance_clock(duration);
        self.flush();
        Ok(())
    }

    /// Registers a font from its bytes with the application's text system.
    pub fn register_font(&mut self, bytes: Vec<u8>) -> HostResult<()> {
        ttf_parser::Face::parse(&bytes, 0)
            .map_err(|error| HostError::InvalidFont(error.to_string()))?;
        self.cx
            .text_system()
            .add_fonts(vec![std::borrow::Cow::Owned(bytes)])
            .map_err(|error| HostError::InvalidFont(format!("{error:#}")))?;
        self.invalidate();
        Ok(())
    }
}

pub(crate) fn check_duration(ms: f64) -> HostResult<()> {
    if ms.is_finite() && ms >= 0.0 {
        Ok(())
    } else {
        Err(HostError::InvalidDuration(ms))
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        // Release custom-element state and view-owned entities (the root's content) and close
        // the window before the application context is destroyed, so teardown never leaves
        // dangling entities.
        let root = self.root.clone();
        let _ = self.cx.update_window(self.window, |_, window, cx| {
            root.update(cx, |root, _| root.content = None);
            window.remove_window();
        });
        self.cx.run_until_parked();
    }
}

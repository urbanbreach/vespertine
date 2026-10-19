//! Shared scaffolding of the PLT-001 examples: a window with a scripted self-check driven
//! through the window's own input path.

#![allow(dead_code)]

use std::{rc::Rc, time::Duration};

use vespertine::{
    App, AppContext as _, AsyncApp, Bounds, CompositionLayout, CompositionStats, Entity, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NativePointerEvent, Pixels,
    PlatformInput, Point, Render, ScrollDelta, ScrollWheelEvent, SurfaceId, Window, WindowBounds,
    WindowHandle, WindowOptions, point, px, size,
};

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    }
}

/// The scripted checks of one example run.
pub struct Script<V: 'static> {
    pub handle: WindowHandle<V>,
    pub cx: AsyncApp,
    pub failures: Vec<String>,
}

impl<V: Render + 'static> Script<V> {
    pub fn check(&mut self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }

    pub async fn sleep(&self, millis: u64) {
        self.cx
            .background_executor()
            .timer(Duration::from_millis(millis))
            .await;
    }

    fn dispatch(&self, event: PlatformInput) {
        self.cx
            .clone()
            .update_window(self.handle.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("window");
    }

    pub fn mouse_move(&self, position: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::none(),
        }));
    }

    pub fn press(&self, position: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        }));
    }

    pub fn release(&self, position: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::none(),
            click_count: 1,
        }));
    }

    pub fn click(&self, position: Point<Pixels>) {
        self.mouse_move(position);
        self.press(position);
        self.release(position);
    }

    pub fn wheel(&self, position: Point<Pixels>, dy: f32) {
        self.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: vespertine::TouchPhase::Moved,
        }));
    }

    /// Updates the view and requests a frame.
    pub fn set(&self, f: impl FnOnce(&mut V, &mut Window) + 'static) {
        self.handle
            .update(&mut self.cx.clone(), |view, window, cx| {
                f(view, window);
                cx.notify();
            })
            .expect("window");
    }

    pub fn read<R: 'static>(&self, f: impl FnOnce(&V) -> R) -> R {
        self.handle
            .read_with(&self.cx, |view, _| f(view))
            .expect("window")
    }

    /// The composition counters of the window.
    pub fn stats(&self) -> CompositionStats {
        self.cx
            .clone()
            .update_window(self.handle.into(), |_, window, _| {
                window.composition_stats().expect("composition is enabled")
            })
            .expect("window")
    }

    /// The flattened surface tree, bottom to top.
    pub fn layout(&self) -> CompositionLayout {
        self.cx
            .clone()
            .update_window(self.handle.into(), |_, window, _| {
                window
                    .composition()
                    .expect("composition is enabled")
                    .layout()
            })
            .expect("window")
    }

    /// The stacking position of a surface in the tree, bottom first.
    pub fn position_of(&self, surface: SurfaceId) -> usize {
        self.layout()
            .position(surface)
            .expect("the surface is in the tree")
    }

    /// Waits until `condition` holds (polling the view), up to ten seconds.
    pub async fn until(&self, condition: impl Fn(&V) -> bool + Clone + 'static) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if self.read(condition.clone()) {
                return true;
            }
            self.sleep(20).await;
        }
        false
    }
}

/// Opens a window of the given logical size showing the view `build` makes, runs `script` once
/// it is up and exits with 0 when every check passed (1 otherwise). With `NATIVE_INTERACTIVE`
/// set the window stays open instead.
pub fn launch<V, S, F>(
    width: f32,
    height: f32,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
    script: S,
) where
    V: Render + 'static,
    S: FnOnce(Script<V>) -> F + 'static,
    F: Future<Output = Vec<String>> + 'static,
{
    let interactive = std::env::var_os("NATIVE_INTERACTIVE").is_some();
    vespertine::Application::with_platform(vespertine_linux::current_platform(false)).run(
        move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    build,
                )
                .expect("open the window");
            cx.activate(true);
            if interactive {
                return;
            }
            cx.spawn(async move |cx| {
                // Let the first frames present.
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let failures = script(Script {
                    handle,
                    cx: cx.clone(),
                    failures: Vec::new(),
                })
                .await;
                let code = i32::from(!failures.is_empty());
                if code != 0 {
                    eprintln!("failed checks: {failures:?}");
                }
                std::process::exit(code);
            })
            .detach();
        },
    );
}

/// What a cooperating producer saw.
#[derive(Default)]
pub struct Seen {
    pub events: Vec<NativePointerEvent>,
    pub wheels: Vec<ScrollWheelEvent>,
}

pub type SharedSeen = Rc<std::cell::RefCell<Seen>>;

pub fn downs(seen: &SharedSeen) -> Vec<Point<Pixels>> {
    seen.borrow()
        .events
        .iter()
        .filter_map(|event| match event {
            NativePointerEvent::Down(down) => Some(down.position),
            _ => None,
        })
        .collect()
}

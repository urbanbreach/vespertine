//! Headless window harness with real shaping, input dispatch and screenshots.
#![allow(dead_code)]

use image::RgbaImage;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, AppContext as _, Context, HeadlessAppContext, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point,
    Render, Window, WindowHandle, point, px, size,
};
use vespertine_richtext::TextService;

pub type Build = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

pub struct Doc {
    build: Build,
}

impl Render for Doc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

pub struct Harness {
    pub cx: HeadlessAppContext,
    pub window: WindowHandle<Doc>,
}

pub fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

impl Harness {
    pub fn new(width: f32, height: f32, build: Build) -> Self {
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans")),
            Arc::new(()),
            vespertine_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(width), px(height)), |_, cx| {
                cx.new(|_| Doc { build })
            })
            .unwrap();
        let mut harness = Self { cx, window };
        harness.draw();
        harness
    }

    pub fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.draw(cx).clear(cx)
            })
            .expect("draw");
        self.cx.run_until_parked();
    }

    /// Draws a frame that constructs and paints every view. An unchanged view replays its
    /// retained output (PRF-001), which neither paints runs nor finishes the text service's
    /// frame, so a test that changes state the views read without notifying them, or that
    /// needs the runs to paint, redraws with this.
    pub fn redraw(&mut self) {
        self.cx
            .update_window(self.window.into(), |_, window, _| window.refresh())
            .expect("refresh");
        self.draw();
    }

    /// Redraws until the frame is stable (programmatic selections apply at the end of a frame
    /// in which the runs paint).
    pub fn settle(&mut self) {
        for _ in 0..3 {
            self.redraw();
        }
    }

    pub fn service(&mut self) -> TextService {
        let window = self.window;
        self.cx
            .update_window(window.into(), |_, window, cx| TextService::of(window, cx))
            .unwrap()
    }

    fn dispatch(&mut self, input: PlatformInput) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(input, cx);
            })
            .unwrap();
        self.draw();
    }

    pub fn move_to(&mut self, position: Point<Pixels>, pressed: Option<MouseButton>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: pressed,
            modifiers: Modifiers::default(),
        }));
    }

    pub fn press(&mut self, position: Point<Pixels>, button: MouseButton, click_count: usize) {
        self.move_to(position, None);
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        }));
    }

    pub fn release(&mut self, position: Point<Pixels>, button: MouseButton, click_count: usize) {
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
        }));
    }

    pub fn click(&mut self, position: Point<Pixels>) {
        self.press(position, MouseButton::Left, 1);
        self.release(position, MouseButton::Left, 1);
    }

    pub fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>) {
        self.press(from, MouseButton::Left, 1);
        self.move_to(to, Some(MouseButton::Left));
        self.release(to, MouseButton::Left, 1);
    }

    /// Dispatches a key press; returns whether the event propagated past every listener.
    pub fn key(&mut self, keystroke: &str) -> bool {
        let keystroke = vespertine::Keystroke::parse(keystroke).unwrap();
        let input = PlatformInput::KeyDown(vespertine::KeyDownEvent {
            keystroke,
            is_held: false,
            prefer_character_input: false,
        });
        let propagated = self
            .cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(input, cx).propagate
            })
            .unwrap();
        self.draw();
        propagated
    }

    /// What an application's Tab binding does: move focus to the next tab stop.
    pub fn focus_next(&mut self) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| window.focus_next(cx))
            .unwrap();
        self.draw();
    }

    pub fn screenshot(&mut self) -> RgbaImage {
        self.draw();
        self.cx
            .capture_screenshot(self.window.into())
            .expect("screenshot")
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.cx
            .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }
}

/// The pixel at logical position `(x, y)` of a screenshot taken for a `width`-wide window.
pub fn rgb_at(image: &RgbaImage, width: f32, x: f32, y: f32) -> [u8; 3] {
    let scale = image.width() as f32 / width;
    let pixel = image.get_pixel((x * scale) as u32, (y * scale) as u32);
    [pixel[0], pixel[1], pixel[2]]
}

pub fn near(actual: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
    actual
        .into_iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

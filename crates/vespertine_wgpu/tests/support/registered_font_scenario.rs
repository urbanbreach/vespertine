//! TXT-003 scenario shared by `examples/registered_font.rs` and `tests/fonts.rs`.
//!
//! Registers a synthetic font in which `A` is one em wide and `B` two ems wide, then checks the
//! width of `AB` at 20 px before and after registration, in the order of events the spec lists.

use std::sync::Arc;
use vespertine::{
    AnyWindowHandle, Context, FontRegistrationError, HeadlessAppContext, Render, TextRun, Window,
    black, div, font, prelude::*, px, size, test_font::TestFontBuilder,
};
use vespertine_wgpu::CosmicTextSystem;

const FAMILY: &str = "Registered Sample Family";
const TEXT: &str = "AB";
const FONT_SIZE: f32 = 20.0;
/// `A` is 1 em and `B` is 2 ems wide.
const REGISTERED_WIDTH: f32 = 3.0 * FONT_SIZE;

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn headless() -> (HeadlessAppContext, AnyWindowHandle) {
    // The system fonts supply the fallback a missing family resolves to.
    let mut cx = HeadlessAppContext::new(Arc::new(CosmicTextSystem::new("Noto Sans")));
    let window = cx
        .open_window(size(px(400.), px(200.)), |_, app| app.new(|_| Blank))
        .expect("headless window opens");
    (cx, window.into())
}

/// The width of `AB` at 20 px shaped through the window's text system.
fn width(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> f32 {
    cx.update_window(window, |_, window, _| {
        let run = TextRun {
            strikethrough: None,
            underline: None,
            background_color: None,
            color: black(),
            font: font(FAMILY),
            len: TEXT.len(),
            letter_spacing: px(0.),
        };
        let line = window
            .text_system()
            .shape_line(TEXT.into(), px(FONT_SIZE), &[run], None);
        f32::from(line.width)
    })
    .expect("window is open")
}

fn assert_registered(width: f32) {
    assert!(
        (width - REGISTERED_WIDTH).abs() <= 1.0,
        "registered width {width} is not within 1 px of {REGISTERED_WIDTH}"
    );
}

pub fn run() {
    let fixture = TestFontBuilder::new(FAMILY).build();

    // The family is requested before its font exists and falls back.
    let (mut cx, window) = headless();
    let fallback = width(&mut cx, window);
    assert!(
        (fallback - REGISTERED_WIDTH).abs() > 5.0,
        "the fallback width {fallback} must differ from the registered width"
    );

    // A rejected buffer returns an error and leaves the runtime working.
    let rejected = cx.update(|app| app.register_font(&[1, 2, 3, 4]));
    assert!(matches!(rejected, Err(FontRegistrationError::NotAFont)));
    assert_eq!(width(&mut cx, window), fallback);

    // Registering after the earlier failed lookup makes the family resolvable.
    cx.update(|app| app.register_font(&fixture))
        .expect("the fixture is a valid font");
    assert_registered(width(&mut cx, window));

    // Registering before any text asks for the family gives the same width, even
    // with a rejected buffer in between.
    let (mut cx, window) = headless();
    cx.update(|app| app.register_font(&fixture))
        .expect("the fixture is a valid font");
    let rejected = cx.update(|app| app.register_font(&[1, 2, 3, 4]));
    assert!(matches!(rejected, Err(FontRegistrationError::NotAFont)));
    assert_registered(width(&mut cx, window));
}

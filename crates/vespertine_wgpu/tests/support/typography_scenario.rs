//! TXT-004 scenario shared by `examples/typography.rs` and `tests/fonts.rs`.
//!
//! Three samples (`Interface 0123` at the interface size, `Code 0123` at 12.5 px and
//! `Terminal 0123` at 13 px) are rendered with embedded proportional and fixed-width families.
//! Changing the interface size from 16 to 20 px must scale only the interface sample.

use std::{borrow::Cow, sync::Arc};
use vespertine::{
    AnyWindowHandle, Context, EmbeddedFontDefault, EmbeddedFontFamily, FontCatalog,
    HeadlessAppContext, Render, TextRun, Typography, TypographyRole, TypographySettings, Window,
    black, div, font, prelude::*, px, rems, size, test_font::TestFontBuilder,
};
use vespertine_wgpu::CosmicTextSystem;

const INTERFACE_TEXT: &str = "Interface 0123";
const CODE_TEXT: &str = "Code 0123";
const TERMINAL_TEXT: &str = "Terminal 0123";
const INTERFACE_LINE_HEIGHT: f32 = 24.0;

struct Samples;

impl Render for Samples {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = Typography::global(cx);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .font_family(typography.effective_family_name(TypographyRole::Interface))
                    .text_size(rems(1.))
                    .line_height(px(INTERFACE_LINE_HEIGHT))
                    .child(INTERFACE_TEXT),
            )
            .child(
                div()
                    .font_family(typography.effective_family_name(TypographyRole::Code))
                    .text_size(px(typography.code_size()))
                    .child(CODE_TEXT),
            )
            .child(
                div()
                    .font_family(typography.effective_family_name(TypographyRole::Terminal))
                    .text_size(px(typography.terminal_size()))
                    .child(TERMINAL_TEXT),
            )
    }
}

fn embedded_families() -> Vec<EmbeddedFontFamily> {
    vec![
        EmbeddedFontFamily::new(
            "sample-sans",
            vec![Cow::Owned(
                TestFontBuilder::proportional("Sample Sans").build(),
            )],
        )
        .with_default(EmbeddedFontDefault::Interface),
        EmbeddedFontFamily::new(
            "sample-mono",
            vec![
                Cow::Owned(TestFontBuilder::monospace("Sample Mono", 600).build()),
                Cow::Owned(
                    TestFontBuilder::monospace("Sample Mono", 600)
                        .italic(true)
                        .subfamily("Italic")
                        .build(),
                ),
            ],
        )
        .with_default(EmbeddedFontDefault::FixedWidth),
    ]
}

/// The widths of the three samples, shaped the way [`Samples`] styles them.
struct Widths {
    interface: f32,
    code: f32,
    terminal: f32,
}

fn measure(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Widths {
    cx.update_window(window, |_, window, app| {
        let typography = Typography::global(app);
        let shape = |role: TypographyRole, text: &str, font_size| {
            let run = TextRun {
                len: text.len(),
                font: font(typography.effective_family_name(role)),
                color: black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line =
                window
                    .text_system()
                    .shape_line(text.to_string().into(), font_size, &[run], None);
            f32::from(line.width)
        };
        Widths {
            interface: shape(
                TypographyRole::Interface,
                INTERFACE_TEXT,
                typography.rem_size(),
            ),
            code: shape(TypographyRole::Code, CODE_TEXT, px(typography.code_size())),
            terminal: shape(
                TypographyRole::Terminal,
                TERMINAL_TEXT,
                px(typography.terminal_size()),
            ),
        }
    })
    .expect("window is open")
}

fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, app| window.draw(app).clear(app))
        .expect("window draws");
}

pub fn run() {
    let mut cx = HeadlessAppContext::new(Arc::new(CosmicTextSystem::new_without_system_fonts(
        "Sample Sans",
    )));
    let catalog = cx.update(|app| FontCatalog::register(app, &embedded_families()));
    assert!(catalog.embedded_entries().all(|entry| entry.is_available()));
    assert_eq!(
        catalog.fixed_width_entries().count(),
        1,
        "only Sample Mono is fixed width"
    );
    cx.update(|app| {
        let settings = TypographySettings::defaults(&catalog);
        Typography::init(catalog, settings, app);
    });
    let window: AnyWindowHandle = cx
        .open_window(size(px(600.), px(200.)), |_, app| app.new(|_| Samples))
        .expect("headless window opens")
        .into();
    draw(&mut cx, window);

    let before = measure(&mut cx, window);
    // 14 glyphs of the proportional family at 16 px; the fixed-width samples are 9 and 13
    // glyphs of 0.6 em.
    assert!(
        (before.code - 9.0 * 0.6 * 12.5).abs() < 0.5,
        "code width {}",
        before.code
    );
    assert!(
        (before.terminal - 13.0 * 0.6 * 13.0).abs() < 0.5,
        "terminal width {}",
        before.terminal
    );

    // Change the interface size to 20 px, updating the window's root rem size.
    let changed = cx
        .update_window(window, |_, window, app| {
            Typography::set_interface_size(window, 20., app)
        })
        .expect("window is open");
    assert!(changed);
    let rem = cx
        .update_window(window, |_, window, _| f32::from(window.rem_size()))
        .expect("window is open");
    assert_eq!(rem, 20.);
    draw(&mut cx, window);

    let after = measure(&mut cx, window);
    let growth = after.interface / before.interface;
    assert!(
        (growth - 20.0 / 16.0).abs() < 0.001,
        "interface width grew by {growth}, expected {}",
        20.0 / 16.0
    );
    assert_eq!(after.code, before.code, "code width must not change");
    assert_eq!(
        after.terminal, before.terminal,
        "terminal width must not change"
    );
}

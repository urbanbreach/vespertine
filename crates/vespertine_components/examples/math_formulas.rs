//! CMP-012 A1: an inline fraction and a centered display formula in a document window.
//!
//! The window shows a paragraph with inline formulas, a centered display formula and chips that
//! switch math on and off and cycle the math size. Select a formula with the pointer and copy it
//! (ctrl-c): the clipboard receives the original delimited source, `$\frac{a}{b}$` for the inline
//! fraction and `$$...$$` for the display formula, never the reserved placeholder characters.
//!
//! Without arguments the example runs interactively. With `MATH_FORMULAS_SCRIPT=1` it selects and
//! copies each formula by itself, checks the copied text and exits 0 when both sources came out as
//! specified (1 otherwise).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vespertine::{
    AnyElement, App, AppContext as _, AsyncApp, Bounds, Context, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PlatformInput, Render, StatefulInteractiveElement, Styled,
    TextRun, Window, WindowBounds, WindowHandle, WindowOptions, div, font, point, px, rgb, size,
};
use vespertine_components::math::{
    DisplayBlock, MathBlock, MathStyle, MathText, MathTextOptions, PaintObserver, PaintReport,
    Presentation, inline_line_height, math_text, present_inline, slot_font, slot_runs,
};
use vespertine_platform::application;
use vespertine_richtext::{TextService, text_root};

const FONT: &str = "IBM Plex Sans";
const INLINE_FRACTION: &str = r"\frac{a}{b}";
const DISPLAY: &str = r"\sum_{i=0}^{n} \frac{1}{i^2} = \frac{\pi^2}{6}";

struct Showcase {
    style: MathStyle,
    inline_report: Rc<RefCell<Option<PaintReport>>>,
    display_report: Rc<RefCell<Option<PaintReport>>>,
}

fn run(len: usize, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font(FONT),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl Showcase {
    fn paragraph(&self, window: &mut Window, cx: &mut App, ink: Hsla, size: f32) -> AnyElement {
        let mut text = MathText::new();
        let pieces: [(&str, bool); 7] = [
            ("An inline fraction ", false),
            (INLINE_FRACTION, true),
            (" sits on the text baseline, as do ", false),
            (r"x^2+\sqrt{y_1}", true),
            (" and ", false),
            (r"\sum_{k=1}^{n} k", true),
            (". Select one and press ctrl-c.", false),
        ];
        for (piece, is_math) in pieces {
            if !is_math {
                text.push_text(piece);
                continue;
            }
            match present_inline(piece, &self.style) {
                Presentation::Formula { formula, .. } => {
                    if !text.push_inline(piece, &formula, self.style.scale()) {
                        text.push_text(&format!("${piece}$"));
                    }
                }
                Presentation::InlineCode(code) => text.push_text(&code),
                Presentation::CodeBlock { .. } => {}
            }
        }
        let mut runs: Vec<(usize, bool)> = Vec::new();
        let mut offset = 0;
        for slot in text.slots() {
            if slot.start > offset {
                runs.push((slot.start - offset, false));
            }
            runs.push((slot.len(), true));
            offset = slot.end;
        }
        if text.displayed().len() > offset {
            runs.push((text.displayed().len() - offset, false));
        }
        let system = window.text_system().clone();
        let text_id = system.resolve_font(&font(FONT));
        let slot_id = system.resolve_font(&slot_font());
        let line_height = inline_line_height(
            &text.flat(runs),
            text.formulas(),
            self.style.scale(),
            px(size * 1.5),
            px(size),
            |is_slot: &bool| {
                let id = if *is_slot { slot_id } else { text_id };
                (
                    f32::from(system.ascent(id, px(100.0))) / 100.0,
                    f32::from(system.descent(id, px(100.0))) / 100.0,
                )
            },
        );
        let reports = self.inline_report.clone();
        let observer: PaintObserver = Rc::new(move |report| {
            *reports.borrow_mut() = Some(report.clone());
        });
        let runs = slot_runs(&text, &run(0, ink));
        let rich = math_text(
            "paragraph",
            &text,
            runs,
            MathTextOptions::inline(self.style.scale()).observer(observer),
            window,
            cx,
        );
        div()
            .w_full()
            .line_height(line_height)
            .text_size(px(size))
            .child(rich)
            .into_any_element()
    }
}

impl Render for Showcase {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ink = Hsla::from(rgb(0xE6E8EB));
        let size = 18.0;
        let paragraph = self.paragraph(window, cx, ink, size);
        let reports = self.display_report.clone();
        let observer: PaintObserver = Rc::new(move |report| {
            *reports.borrow_mut() = Some(report.clone());
        });
        let display = DisplayBlock::new("display", DISPLAY, self.style, run(0, ink), px(size))
            .line_height(px(size * 1.5))
            .observer(observer)
            .build(window, cx);
        let display = match display {
            MathBlock::Typeset(element) => element,
            MathBlock::Fallback { language, text } => div()
                .font_family("DejaVu Sans Mono")
                .child(format!("{language}: {text}"))
                .into_any_element(),
        };
        let chip = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .bg(rgb(0x30343D))
                .cursor_pointer()
                .child(label)
        };
        let enabled = self.style.enabled;
        let percent = self.style.size();
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181A1F))
                .text_color(rgb(0xE6E8EB))
                .font_family(FONT)
                .text_size(px(14.0))
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            chip(
                                "toggle",
                                format!("Math: {}", if enabled { "on" } else { "off" }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.style = this.style.enabled(!this.style.enabled);
                                cx.notify();
                            })),
                        )
                        .child(
                            chip("size", format!("Size: {percent}%")).on_click(cx.listener(
                                |this, _, _, cx| {
                                    let next = match this.style.size() {
                                        75 => 100,
                                        100 => 150,
                                        150 => 200,
                                        _ => 75,
                                    };
                                    this.style = this.style.size_percent(next);
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(paragraph)
                .child(display),
        )
    }
}

fn input_move(x: f32, y: f32, pressed: Option<MouseButton>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: pressed,
        modifiers: Modifiers::default(),
    })
}

/// Selects `from..to` at height `y` with the pointer and presses ctrl-c. Returns the text the
/// copy chord puts on the clipboard (the selected text of the document) and what the system
/// clipboard held afterwards, which a compositor only serves back to a focused client.
async fn select_and_copy(
    cx: &mut AsyncApp,
    window: WindowHandle<Showcase>,
    from: f32,
    to: f32,
    y: f32,
) -> (Option<String>, Option<String>) {
    let dispatch = |cx: &mut AsyncApp, input: PlatformInput| {
        // Not `WindowHandle::update`: that leases the root view, which handlers update.
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(input, cx);
        })
        .ok();
    };
    let pause = |cx: &mut AsyncApp| cx.background_executor().timer(Duration::from_millis(150));
    dispatch(cx, input_move(from, y, None));
    pause(cx).await;
    dispatch(
        cx,
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(from), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
    pause(cx).await;
    dispatch(cx, input_move(to, y, Some(MouseButton::Left)));
    pause(cx).await;
    dispatch(
        cx,
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(to), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    );
    pause(cx).await;
    let selected = cx
        .update_window(window.into(), |_, window, cx| {
            TextService::of(window, cx).selected_text()
        })
        .ok()
        .flatten();
    dispatch(
        cx,
        PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse("ctrl-c").expect("a chord"),
            is_held: false,
            prefer_character_input: false,
        }),
    );
    pause(cx).await;
    let clipboard = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    (selected, clipboard)
}

fn run_script(cx: &mut App, window: WindowHandle<Showcase>) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(800))
            .await;
        let (inline, display) = window
            .read_with(cx, |view, _| {
                (
                    view.inline_report.borrow().clone(),
                    view.display_report.borrow().clone(),
                )
            })
            .expect("window state");
        let mut failures = 0;
        for (name, report, expected) in [
            ("inline", inline, format!("${INLINE_FRACTION}$")),
            ("display", display, format!("$${DISPLAY}$$")),
        ] {
            let Some(slot) = report.and_then(|report| report.slots.first().cloned()) else {
                println!("math_formulas: {name} formula was not painted");
                failures += 1;
                continue;
            };
            let y = slot.row_top + slot.line_height / 2.0;
            let (copied, clipboard) =
                select_and_copy(cx, window, slot.slot.start + 1.0, slot.slot.end + 1.0, y).await;
            if copied.as_deref() == Some(expected.as_str()) {
                println!("math_formulas: {name} copies as {expected} (clipboard: {clipboard:?})");
            } else {
                println!("math_formulas: {name} copied {copied:?}, expected {expected:?}");
                failures += 1;
            }
        }
        std::process::exit(i32::from(failures > 0));
    })
    .detach();
}

fn main() {
    let script = std::env::var_os("MATH_FORMULAS_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.0), px(360.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    // Make sure the pointer service of the document exists before the first frame.
                    let _ = TextService::of(window, cx);
                    cx.new(|_| Showcase {
                        style: MathStyle::new(),
                        inline_report: Rc::new(RefCell::new(None)),
                        display_report: Rc::new(RefCell::new(None)),
                    })
                },
            )
            .expect("open the window");
        if script {
            run_script(cx, window);
        }
        cx.activate(true);
    });
}

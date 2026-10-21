//! Shared helpers of the CMP-012 tests: paragraphs with inline formulas on a test host.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, AppContext as _, Context, Hsla, IntoElement, ParentElement, Render, Styled,
    TextRun, Window, div, font, px, rgb,
};
use vespertine_components::math::{
    Formula, MathStyle, MathText, MathTextOptions, PaintObserver, PaintReport, Presentation,
    inline_line_height, math_text, present_inline, slot_font, slot_runs,
};
use vespertine_richtext::{TextService, text_root};
use vespertine_test_host::{HostOptions, TestHost};

pub const FONT: &str = "IBM Plex Sans";
pub const BG: u32 = 0x181A1F;
pub const INK: u32 = 0xE6E8EB;

/// One piece of a paragraph.
#[derive(Clone, Copy)]
pub enum Piece {
    Text(&'static str),
    Math(&'static str),
}

pub fn text_run(len: usize) -> TextRun {
    TextRun {
        len,
        font: font(FONT),
        color: Hsla::from(rgb(INK)),
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: px(0.),
    }
}

/// Builds the paragraph text of `pieces`: formulas that fall back are shown as inline code text.
pub fn paragraph(pieces: &[Piece], style: &MathStyle) -> MathText {
    let mut text = MathText::new();
    for piece in pieces {
        match piece {
            Piece::Text(t) => text.push_text(t),
            Piece::Math(source) => match present_inline(source, style) {
                Presentation::Formula { formula, .. } => {
                    assert!(text.push_inline(source, &formula, style.scale()));
                }
                Presentation::InlineCode(code) => text.push_text(&code),
                Presentation::CodeBlock { .. } => unreachable!(),
            },
        }
    }
    text
}

/// Font ascent and descent in ems of the text font and of the slot (reference) font.
pub fn font_metrics(window: &Window) -> impl Fn(&bool) -> (f32, f32) {
    let system = window.text_system().clone();
    let text_id = system.resolve_font(&font(FONT));
    let slot_id = system.resolve_font(&slot_font());
    move |is_slot: &bool| {
        let id = if *is_slot { slot_id } else { text_id };
        (
            f32::from(system.ascent(id, px(100.0))) / 100.0,
            f32::from(system.descent(id, px(100.0))) / 100.0,
        )
    }
}

/// The line height of a paragraph with inline formulas at `size` with `configured` as its base.
pub fn paragraph_line_height(
    text: &MathText,
    style: &MathStyle,
    size: f32,
    configured: f32,
    window: &Window,
) -> f32 {
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
    f32::from(inline_line_height(
        &text.flat(runs),
        text.formulas(),
        style.scale(),
        px(configured),
        px(size),
        font_metrics(window),
    ))
}

/// A text-root page of one paragraph at the given font size; the line height is raised for the
/// inline formulas as a document renderer does.
pub fn paragraph_page(
    text: &MathText,
    style: &MathStyle,
    size: f32,
    configured_line_height: f32,
    width: f32,
    observer: Option<PaintObserver>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let line_height = paragraph_line_height(text, style, size, configured_line_height, window);
    let runs = slot_runs(text, &text_run(text.displayed().len()));
    let mut options = MathTextOptions::inline(style.scale());
    options.observer = observer;
    let rich = math_text("paragraph", text, runs, options, window, cx);
    text_root(
        div()
            .size_full()
            .bg(rgb(BG))
            .p(px(8.))
            .font_family(FONT)
            .text_size(px(size))
            .line_height(px(line_height))
            .text_color(rgb(INK))
            .child(div().w(px(width)).child(rich)),
    )
    .into_any_element()
}

/// Collects the paint reports of a formula element.
#[derive(Clone, Default)]
pub struct Reports(pub Rc<RefCell<Vec<PaintReport>>>);

impl Reports {
    pub fn observer(&self) -> PaintObserver {
        let reports = self.0.clone();
        Rc::new(move |report| reports.borrow_mut().push(report.clone()))
    }

    pub fn last(&self) -> PaintReport {
        self.0.borrow().last().cloned().expect("a painted formula")
    }
}

pub struct Page {
    pub host: TestHost,
    pub service: Rc<RefCell<Option<TextService>>>,
}

struct Doc(Box<dyn Fn(&mut Window, &mut App) -> AnyElement>);

impl Render for Doc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)(window, cx)
    }
}

/// Hosts `build` in an offscreen window; `None` when the machine has no GPU test backend.
pub fn host(
    width: f64,
    height: f64,
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> Option<Page> {
    if !vespertine_test_host::is_available() {
        eprintln!("skipped: {:?}", vespertine_test_host::unavailable_reason());
        return None;
    }
    let service = Rc::new(RefCell::new(None));
    let captured = service.clone();
    let host = TestHost::with_view(HostOptions::with_size(width, height), move |window, cx| {
        *captured.borrow_mut() = Some(TextService::of(window, cx));
        cx.new(|_| Doc(Box::new(build)))
    })
    .expect("host");
    Some(Page { host, service })
}

pub fn formula_of(source: &str, display: bool) -> Arc<Formula> {
    vespertine_components::math::lookup(source, display, Default::default())
        .unwrap_or_else(|| panic!("formula {source}"))
}

impl Page {
    /// Presses at `from`, drags to `to` and releases, painting a frame between the steps
    /// (selection listeners are installed during paint).
    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        let host = &mut self.host;
        host.mouse_down(f64::from(from.0), f64::from(from.1), 0, "")
            .unwrap();
        host.paint_frame();
        host.mouse_move(f64::from(to.0), f64::from(to.1), Some(0), "")
            .unwrap();
        host.paint_frame();
        host.mouse_up(f64::from(to.0), f64::from(to.1), 0, "")
            .unwrap();
        host.paint_frame();
    }

    /// The text a copy would put on the clipboard.
    pub fn selected_text(&self) -> Option<String> {
        self.service.borrow().as_ref().unwrap().selected_text()
    }
}

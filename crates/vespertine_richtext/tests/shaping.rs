//! TXT-006 against real shaping: paint-only invariance (A2) and geometry queries on wrapped text.

use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
use vespertine::{
    AppContext as _, Context, FontStyle, FontWeight, HeadlessAppContext, Hsla, IntoElement,
    ParentElement, Pixels, Render, Styled, StyledText, TextLayout, TextRun, UnderlineStyle,
    WindowHandle, div, font, point, px, rgb, size,
};
use vespertine_richtext::{IndexHit, RichLayout, RichSpans, plain_run, runs_from_spans};

struct Probe {
    text: String,
    runs: Vec<TextRun>,
    width: f32,
    size: f32,
    line_height: f32,
    layout: Option<TextLayout>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut vespertine::Window, _: &mut Context<Self>) -> impl IntoElement {
        let styled = StyledText::new(self.text.clone()).with_runs(self.runs.clone());
        self.layout = Some(styled.layout().clone());
        div()
            .w(px(self.width))
            .font_family("Noto Sans")
            .text_size(px(self.size))
            .line_height(px(self.line_height))
            .child(styled)
    }
}

struct Harness {
    cx: HeadlessAppContext,
    window: WindowHandle<Probe>,
}

impl Harness {
    fn new(text: &str, runs: Vec<TextRun>, width: f32) -> Self {
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans")),
            Arc::new(()),
            vespertine_platform::current_headless_renderer,
        );
        let text = text.to_string();
        let window = cx
            .open_window(size(px(400.), px(300.)), |_, cx| {
                cx.new(|_| Probe {
                    text,
                    runs,
                    width,
                    size: 16.,
                    line_height: 24.,
                    layout: None,
                })
            })
            .unwrap();
        let mut harness = Self { cx, window };
        harness.draw();
        harness
    }

    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.draw(cx).clear(cx)
            })
            .expect("draw");
    }

    fn update(&mut self, update: impl FnOnce(&mut Probe)) {
        let entity = self.window.entity(&self.cx).unwrap();
        self.cx.update(|app| {
            entity.update(app, |probe, cx| {
                update(probe);
                cx.notify();
            })
        });
        self.draw();
    }

    fn layout(&mut self) -> RichLayout {
        let entity = self.window.entity(&self.cx).unwrap();
        let layout = self
            .cx
            .update(|app| entity.read(app).layout.clone().unwrap());
        RichLayout::new(layout)
    }
}

fn base_font() -> vespertine::Font {
    font("Noto Sans")
}

const TEXT: &str = "The quick brown fox jumps over the lazy dog and keeps running far away";

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

fn shape_signature(harness: &mut Harness) -> (Vec<usize>, Pixels, Pixels) {
    let layout = harness.layout();
    (
        layout.line_starts(),
        layout.unwrapped_width(),
        layout.bounds().size.width,
    )
}

#[test]
fn paint_only_updates_never_change_line_breaks_or_widths() {
    let font = base_font();
    let runs = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
    let mut harness = Harness::new(TEXT, runs, 160.);
    let before = shape_signature(&mut harness);
    assert!(before.0.len() > 2, "text must wrap: {:?}", before.0);

    // Color-only: split the text into differently colored runs with the same font.
    harness.update(|probe| {
        probe.runs = runs_from_spans(
            TEXT,
            &[(4..9, color(0xff0000)), (20..30, color(0x00ff00))],
            color(0xeeeeee),
            &font,
        );
    });
    assert_eq!(shape_signature(&mut harness), before, "color-only update");

    // Background-only.
    harness.update(|probe| {
        let mut runs = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
        runs[0].background_color = Some(color(0x3a5a8c));
        probe.runs = runs;
    });
    assert_eq!(
        shape_signature(&mut harness),
        before,
        "background-only update"
    );

    // Underline-only (also wavy).
    harness.update(|probe| {
        let mut runs = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
        runs[0].underline = Some(UnderlineStyle {
            thickness: px(2.),
            color: Some(color(0xff00ff)),
            wavy: true,
        });
        probe.runs = runs;
    });
    assert_eq!(
        shape_signature(&mut harness),
        before,
        "underline-only update"
    );
}

#[test]
fn weight_and_italic_changes_change_the_shaped_text() {
    let font = base_font();
    let plain = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
    let mut harness = Harness::new(TEXT, plain, 160.);
    let before = shape_signature(&mut harness);

    let mut bold = font.clone();
    bold.weight = FontWeight::BOLD;
    harness.update(|probe| probe.runs = vec![plain_run(TEXT.len(), &bold, color(0xeeeeee))]);
    let bold_signature = shape_signature(&mut harness);
    assert_ne!(bold_signature.1, before.1, "bold changes the advances");
    assert_ne!(bold_signature, before);

    let mut italic = font;
    italic.style = FontStyle::Italic;
    harness.update(|probe| probe.runs = vec![plain_run(TEXT.len(), &italic, color(0xeeeeee))]);
    assert_ne!(
        shape_signature(&mut harness).1,
        before.1,
        "italic changes the advances"
    );
}

#[test]
fn shaping_key_ignores_paint_only_attributes() {
    let font = base_font();
    let a = RichSpans::new("hello world", vec![plain_run(11, &font, color(0xeeeeee))]).unwrap();
    let mut painted = plain_run(11, &font, color(0x112233));
    painted.background_color = Some(color(0x445566));
    painted.underline = Some(UnderlineStyle {
        thickness: px(1.),
        color: None,
        wavy: false,
    });
    let b = RichSpans::new("hello world", vec![painted]).unwrap();
    let key = |spans: &RichSpans| spans.shaping_key(px(14.), Some(px(100.)), None);
    assert_eq!(key(&a), key(&b));
    let mut bold = font;
    bold.weight = FontWeight::BOLD;
    let c = RichSpans::new("hello world", vec![plain_run(11, &bold, color(0xeeeeee))]).unwrap();
    assert_ne!(key(&a), key(&c));
    assert_ne!(
        a.shaping_key(px(14.), Some(px(100.)), None),
        a.shaping_key(px(15.), Some(px(100.)), None)
    );
}

#[test]
fn row_geometry_matches_the_shaped_layout() {
    let font = base_font();
    let runs = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
    let mut harness = Harness::new(TEXT, runs, 160.);
    let layout = harness.layout();
    let rows = layout.rows();
    assert!(rows.len() >= 3);
    let line_height = layout.line_height();
    assert_eq!(line_height, px(24.));
    for (ix, row) in rows.iter().enumerate() {
        assert_eq!(row.top, layout.bounds().origin.y + line_height * ix as f32);
        assert!(row.ascent > px(0.) && row.descent > px(0.));
        let baseline = row.baseline_offset(line_height);
        assert!(baseline > row.ascent - px(0.01) && baseline < line_height);
    }
    // Soft-wrap affinity: the row-ending index is reported at the end of its row, the same
    // index starts the next row for range geometry.
    let boundary = rows[1].start;
    let end_of_row0 = layout.position_for_index(boundary).unwrap();
    assert_eq!(end_of_row0.y, layout.bounds().origin.y);
    let after = layout.position_for_index(boundary + 1).unwrap();
    assert_eq!(after.y, layout.bounds().origin.y + line_height);

    // A range crossing the wrap boundary has one full-height rectangle per row, no gaps.
    let rects = layout.range_rects(boundary - 3..boundary + 3, px(0.), px(0.));
    assert_eq!(rects.len(), 2, "{rects:?}");
    assert_eq!(rects[0].size.height, line_height);
    assert_eq!(rects[0].bottom(), rects[1].top());
    assert_eq!(rects[1].origin.x, layout.bounds().origin.x);
    // A range that begins exactly at the soft wrap starts on the next row.
    let rects = layout.range_rects(boundary..boundary + 3, px(0.), px(0.));
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].origin.y, after.y);
    // The whole text yields one rectangle per row.
    let rects = layout.range_rects(0..TEXT.len(), px(0.), px(0.));
    assert_eq!(rects.len(), rows.len());
    // Stale offsets past the text never panic.
    let _ = layout.range_rects(0..TEXT.len() + 50, px(0.), px(0.));
}

#[test]
fn index_lookup_distinguishes_exact_and_nearest_hits() {
    let font = base_font();
    let runs = vec![plain_run(TEXT.len(), &font, color(0xeeeeee))];
    let mut harness = Harness::new(TEXT, runs, 160.);
    let layout = harness.layout();
    let origin = layout.bounds().origin;
    let inside = layout.index_for_position(point(origin.x + px(5.), origin.y + px(5.)));
    assert!(inside.is_exact(), "{inside:?}");
    assert_eq!(
        layout.index_for_position(point(origin.x + px(5.), origin.y - px(5.))),
        IndexHit::Nearest(0)
    );
    let past_end_of_row0 = layout.index_for_position(point(origin.x + px(390.), origin.y + px(5.)));
    assert_eq!(past_end_of_row0, IndexHit::Nearest(layout.rows()[1].start));
    let below = layout.index_for_position(point(origin.x + px(5.), origin.y + px(900.)));
    assert_eq!(below, IndexHit::Nearest(TEXT.len()));
}

#[test]
fn wrapped_unicode_rows_start_on_grapheme_boundaries() {
    let text = "Latin words e\u{301}cole \u{4e2d}\u{6587}\u{6587}\u{5b57}\u{5b57} family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} flag \u{1F1EB}\u{1F1EE} end of the line text";
    let font = base_font();
    let runs = vec![plain_run(text.len(), &font, color(0xeeeeee))];
    let mut harness = Harness::new(text, runs, 90.);
    let layout = harness.layout();
    let starts = layout.line_starts();
    assert!(starts.len() > 3);
    let boundaries: Vec<usize> = text.grapheme_indices(true).map(|(ix, _)| ix).collect();
    for start in starts {
        assert!(
            start == 0 || boundaries.contains(&start),
            "row start {start} is inside a grapheme cluster"
        );
    }
}

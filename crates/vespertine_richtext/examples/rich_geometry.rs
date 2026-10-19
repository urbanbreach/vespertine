//! TXT-006 A1: mixed Unicode text (Latin, CJK, combining marks, ZWJ emoji) wrapped at a narrow
//! width, with a shortened link label and a reserved inline-object slot. Pointer positions across
//! the wrapped text are mapped back to source offsets; the example verifies that a click inside the
//! replacement resolves to the replacement's source start and that no offset falls inside a
//! grapheme cluster. It prints the report and exits 0 (1 when a check fails).

use std::cell::RefCell;
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;
use vespertine::{
    App, Bounds, Context, Pixels, Render, SharedString, StyledText, TextLayout, TextRun,
    UnderlineStyle, Window, WindowBounds, WindowOptions, black, div, font, point, prelude::*, px,
    rgb, size,
};
use vespertine_platform::application;
use vespertine_richtext::{
    DisplayMap, FILE_LINK_SLOT, FlatText, OriginalText, RichLayout, fit_labels, insert_slots,
    plain_run,
};

const SOURCE: &str = "Mixed: Latin e\u{301}cole, CJK \u{4e2d}\u{6587}\u{6587}\u{5b57}, family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}, then see https://example.com/some/very/long/path/to/a/file.rs for details, or open notes.md instead.";
const SIZE: f32 = 15.0;
const LABEL_WIDTH: f32 = 150.0;

struct Presented {
    original: OriginalText,
    runs: Vec<TextRun>,
    source_link: std::ops::Range<usize>,
    displayed_link: std::ops::Range<usize>,
    slot: std::ops::Range<usize>,
    note_end: usize,
}

fn present(window: &Window) -> Presented {
    let family = font("Noto Sans");
    let link_start = SOURCE.find("https://").unwrap();
    let link_end = link_start + "https://example.com/some/very/long/path/to/a/file.rs".len();
    let note_start = SOURCE.find("notes.md").unwrap();
    let note_end = note_start + "notes.md".len();
    let link = |len: usize| {
        let mut run = plain_run(len, &family, rgb(0x2255cc).into());
        run.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        });
        run
    };
    let runs = vec![
        plain_run(link_start, &family, black()),
        link(link_end - link_start),
        plain_run(note_start - link_end, &family, black()),
        link(note_end - note_start),
        plain_run(SOURCE.len() - note_end, &family, black()),
    ];

    // Stage 1: reserve a slot at the end of each link (pure insertions).
    let slotted = insert_slots(
        SOURCE,
        &runs,
        &[(link_end, FILE_LINK_SLOT), (note_end, FILE_LINK_SLOT)],
    );
    let displayed_link = link_start..slotted.slots[0].end;
    let displayed_note = slotted.map.source_to_displayed(note_start)..slotted.slots[1].end;

    // Stage 2: shorten the label (slot included) to LABEL_WIDTH.
    let flat = FlatText {
        text: slotted.text.clone(),
        runs: slotted
            .runs
            .iter()
            .map(|run| (run.len, run.clone()))
            .collect(),
        links: vec![displayed_link, displayed_note],
        code: vec![],
        slots: slotted.slots.clone(),
    };
    let text_system = window.text_system().clone();
    let shortened = fit_labels(&flat, px(LABEL_WIDTH), |text, runs| {
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|(len, style)| TextRun {
                len: *len,
                ..style.clone()
            })
            .collect();
        text_system
            .shape_line(SharedString::from(text.to_string()), px(SIZE), &runs, None)
            .width
    });
    let original = OriginalText::plain(SOURCE)
        .present(slotted.text.clone(), slotted.map)
        .present(shortened.flat.text.clone(), shortened.map.clone());
    Presented {
        runs: shortened
            .flat
            .runs
            .iter()
            .map(|(len, style)| TextRun {
                len: *len,
                ..style.clone()
            })
            .collect(),
        displayed_link: shortened.flat.links[0].clone(),
        slot: shortened
            .flat
            .slots
            .first()
            .cloned()
            .expect("the second slot is kept"),
        source_link: link_start..link_end,
        note_end,
        original,
    }
}

struct Demo {
    presented: Option<Presented>,
    layout: Rc<RefCell<Option<TextLayout>>>,
    checked: bool,
}

fn fail(message: String) -> ! {
    eprintln!("rich_geometry: FAILED: {message}");
    std::process::exit(1);
}

impl Demo {
    fn verify(presented: &Presented, layout: &RichLayout) {
        let displayed = presented.original.displayed();
        let source = presented.original.source();
        let rows = layout.line_starts();
        println!("source   : {source:?}");
        println!("displayed: {displayed:?}");
        println!("row starts (displayed bytes): {rows:?}");
        let boundaries: Vec<usize> = displayed.grapheme_indices(true).map(|(i, _)| i).collect();
        let source_boundaries: Vec<usize> = source.grapheme_indices(true).map(|(i, _)| i).collect();
        for start in &rows {
            if *start != 0 && !boundaries.contains(start) {
                fail(format!("row start {start} is inside a grapheme cluster"));
            }
        }
        // Sweep pointers across every row.
        let bounds = layout.bounds();
        let mut checked = 0;
        for row in layout.rows() {
            let y = row.top + layout.line_height() / 2.0;
            let mut x = bounds.origin.x;
            while x < bounds.origin.x + bounds.size.width {
                let hit = layout.index_for_position(point(x, y));
                let displayed_ix = hit.index();
                let source_ix = presented.original.source_index(displayed_ix);
                if !source.is_char_boundary(source_ix) {
                    fail(format!(
                        "source offset {source_ix} is not a character boundary"
                    ));
                }
                if source_ix != source.len() && !source_boundaries.contains(&source_ix) {
                    fail(format!(
                        "source offset {source_ix} is inside a grapheme cluster"
                    ));
                }
                checked += 1;
                x += px(1.);
            }
        }
        println!("{checked} pointer positions mapped; no offset inside a grapheme cluster");

        // A click inside the ellipsis returns the replacement's source start.
        let replacement = presented
            .original
            .map()
            .replacements()
            .first()
            .expect("the label is shortened")
            .clone();
        let ellipsis_mid = replacement.displayed.start + 1;
        let ellipsis_pos = layout
            .position_for_index(replacement.displayed.start)
            .expect("ellipsis position");
        let next_pos = layout
            .position_for_index(replacement.displayed.end)
            .expect("end of ellipsis");
        let probe = point(
            (ellipsis_pos.x + next_pos.x) / 2.0,
            ellipsis_pos.y + layout.line_height() / 2.0,
        );
        let hit = layout.index_for_position(probe);
        let source_ix = presented.original.source_index(hit.index());
        println!(
            "click inside the ellipsis (displayed {ellipsis_mid}) -> source {source_ix} \
             (replacement source start {})",
            replacement.source.start
        );
        if hit.index() < replacement.displayed.start
            || hit.index() >= replacement.displayed.end
            || source_ix != presented.original.source_index(replacement.displayed.start)
        {
            fail(
                "a click inside the ellipsis must resolve to the replacement's source start".into(),
            );
        }
        // A click on the kept slot resolves to its insertion index (the link end in the source).
        let slot_pos = layout
            .position_for_index(presented.slot.start)
            .expect("slot");
        let slot_end_pos = layout
            .position_for_index(presented.slot.end)
            .expect("slot end");
        let probe = point(
            (slot_pos.x + slot_end_pos.x) / 2.0,
            slot_pos.y + layout.line_height() / 2.0,
        );
        let hit = layout.index_for_position(probe);
        let source_ix = presented.original.source_index(hit.index());
        println!(
            "click on the slot -> source {source_ix} (insertion index {})",
            presented.note_end
        );
        if !presented.slot.contains(&hit.index()) || source_ix != presented.note_end {
            fail("a click on a slot must resolve to the insertion index".into());
        }
        // Copying the whole displayed text returns the original source.
        if presented.original.copy(0..displayed.len()) != source {
            fail("copy of the whole text differs from the source".into());
        }
        println!(
            "copy of the whole displayed text equals the source; link {:?} shown as {:?}, slot {:?}",
            presented.source_link, presented.displayed_link, presented.slot
        );
        let _ = DisplayMap::identity();
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let presented = self.presented.get_or_insert_with(|| present(window));
        let styled = StyledText::new(presented.original.displayed().to_string())
            .with_runs(presented.runs.clone());
        *self.layout.borrow_mut() = Some(styled.layout().clone());
        if !self.checked {
            self.checked = true;
            let layout = self.layout.clone();
            let original = presented.original.clone();
            let runs = presented.runs.clone();
            let (source_link, displayed_link, slot, note_end) = (
                presented.source_link.clone(),
                presented.displayed_link.clone(),
                presented.slot.clone(),
                presented.note_end,
            );
            window.on_next_frame(move |_, cx| {
                let layout = layout.borrow().clone().expect("laid out");
                let presented = Presented {
                    original,
                    runs,
                    source_link,
                    displayed_link,
                    slot,
                    note_end,
                };
                Demo::verify(&presented, &RichLayout::new(layout));
                cx.quit();
            });
            window.request_animation_frame();
        }
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .p(px(16.))
            .font_family("Noto Sans")
            .text_size(px(SIZE))
            .line_height(px(22.))
            .w(px(250.))
            .child(styled)
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(282.), px(240.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    presented: None,
                    layout: Rc::new(RefCell::new(None)),
                    checked: false,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

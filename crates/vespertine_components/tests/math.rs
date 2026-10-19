//! CMP-012: inline and display mathematical notation.

mod math_support;

use math_support::*;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use vespertine::{IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_components::math::{
    DisplayBlock, DisplayItem, FaceStatus, Formula, FormulaCache, MathBlock, MathFace, MathFonts,
    MathStyle, MathText, Presentation, ShapedSample, display_line_height, inline_line_height,
    present_display, present_inline, reserved_slot_text, slot_chars, slot_runs, space_advance,
    verify_face,
};
use vespertine_richtext::text_root;

const FACE: MathFace = MathFace::DEFAULT;

fn rule_formula(items: usize, width: f32) -> Formula {
    Formula::from_items(
        vec![
            DisplayItem::Rule {
                x: 0.0,
                y: -0.5,
                width,
                height: 0.1,
            };
            items
        ],
        width,
        0.5,
        0.0,
    )
    .unwrap()
}

// ---- A2 -------------------------------------------------------------------------------------

#[test]
fn malformed_sources_return_nothing_and_are_cached() {
    let cache = FormulaCache::with_limits(64, usize::MAX);
    for source in [r"\nosuchcommand", "{a", "a}", r"\frac{1}{", r"x^1^2"] {
        assert!(cache.lookup(source, false, FACE).is_none(), "{source}");
        let layouts = cache.layout_count();
        assert!(cache.lookup(source, false, FACE).is_none());
        assert_eq!(
            cache.layout_count(),
            layouts,
            "a second call for {source} performs no layout"
        );
        assert_eq!(cache.is_cached_error(source, false, FACE), Some(true));
    }
    // A valid formula is cached as a formula, not as an error.
    assert!(cache.lookup(r"\frac{a}{b}", false, FACE).is_some());
    assert_eq!(
        cache.is_cached_error(r"\frac{a}{b}", false, FACE),
        Some(false)
    );
    // Inline and display layouts are different entries.
    assert!(!cache.contains(r"\frac{a}{b}", true, FACE));
}

// ---- A3 -------------------------------------------------------------------------------------

#[test]
fn disabled_math_shows_inline_code_and_a_math_labelled_code_block() {
    let disabled = MathStyle::new().enabled(false);
    match present_inline("x^2", &disabled) {
        Presentation::InlineCode(code) => assert_eq!(code, "$x^2$"),
        other => panic!("{other:?}"),
    }
    match present_display("x^2", &disabled) {
        Presentation::CodeBlock { language, text } => {
            assert_eq!(language, "math");
            assert_eq!(text, "x^2");
        }
        other => panic!("{other:?}"),
    }
    // A malformed formula falls back to its source and never shows the engine's error text.
    let enabled = MathStyle::new();
    match present_inline(r"\nosuchcommand", &enabled) {
        Presentation::InlineCode(code) => assert_eq!(code, r"$\nosuchcommand$"),
        other => panic!("{other:?}"),
    }
    match present_display(r"\nosuchcommand", &enabled) {
        Presentation::CodeBlock { language, text } => {
            assert_eq!(language, "math");
            assert_eq!(text, r"\nosuchcommand");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        present_inline("x^2", &enabled),
        Presentation::Formula { .. }
    ));
}

#[test]
fn a_disabled_display_block_is_a_math_labelled_code_block() {
    let result = Arc::new(Mutex::new(None));
    let sink = result.clone();
    let Some(_page) = host(300.0, 100.0, move |window, cx| {
        let block = DisplayBlock::new(
            "disabled",
            r"\frac{a}{b}",
            MathStyle::new().enabled(false),
            text_run(0),
            px(16.0),
        )
        .build(window, cx);
        *sink.lock().unwrap() = Some(match block {
            MathBlock::Fallback { language, text } => Some((language, text)),
            MathBlock::Typeset(_) => None,
        });
        div().into_any_element()
    }) else {
        return;
    };
    let result = result.lock().unwrap().take().expect("built");
    assert_eq!(result, Some(("math", r"\frac{a}{b}".to_string())));
}

// ---- A4 -------------------------------------------------------------------------------------

#[test]
fn oversized_sources_are_refused_and_never_cached() {
    let cache = FormulaCache::with_limits(64, usize::MAX);
    let too_long = "x".repeat(16385);
    assert!(cache.lookup(&too_long, false, FACE).is_none());
    assert!(cache.is_empty(), "a refused source is not cached");
    assert_eq!(cache.layout_count(), 0, "refused before parsing");
    // 16384 bytes are accepted (spaces keep the formula narrow).
    let at_limit = format!("x{}", " ".repeat(16383));
    assert_eq!(at_limit.len(), 16384);
    assert!(cache.lookup(&at_limit, false, FACE).is_some());
    assert_eq!(cache.len(), 1);
}

#[test]
fn a_formula_over_twelve_ems_falls_back_inline_but_renders_as_display() {
    let rows = format!(r"\begin{{matrix}}{}a\end{{matrix}}", r"a \\ ".repeat(11));
    let style = MathStyle::new();
    let formula = formula_of(&rows, false);
    assert!(formula.height() > 12.0 && formula.height() < 200.0);
    match present_inline(&rows, &style) {
        Presentation::InlineCode(code) => assert_eq!(code, format!("${rows}$")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        present_display(&rows, &style),
        Presentation::Formula { .. }
    ));
    // The limit is on the scaled height: a smaller size lets a formula just over 12 em through.
    let shrunk = MathStyle::new().size_percent(75);
    assert!(
        formula.height() * shrunk.scale() > 12.0 || {
            matches!(present_inline(&rows, &shrunk), Presentation::Formula { .. })
        }
    );
}

#[test]
fn slots_beyond_8192_ems_or_with_a_non_finite_extent_are_refused() {
    assert!(slot_chars(8192.0, 1.0).is_some());
    assert!(slot_chars(8192.5, 1.0).is_none());
    assert!(slot_chars(5000.0, 2.0).is_none());
    assert!(slot_chars(f32::INFINITY, 1.0).is_none());
    assert!(slot_chars(f32::NAN, 1.0).is_none());
    assert!(slot_chars(-1.0, 1.0).is_none());
    assert!(slot_chars(0.0, 1.0).is_some_and(|count| count == 1));
    let wide = Arc::new(rule_formula(1, 9000.0));
    assert!(reserved_slot_text(&wide, 1.0).is_none());
    let mut text = MathText::new();
    text.push_text("before ");
    assert!(!text.push_inline("w", &wide, 1.0));
    assert_eq!(text.displayed(), "before ");
    assert!(text.slots().is_empty());
    // The engine itself refuses wide and tall formulas.
    let too_wide = r"\qquad ".repeat(1100);
    assert!(formula_lookup_is_none(&too_wide));
}

fn formula_lookup_is_none(source: &str) -> bool {
    FormulaCache::with_limits(8, usize::MAX)
        .lookup(source, false, FACE)
        .is_none()
}

// ---- A5 -------------------------------------------------------------------------------------

#[test]
fn the_cache_evicts_the_least_recently_used_entry_by_count() {
    let cache = FormulaCache::with_limits(2, usize::MAX);
    cache.lookup("a", false, FACE);
    cache.lookup("b", false, FACE);
    // Touching the first entry makes the second the eviction victim.
    cache.lookup("a", false, FACE);
    cache.lookup("c", false, FACE);
    assert!(cache.contains("a", false, FACE));
    assert!(!cache.contains("b", false, FACE));
    assert!(cache.contains("c", false, FACE));
    assert_eq!(cache.len(), 2);
    // Errors take part in the same accounting.
    cache.lookup(r"\bad", false, FACE);
    assert_eq!(cache.len(), 2);
}

#[test]
fn the_cache_never_exceeds_its_byte_bound_and_accounts_entry_weights() {
    let bound = 20_000;
    let cache = FormulaCache::with_limits(1000, bound);
    let mut inserted = 0;
    for terms in 20..40 {
        let source = (0..terms)
            .map(|i| format!("a_{i}"))
            .collect::<Vec<_>>()
            .join("+");
        assert!(cache.lookup(&source, false, FACE).is_some());
        inserted += 1;
        assert!(
            cache.retained_bytes() <= bound,
            "{} > {bound}",
            cache.retained_bytes()
        );
        assert_eq!(
            cache.retained_bytes(),
            cache.weights().iter().sum::<usize>(),
            "retained bytes equal the sum of the entry weights"
        );
    }
    assert!(cache.len() > 1, "more than one entry is retained");
    assert!(
        cache.len() < inserted,
        "fewer than all entries are retained"
    );
    // The most recent entry is always retained.
    let last = (0..39)
        .map(|i| format!("a_{i}"))
        .collect::<Vec<_>>()
        .join("+");
    assert!(cache.contains(&last, false, FACE));
}

#[test]
fn the_process_wide_cache_has_the_specified_bounds() {
    assert_eq!(vespertine_components::math::MAX_ENTRIES, 4096);
    assert_eq!(vespertine_components::math::MAX_BYTES, 8 * 1024 * 1024);
    let formula = formula_of(r"\frac{a}{b}", false);
    assert_eq!(
        formula.weight(11),
        11 + std::mem::size_of_val(formula.items())
    );
}

// ---- A6 -------------------------------------------------------------------------------------

#[test]
fn slots_cover_the_painted_extent_in_whole_reference_spaces() {
    for source in [
        r"\frac{a}{b}",
        "x",
        r"\sum_{i=0}^{n} i^2",
        "{}",
        r"\sqrt{x^2+y^2}",
    ] {
        let formula = formula_of(source, false);
        for scale in [0.75, 1.0, 1.5, 2.0] {
            let count = slot_chars(formula.extent(), scale).unwrap();
            let width = count as f32 * space_advance();
            assert!(count >= 1);
            assert!(width + 1e-4 >= formula.extent() * scale, "{source}");
            assert!(
                count == 1 || (count as f32 - 1.0) * space_advance() < formula.extent() * scale,
                "ceil, not more: {source} at {scale}"
            );
            let slot = reserved_slot_text(&formula, scale).unwrap();
            assert_eq!(slot.chars().count(), count);
            assert!(slot.chars().all(|ch| ch == '\u{a0}'));
        }
    }
}

#[test]
fn slots_scale_with_the_math_size() {
    let formula = formula_of(r"\frac{a+b}{c}", false);
    let at = |percent: i32| {
        let style = MathStyle::new().size_percent(percent);
        slot_chars(formula.extent(), style.scale()).unwrap() as f32
    };
    let (small, normal, large) = (at(75), at(100), at(200));
    assert!(small < normal && normal < large);
    assert!((large - 2.0 * normal).abs() <= 1.0, "{large} vs {normal}");
    assert!((small - 0.75 * normal).abs() <= 1.0, "{small} vs {normal}");
}

#[test]
fn a_display_formulas_height_scales_with_the_size_and_its_pad_does_not() {
    let formula = formula_of(r"\frac{a}{b}", true);
    let size = px(16.0);
    let at = |percent: i32| {
        let scale = MathStyle::new().size_percent(percent).scale();
        f32::from(display_line_height(&formula, scale, size, px(0.0)))
    };
    for percent in [75, 100, 150, 200] {
        let scale = percent as f32 / 100.0;
        assert_eq!(
            at(percent),
            (formula.height() * scale * 16.0 + 12.0).ceil(),
            "height x scale x size plus a pad of 12 whatever the math size"
        );
    }
    assert!(((at(200) - 12.0) - 2.0 * (at(100) - 12.0)).abs() <= 1.0);
    assert_eq!(
        f32::from(display_line_height(&formula, 1.0, size, px(500.0))),
        500.0,
        "the configured line height wins when it is larger"
    );
}

#[test]
fn negative_space_formulas_stay_inside_their_slot() {
    let sources = [
        r"\!\!\!\!x\!\!\!\!",
        r"x\!\!\!\!\!\!\!",
        r"\!\!\!\!\!\!\!\!y",
    ];
    let style = MathStyle::new();
    for source in sources {
        let formula = formula_of(source, false);
        assert!(
            formula.extent_left() < 0.0 || formula.extent() > formula.advance(),
            "{source} overhangs its advance box"
        );
    }
    let reports = Reports::default();
    let observer = reports.observer();
    let text = paragraph(
        &[
            Piece::Text("ab"),
            Piece::Math(sources[0]),
            Piece::Text("cd"),
            Piece::Math(sources[1]),
            Piece::Text("ef"),
            Piece::Math(sources[2]),
            Piece::Text("gh"),
        ],
        &style,
    );
    let Some(mut page) = host(320.0, 80.0, move |window, cx| {
        paragraph_page(
            &text,
            &style,
            16.0,
            24.0,
            300.0,
            Some(observer.clone()),
            window,
            cx,
        )
    }) else {
        return;
    };
    page.host.flush();
    let report = reports.last();
    assert_eq!(report.slots.len(), 3);
    for slot in &report.slots {
        assert!(
            slot.ink.start >= slot.slot.start - 0.01 && slot.ink.end <= slot.slot.end + 0.01,
            "ink {:?} stays inside slot {:?}",
            slot.ink,
            slot.slot
        );
    }
}

// ---- A7 -------------------------------------------------------------------------------------

#[test]
fn an_inline_fraction_raises_the_line_height_of_the_paragraph() {
    let style = MathStyle::new();
    let fraction = formula_of(r"\frac{a}{b}", false);
    let small = formula_of("x", false);
    let metrics = |_: &()| (0.9f32, 0.3f32);
    let size = px(16.0);
    let configured = px(20.0);

    let mut raised = MathText::new();
    raised.push_text("one ");
    raised.push_inline(r"\frac{a}{b}", &fraction, style.scale());
    raised.push_text(" two\nthree\nfour");
    let flat = raised.flat(vec![(raised.displayed().len(), ())]);
    let height = inline_line_height(&flat, raised.formulas(), 1.0, configured, size, metrics);
    let lift = (0.9 - 0.3) / 2.0;
    let half = (fraction.ascent() - lift).max(fraction.descent() + lift);
    assert_eq!(f32::from(height), (2.0 * half * 16.0).ceil());
    assert!(height > configured, "the fraction opens the line height");

    // The fraction sits on the last hard line only: the element has one line height, so the
    // lines without formulas get the raised height as well.
    let mut late = MathText::new();
    late.push_text("one\ntwo\nthree ");
    late.push_inline(r"\frac{a}{b}", &fraction, style.scale());
    let late_flat = late.flat(vec![(late.displayed().len(), ())]);
    assert_eq!(
        inline_line_height(&late_flat, late.formulas(), 1.0, configured, size, metrics),
        height
    );

    // A formula that fits the configured line leaves it alone.
    let mut plain = MathText::new();
    plain.push_text("a ");
    plain.push_inline("x", &small, style.scale());
    let plain_flat = plain.flat(vec![(plain.displayed().len(), ())]);
    assert_eq!(
        inline_line_height(&plain_flat, plain.formulas(), 1.0, px(40.0), size, metrics),
        px(40.0)
    );
}

#[test]
fn a_paragraph_without_inline_formulas_keeps_the_configured_line_height() {
    let metrics = |_: &()| (0.9f32, 0.3f32);
    // A paragraph with only display formulas has no inline slots: its text is ordinary text.
    let mut text = MathText::new();
    text.push_text("A paragraph that mentions formulas only in display blocks.");
    let flat = text.flat(vec![(text.displayed().len(), ())]);
    assert_eq!(
        inline_line_height(&flat, text.formulas(), 1.0, px(24.0), px(16.0), metrics),
        px(24.0)
    );
    // A display block is its own line box: its height follows the display rule.
    let display = formula_of(r"\sum_{i=0}^{n} \frac{1}{i^2}", true);
    assert!(display_line_height(&display, 1.0, px(16.0), px(24.0)) > px(24.0));
}

#[test]
fn inline_formulas_sit_on_the_baseline_inside_a_raised_line_box() {
    let style = MathStyle::new();
    let reports = Reports::default();
    let observer = reports.observer();
    let text = paragraph(
        &[
            Piece::Text("The ratio "),
            Piece::Math(r"\frac{\sqrt{a+b}}{c^2}"),
            Piece::Text(
                " is small and wraps over several lines because this text is long enough to need them, ",
            ),
            Piece::Math(r"\sum_{k=1}^{n} \frac{1}{k}"),
            Piece::Text(" again."),
        ],
        &style,
    );
    let Some(mut page) = host(260.0, 200.0, move |window, cx| {
        paragraph_page(
            &text,
            &style,
            16.0,
            24.0,
            240.0,
            Some(observer.clone()),
            window,
            cx,
        )
    }) else {
        return;
    };
    page.host.flush();
    let report = reports.last();
    assert_eq!(report.slots.len(), 2);
    for slot in &report.slots {
        assert!(
            slot.line_height > 24.0,
            "the line height is raised: {}",
            slot.line_height
        );
        // The whole formula lies inside its line box.
        assert!(slot.extent_y.start >= slot.row_top - 0.5);
        assert!(slot.extent_y.end <= slot.row_top + slot.line_height + 0.5);
    }
    let first = &report.slots[0];
    // The baseline lies at (line - (ascent + descent)) / 2 + ascent below the row top, i.e.
    // inside the line and below its middle for a font whose ascent exceeds its descent.
    assert!(first.baseline > first.row_top);
    assert!(first.baseline < first.row_top + first.line_height);
}

// ---- A8 -------------------------------------------------------------------------------------

#[test]
fn concurrent_misses_share_the_first_insertion_and_never_hold_the_lock_through_a_layout() {
    let cache = Arc::new(FormulaCache::with_limits(16, usize::MAX));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let slow = {
        let cache = cache.clone();
        thread::spawn(move || {
            cache.lookup_with("shared", false, FACE, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(rule_formula(3, 1.0))
            })
        })
    };
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // The slow thread is inside its layout. The lock is free: another thread's lookups finish.
    let (done_tx, done_rx) = mpsc::channel();
    {
        let cache = cache.clone();
        thread::spawn(move || {
            let other = cache.lookup_with("other", false, FACE, || Ok(rule_formula(1, 1.0)));
            // The first insertion for the contended key wins.
            let first = cache.lookup_with("shared", false, FACE, || Ok(rule_formula(1, 2.0)));
            done_tx.send((other, first)).unwrap();
        });
    }
    let (other, first) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("lookups complete while another thread lays out");
    assert!(other.is_some());
    let first = first.unwrap();
    release_tx.send(()).unwrap();
    let second = slow.join().unwrap().unwrap();
    assert!(
        Arc::ptr_eq(&first, &second),
        "both callers receive the first insertion's result"
    );
    assert_eq!(second.items().len(), 1);
    assert_eq!(cache.layout_count(), 3);
    assert_eq!(cache.len(), 2);
}

#[test]
fn layout_runs_with_enough_stack_even_when_called_from_a_small_thread() {
    let source = format!("{}x{}", r"\frac{1}{".repeat(60), "}".repeat(60));
    let cache = Arc::new(FormulaCache::with_limits(8, usize::MAX));
    let result = thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || cache.lookup(&source, false, FACE).is_some())
        .unwrap()
        .join()
        .unwrap();
    assert!(result, "a formula nested 60 levels deep lays out");
    let too_deep = format!("{}x{}", "{".repeat(65), "}".repeat(65));
    assert!(formula_lookup_is_none(&too_deep));
}

// ---- A9 -------------------------------------------------------------------------------------

#[test]
fn a_renumbered_system_copy_fails_verification() {
    let shape = |renumber: u32, matches: bool, drop_last: bool| {
        move |text: &str| {
            let face = vespertine_components::math::faces::face_data(0).unwrap();
            let mut out = Vec::new();
            for (index, ch) in text.char_indices() {
                out.push(ShapedSample {
                    font_matches: matches,
                    glyph: u32::from(face.glyph(ch).unwrap().0) + renumber,
                    index,
                });
            }
            if drop_last {
                out.pop();
            }
            out
        }
    };
    assert!(verify_face(0, &mut shape(0, true, false)));
    assert!(
        !verify_face(0, &mut shape(1, true, false)),
        "renumbered glyphs"
    );
    assert!(!verify_face(0, &mut shape(0, false, false)), "another font");
    assert!(!verify_face(0, &mut shape(0, true, true)), "missing glyph");
}

fn paint_fraction(
    status: Option<FaceStatus>,
) -> (vespertine_components::math::PaintReport, Option<FaceStatus>) {
    let reports = Reports::default();
    let observer = reports.observer();
    let style = MathStyle::new();
    let text = paragraph(
        &[
            Piece::Text("a "),
            Piece::Math(r"\frac{x}{y}+\sqrt{z}"),
            Piece::Text(" b"),
        ],
        &style,
    );
    let final_status = Arc::new(Mutex::new(None));
    let status_sink = final_status.clone();
    let Some(mut page) = host(200.0, 80.0, move |window, cx| {
        if let Some(status) = status {
            MathFonts::set_status(cx, 0, status);
        }
        let element = paragraph_page(
            &text,
            &style,
            16.0,
            24.0,
            180.0,
            Some(observer.clone()),
            window,
            cx,
        );
        *status_sink.lock().unwrap() = MathFonts::status(cx, 0);
        element
    }) else {
        return (Default::default(), None);
    };
    page.host.flush();
    let status = *final_status.lock().unwrap();
    (reports.last(), status)
}

#[test]
fn an_unverified_copy_paints_outlines_and_a_verified_font_paints_through_the_atlas() {
    if !vespertine_test_host::is_available() {
        return;
    }
    // The bundled face is registered on first use and verified: glyphs use the atlas.
    let (report, status) = paint_fraction(None);
    assert!(
        matches!(status, Some(FaceStatus::Verified(_))),
        "the registered bundled face verifies: {status:?}"
    );
    assert!(report.atlas_glyphs > 0);
    assert_eq!(report.outline_glyphs, 0);
    assert!(
        report.rules > 0,
        "the fraction bar and the radical bar are quads"
    );

    // A system copy with renumbered glyph identifiers is unverified: outlines from the
    // bundled data, never another copy of the family.
    let (report, status) = paint_fraction(Some(FaceStatus::Unverified));
    assert_eq!(status, Some(FaceStatus::Unverified));
    assert!(report.outline_glyphs > 0);
    assert_eq!(report.atlas_glyphs, 0);
    assert!(report.rules > 0);
}

#[test]
fn outline_and_atlas_painting_agree_on_the_formula_geometry() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let (atlas, _) = paint_fraction(None);
    let (outline, _) = paint_fraction(Some(FaceStatus::Unverified));
    assert_eq!(atlas.slots, outline.slots);
    assert_eq!(
        atlas.atlas_glyphs, outline.outline_glyphs,
        "the same glyphs paint either way"
    );
}

// ---- R5, R8: selection, copy, scrolling ------------------------------------------------------

#[test]
fn selecting_across_an_inline_slot_copies_the_dollar_delimited_source() {
    let style = MathStyle::new();
    let reports = Reports::default();
    let observer = reports.observer();
    let text = paragraph(
        &[
            Piece::Text("Inline "),
            Piece::Math(r"\frac{a}{b}"),
            Piece::Text(" fraction"),
        ],
        &style,
    );
    assert_eq!(text.original(), r"Inline $\frac{a}{b}$ fraction");
    let Some(mut page) = host(320.0, 80.0, move |window, cx| {
        paragraph_page(
            &text,
            &style,
            16.0,
            24.0,
            300.0,
            Some(observer.clone()),
            window,
            cx,
        )
    }) else {
        return;
    };
    page.host.flush();
    let slot = reports.last().slots[0].clone();
    let y = f64::from(slot.row_top + slot.line_height / 2.0);
    let y = y as f32;
    let copied = |page: &mut Page, from: f32, to: f32| {
        page.drag((from, y), (to, y));
        page.selected_text()
    };
    // From just before the slot to just after it: the formula, delimiters included.
    let selected = copied(&mut page, slot.slot.start + 1.0, slot.slot.end + 1.0);
    assert_eq!(selected.as_deref(), Some(r"$\frac{a}{b}$"));
    // Starting inside the slot still takes the whole formula (one unit of the original).
    let mid = (slot.slot.start + slot.slot.end) / 2.0;
    let selected = copied(&mut page, mid, slot.slot.end + 40.0);
    let selected = selected.expect("a selection");
    assert!(selected.starts_with(r"$\frac{a}{b}$"), "{selected}");
    assert!(
        !selected.contains('\u{a0}'),
        "placeholders are never copied"
    );
    // The whole paragraph copies as the original text.
    let selected = copied(&mut page, 9.0, 300.0);
    assert_eq!(selected.as_deref(), Some(r"Inline $\frac{a}{b}$ fraction"));
}

#[test]
fn a_display_formula_copies_as_the_doubled_dollar_form_and_is_centered_in_its_block() {
    let reports = Reports::default();
    let observer = reports.observer();
    let Some(mut page) = host(400.0, 120.0, move |window, cx| {
        let block = DisplayBlock::new(
            "display",
            r"\frac{a}{b} = c",
            MathStyle::new(),
            text_run(0),
            px(16.0),
        )
        .line_height(px(24.0))
        .observer(observer.clone())
        .build(window, cx);
        let MathBlock::Typeset(block) = block else {
            panic!("typeset");
        };
        text_root(
            div()
                .size_full()
                .bg(rgb(BG))
                .p(px(8.0))
                .font_family(FONT)
                .text_color(rgb(INK))
                .child(block),
        )
        .into_any_element()
    }) else {
        return;
    };
    page.host.flush();
    let report = reports.last();
    let slot = report.slots[0].clone();
    // Centered horizontally in the block (8 px padding on a 400 px window).
    let ink_middle = (slot.ink.start + slot.ink.end) / 2.0;
    assert!((ink_middle - 200.0).abs() < 2.0, "{ink_middle}");
    // Centered vertically in its line box instead of sitting on a text baseline.
    let box_middle = (slot.extent_y.start + slot.extent_y.end) / 2.0;
    assert!((box_middle - (slot.row_top + slot.line_height / 2.0)).abs() < 1.0);
    let y = f64::from(slot.row_top + slot.line_height / 2.0);
    page.drag(
        (slot.slot.start - 1.0, y as f32),
        (slot.slot.end + 1.0, y as f32),
    );
    assert_eq!(
        page.selected_text().as_deref(),
        Some(r"$$\frac{a}{b} = c$$")
    );
}

#[test]
fn a_display_formula_wider_than_the_column_scrolls_horizontally() {
    let reports = Reports::default();
    let observer = reports.observer();
    let source = (1..=30)
        .map(|i| format!("a_{{{i}}}"))
        .collect::<Vec<_>>()
        .join("+");
    let Some(mut page) = host(240.0, 100.0, move |window, cx| {
        let MathBlock::Typeset(block) =
            DisplayBlock::new("wide", &source, MathStyle::new(), text_run(0), px(16.0))
                .observer(observer.clone())
                .build(window, cx)
        else {
            panic!("typeset");
        };
        text_root(
            div()
                .size_full()
                .bg(rgb(BG))
                .font_family(FONT)
                .text_color(rgb(INK))
                .child(block),
        )
        .into_any_element()
    }) else {
        return;
    };
    page.host.flush();
    let before = reports.last().slots[0].clone();
    assert!(
        before.ink.end - before.ink.start > 240.0,
        "wider than the column"
    );
    assert!(
        before.ink.start >= -0.5,
        "starts at the left edge when it overflows"
    );
    page.host.mouse_move(120.0, 10.0, None, "").unwrap();
    page.host.paint_frame();
    page.host
        .scroll_wheel(120.0, 10.0, -150.0, 0.0, "")
        .unwrap();
    page.host.paint_frame();
    let after = reports.last().slots[0].clone();
    assert!(
        (before.slot.start - after.slot.start).abs() > 100.0,
        "the block scrolled horizontally: {} -> {}",
        before.slot.start,
        after.slot.start
    );
    assert_eq!(before.row_top, after.row_top, "and not vertically");
}

#[test]
fn slot_runs_use_the_reference_face_and_the_text_color() {
    let style = MathStyle::new();
    let text = paragraph(
        &[Piece::Text("a "), Piece::Math("x"), Piece::Text(" b")],
        &style,
    );
    let runs = slot_runs(&text, &text_run(0));
    assert_eq!(
        runs.iter().map(|run| run.len).sum::<usize>(),
        text.displayed().len()
    );
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1].font, vespertine_components::math::slot_font());
    assert_eq!(runs[1].color, runs[0].color);
    assert_eq!(text.slots()[0].len(), runs[1].len);
}

#[test]
fn a_formulas_paint_color_never_changes_its_layout() {
    // Painting takes the run color at paint time; the cached formula is color independent.
    let a = formula_of(r"\frac{a}{b}", false);
    let b = formula_of(r"\frac{a}{b}", false);
    assert!(Arc::ptr_eq(&a, &b), "one cached layout serves every color");
    assert_eq!(a.extent(), b.extent());
}

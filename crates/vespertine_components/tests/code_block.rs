//! CMP-014 code block host: rows, copy, gutter, typography, coloration, scrolling.

mod docs_support;

use docs_support::{Log, drag, host, log, washes};
use vespertine::{IntoElement, ParentElement, Styled, div, px};
use vespertine_components::code::{CodeBlock, CodeScrollState, LineBounds};
use vespertine_richtext::highlight;
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec};
use vespertine_test_host::require_backend;
use vespertine_theme::document::DocumentTheme;

fn mono(block: CodeBlock) -> CodeBlock {
    block
        .font_family("Liberation Mono")
        .text_size(px(14.))
        .line_height(px(22.))
}

fn bounds_of(lines: &Log<LineBounds>) -> Vec<LineBounds> {
    let mut latest: Vec<LineBounds> = Vec::new();
    for line in lines.borrow().iter() {
        match latest.iter_mut().find(|known| known.index == line.index) {
            Some(slot) => *slot = line.clone(),
            None => latest.push(line.clone()),
        }
    }
    latest.sort_by_key(|line| line.index);
    latest
}

#[test]
fn a2_selection_copies_blank_lines_whitespace_and_crlf_lines_exactly() {
    require_backend!();
    let lines: Log<LineBounds> = log();
    let sink = lines.clone();
    let mut host = host(400., 200., move || {
        let sink = sink.clone();
        div()
            .p(px(12.))
            .child(mono(
                CodeBlock::new("code", "  a\t \r\n\r\nb  \r\n").on_line_bounds(move |line| {
                    sink.borrow_mut().push(line.clone());
                }),
            ))
            .child(vespertine_richtext::RichText::new("after", "after"))
            .into_any_element()
    });
    host.paint_frame();
    let rows = bounds_of(&lines);
    assert_eq!(rows.len(), 4, "a, blank, b and the empty final line");
    let first = rows.first().unwrap().bounds;
    let last = rows.last().unwrap().bounds;
    let from = (
        f64::from(first.origin.x.as_f32()) + 1.,
        f64::from(first.origin.y.as_f32()) + 5.,
    );
    // Through the end of "b  " (the empty final line is still part of the selection when more
    // text follows it).
    let b = rows[2].bounds;
    let to_b = (
        f64::from(b.origin.x.as_f32()) + 300.,
        f64::from(b.origin.y.as_f32()) + 5.,
    );
    let text = drag(&mut host, from, to_b).unwrap();
    assert_eq!(
        text, "  a\t \n\nb  ",
        "blank line and whitespace exact, CRLF is LF"
    );
    let to_after = (
        f64::from(last.origin.x.as_f32()) + 25.,
        f64::from(last.origin.y.as_f32()) + 22. + 8.,
    );
    let text = drag(&mut host, from, to_after).unwrap();
    eprintln!(
        "selected {text:?} painted {:?}",
        docs_support::service(&mut host).painted_text()
    );
    assert!(
        text.starts_with("  a\t \n\nb  \n\n"),
        "the empty final line survives before following text: {text:?}"
    );
}

#[test]
fn a2_empty_code_is_one_row_and_height_is_lines_times_line_height() {
    require_backend!();
    for (code, count) in [("", 1usize), ("one", 1), ("a\nb\n\nc", 4)] {
        let lines: Log<LineBounds> = log();
        let sink = lines.clone();
        let mut host = host(300., 200., move || {
            let sink = sink.clone();
            div()
                .child(mono(CodeBlock::new("code", code).on_line_bounds(
                    move |line| {
                        sink.borrow_mut().push(line.clone());
                    },
                )))
                .into_any_element()
        });
        host.paint_frame();
        let rows = bounds_of(&lines);
        assert_eq!(rows.len(), count, "{code:?}");
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.bounds.size.height, px(22.), "fixed row height");
            assert_eq!(row.index, index);
            assert_eq!(row.source_line, index + 1);
        }
    }
}

#[test]
fn a2_line_numbers_are_chrome_excluded_from_copy_and_search() {
    require_backend!();
    let lines: Log<LineBounds> = log();
    let sink = lines.clone();
    let mut host = host(300., 200., move || {
        let sink = sink.clone();
        highlight(
            "find",
            HighlightDeclaration::from_spec(HighlightSpec::new("2")),
            div().child(mono(
                CodeBlock::new("code", "x1\nx2\nx3")
                    .show_line_numbers(true)
                    .on_line_bounds(move |line| sink.borrow_mut().push(line.clone())),
            )),
        )
        .into_any_element()
    });
    host.paint_frame();
    let rows = bounds_of(&lines);
    let first = rows[0].bounds;
    let last = rows[2].bounds;
    // Drag from the gutter (left of the text) across all rows.
    let text = drag(
        &mut host,
        (
            f64::from(first.origin.x.as_f32()) - 20.,
            f64::from(first.origin.y.as_f32()) + 5.,
        ),
        (
            f64::from(last.origin.x.as_f32()) + 100.,
            f64::from(last.origin.y.as_f32()) + 5.,
        ),
    )
    .unwrap();
    assert_eq!(text, "x1\nx2\nx3", "no line numbers in the copy");
    let washes = washes(&mut host);
    assert_eq!(
        washes.len(),
        1,
        "only the code line containing 2 matches: {washes:?}"
    );
    assert_eq!(washes[0].0, "x2");
}

#[test]
fn a3_typography_resolves_from_style_over_theme() {
    require_backend!();
    let heights = |style: fn(CodeBlock) -> CodeBlock, theme: DocumentTheme| {
        let lines: Log<LineBounds> = log();
        let sink = lines.clone();
        let mut host = host(300., 200., move || {
            let sink = sink.clone();
            let theme = theme.clone();
            div()
                .child(style(
                    CodeBlock::new("code", "a\nb")
                        .theme(theme)
                        .on_line_bounds(move |line| sink.borrow_mut().push(line.clone())),
                ))
                .into_any_element()
        });
        host.paint_frame();
        bounds_of(&lines)[0].bounds.size.height
    };
    let dark = DocumentTheme::dark();
    assert_eq!(heights(|b| b, dark.clone()), px(18.), "theme line height");
    assert_eq!(
        heights(|b| b.text_size(px(25.)), dark.clone()),
        px(36.),
        "twice the theme size doubles the row"
    );
    assert_eq!(heights(|b| b.line_height(px(30.)), dark.clone()), px(30.));
    let mut zero = dark.clone();
    zero.metrics.code_text_size = 0.;
    let height = heights(|b| b.text_size(px(20.)), zero);
    assert!(height.as_f32().is_finite());
    assert_eq!(
        height,
        px(18.),
        "a zero theme size keeps the theme line height"
    );
    assert_eq!(
        heights(|b| b.line_height(px(0.)), dark),
        px(18.),
        "zero override ignored"
    );
}

#[test]
fn a4_coloration_never_changes_row_heights_or_glyph_positions() {
    require_backend!();
    let measure = |language: Option<&'static str>, keyword: u32| {
        let lines: Log<LineBounds> = log();
        let sink = lines.clone();
        let mut theme = DocumentTheme::dark();
        theme.syntax.keyword = vespertine::rgb(keyword);
        let mut host = host(400., 200., move || {
            let sink = sink.clone();
            let mut block = mono(
                CodeBlock::new("code", "fn main() { let x = 1; }\n// comment")
                    .theme(theme.clone())
                    .on_line_bounds(move |line| sink.borrow_mut().push(line.clone())),
            );
            if let Some(language) = language {
                block = block.language(language);
            }
            highlight(
                "find",
                HighlightDeclaration::from_spec(HighlightSpec::new("main")),
                div().child(block),
            )
            .into_any_element()
        });
        host.paint_frame();
        let heights: Vec<_> = bounds_of(&lines)
            .iter()
            .map(|l| l.bounds.size.height)
            .collect();
        let rects: Vec<_> = washes(&mut host).into_iter().flat_map(|w| w.1).collect();
        (heights, rects)
    };
    let plain = measure(None, 0xff0000);
    let rust = measure(Some("rust"), 0xff0000);
    let rust_other_theme = measure(Some("rust"), 0x00ff00);
    let unknown = measure(Some("no-such-language"), 0x00ff00);
    assert_eq!(plain.0, rust.0);
    assert_eq!(rust.0, rust_other_theme.0);
    assert!(!plain.1.is_empty());
    assert_eq!(
        plain.1, rust.1,
        "glyph positions are unchanged by highlighting"
    );
    assert_eq!(rust.1, rust_other_theme.1);
    assert_eq!(plain, unknown, "an unknown language renders plain");
}

#[test]
fn a8_horizontal_pan_leaves_the_outer_surface_bounds_unchanged() {
    require_backend!();
    let surface: Log<vespertine::Bounds<vespertine::Pixels>> = log();
    let sink = surface.clone();
    let state = CodeScrollState::new();
    let scroll = state.clone();
    let mut host = host(300., 200., move || {
        let sink = sink.clone();
        div()
            .p(px(10.))
            .child(
                div().size(px(200.)).child(mono(
                    CodeBlock::new(
                        "code",
                        "a very long line that will certainly overflow the two hundred pixel surface width and then some more text",
                    )
                    .scroll_state(scroll.clone())
                    .on_bounds(move |bounds| sink.borrow_mut().push(bounds)),
                )),
            )
            .into_any_element()
    });
    host.paint_frame();
    let before = state.offset_x();
    assert_eq!(before, px(0.));
    host.mouse_move(60., 20., None, "").unwrap();
    host.paint_frame();
    host.scroll_wheel(60., 20., -80., 0., "").unwrap();
    host.paint_frame();
    assert!(
        state.offset_x() > px(0.),
        "a horizontal gesture pans the viewport"
    );
    // A vertical wheel does not move it.
    let panned = state.offset_x();
    host.scroll_wheel(60., 20., 0., -50., "").unwrap();
    host.paint_frame();
    assert_eq!(state.offset_x(), panned, "vertical wheel passes through");
    let reported = surface.borrow();
    assert!(reported.len() >= 2);
    assert!(
        reported.windows(2).all(|pair| pair[0] == pair[1]),
        "the surface bounds never move: {reported:?}"
    );
}

#[test]
fn r9_the_scroll_indicator_press_and_drag_move_the_offset() {
    require_backend!();
    let state = CodeScrollState::new();
    let scroll = state.clone();
    let mut host = host(300., 120., move || {
        div()
            .p(px(10.))
            .child(div().w(px(200.)).child(mono(
                CodeBlock::new(
                    "code",
                    "a very long line that will certainly overflow the two hundred pixel surface width and then some more text",
                )
                .scroll_state(scroll.clone())
                .scroll_indicator(true),
            )))
            .into_any_element()
    });
    host.paint_frame();
    host.paint_frame();
    assert_eq!(state.offset_x(), px(0.));
    // The hit area is the bottom 10 px of the block (block spans y 10..32).
    let (x, y) = (190., 28.);
    host.mouse_move(x, y, None, "").unwrap();
    host.paint_frame();
    host.mouse_down(x, y, 0, "").unwrap();
    host.paint_frame();
    let pressed = state.offset_x();
    assert!(
        pressed > px(100.),
        "a press near the track end scrolls there: {pressed:?}"
    );
    assert!(state.dragging());
    host.mouse_move(20., y, Some(0), "").unwrap();
    host.paint_frame();
    assert!(
        state.offset_x() < pressed,
        "dragging back moves the pan back"
    );
    host.mouse_up(20., y, 0, "").unwrap();
    host.paint_frame();
    assert!(!state.dragging());
}

#[test]
fn r8_the_card_header_controls_copy_and_toggle() {
    require_backend!();
    use std::rc::Rc;
    use vespertine_components::code::{CodeCard, CopyOptions, WrapToggle};
    let copies: Log<(usize, String)> = log();
    let toggles = Rc::new(std::cell::Cell::new(0));
    let copied = Rc::new(std::cell::Cell::new(None::<usize>));
    let (sink, count, mark) = (copies.clone(), toggles.clone(), copied.clone());
    let mut host = host(400., 200., move || {
        let (sink, count, mark) = (sink.clone(), count.clone(), mark.clone());
        let card = CodeCard::new("card", 3, "let a = 1;\nlet b = 2;")
            .language("rust")
            .copy(CopyOptions {
                copied_index: mark.get(),
                on_copy: Rc::new(move |index, code, _, _| {
                    sink.borrow_mut().push((index, code.to_owned()))
                }),
            })
            .wrap_toggle(WrapToggle {
                wrapped: false,
                on_toggle: Rc::new(move |_, _| count.set(count.get() + 1)),
            });
        div().p(px(20.)).child(card).into_any_element()
    });
    host.paint_frame();
    // The language label is chrome; the card shows no "Copied" yet.
    let painted = host.painted_text();
    assert!(painted.iter().any(|text| text == "rust"));
    assert!(!painted.iter().any(|text| text == "Copied"));
    // Right-aligned actions: the copy button is the last control; click near the right edge.
    let (copy_x, wrap_x, y) = (
        400. - 20. - 5. - 12.,
        400. - 20. - 5. - 24. - 2. - 11.,
        20. + 1. + 14.,
    );
    host.mouse_move(copy_x, y, None, "").unwrap();
    host.paint_frame();
    host.mouse_down(copy_x, y, 0, "").unwrap();
    host.mouse_up(copy_x, y, 0, "").unwrap();
    host.paint_frame();
    assert_eq!(
        copies.borrow().as_slice(),
        [(3, "let a = 1;\nlet b = 2;".to_owned())]
    );
    host.mouse_move(wrap_x, y, None, "").unwrap();
    host.paint_frame();
    host.mouse_down(wrap_x, y, 0, "").unwrap();
    host.mouse_up(wrap_x, y, 0, "").unwrap();
    host.paint_frame();
    assert_eq!(toggles.get(), 1);
    // After the owner marks the block as copied the control shows "Copied" (still chrome).
    copied.set(Some(3));
    host.paint_frame();
    let painted = docs_support::service(&mut host).painted_text();
    assert!(
        painted
            .iter()
            .any(|text| text.text == "Copied" && text.chrome)
    );
    assert!(
        painted
            .iter()
            .any(|text| text.text == "rust" && text.chrome)
    );
}

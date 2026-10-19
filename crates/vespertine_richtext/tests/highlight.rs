//! TXT-009 against real shaping: declarations over retained and native text, nesting, numbering,
//! the inspection logs and the composited wash colors.

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use support::{Build, Harness, at, near, rgb_at};
use vespertine::{AnyElement, IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_richtext::search::{
    HighlightDeclaration, HighlightSpec, default_hex_color, find_ranges, scan_count,
};
use vespertine_richtext::{RichText, highlight, selection_color, text_root};

const WIDTH: f32 = 300.0;

fn page(child: impl IntoElement) -> AnyElement {
    text_root(
        div()
            .size_full()
            .bg(rgb(0x181818))
            .font_family("Noto Sans")
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(rgb(0xeeeeee))
            .child(child),
    )
    .into_any_element()
}

fn spec(query: &str) -> HighlightSpec {
    HighlightSpec::new(query)
        .with_color(default_hex_color("#F0C04066").unwrap())
        .with_active_color(default_hex_color("#F08040AA").unwrap())
}

#[test]
fn washes_paint_per_match_and_the_active_one_uses_its_color() {
    let spec = spec("red").with_active_index(Some(1));
    let build: Build = Rc::new(move |_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec.clone()),
            RichText::new("t", "Find red, red and RED."),
        ))
    });
    let mut harness = Harness::new(WIDTH, 60., build);
    harness.settle();
    let service = harness.service();
    let washes = service.painted_highlights();
    assert_eq!(washes.len(), 3);
    assert_eq!(
        washes
            .iter()
            .map(|w| (w.start, w.end, w.active))
            .collect::<Vec<_>>(),
        vec![(5, 8, false), (10, 13, true), (18, 21, false)]
    );
    assert!(
        washes
            .iter()
            .all(|w| w.rects.len() == 1 && w.rects[0][3] == 24.0)
    );
    assert!(washes.iter().all(|w| w.text == "Find red, red and RED."));
    // Composite colors: #F0C04066 over #181818 = #6E5B28, active #F08040AA over it = #A85D33.
    let image = harness.screenshot();
    let probe = |index: usize| {
        let rect = washes[index].rects[0];
        rgb_at(&image, WIDTH, rect[0] + 1.0, rect[1] + 2.0)
    };
    assert!(near(probe(0), [0x6e, 0x5b, 0x28], 2), "{:?}", probe(0));
    assert!(near(probe(1), [0xa8, 0x5d, 0x33], 2), "{:?}", probe(1));
    assert!(near(probe(2), [0x6e, 0x5b, 0x28], 2));
}

#[test]
fn a_match_crossing_a_soft_wrap_yields_two_rectangles() {
    let build: Build = Rc::new(|_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec("brown fox jumps")),
            div().w(px(110.)).child(RichText::new(
                "t",
                "The quick brown fox jumps over the lazy dog",
            )),
        ))
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    harness.settle();
    let washes = harness.service().painted_highlights();
    assert_eq!(washes.len(), 1);
    assert!(washes[0].rects.len() >= 2, "{:?}", washes[0].rects);
    for pair in washes[0].rects.windows(2) {
        assert_eq!(pair[0][1] + pair[0][3], pair[1][1], "rows are contiguous");
    }
}

#[test]
fn counts_and_scans_follow_the_rules() {
    let counts: Rc<RefCell<Vec<usize>>> = Rc::default();
    let active = Rc::new(RefCell::new(Some(0usize)));
    let query = Rc::new(RefCell::new("red".to_string()));
    let build: Build = {
        let (counts, active, query) = (counts.clone(), active.clone(), query.clone());
        Rc::new(move |_, _| {
            let counts = counts.clone();
            let spec = spec(&query.borrow()).with_active_index(*active.borrow());
            page(
                highlight(
                    "h",
                    HighlightDeclaration::from_spec(spec),
                    div()
                        .child(RichText::new("a", "red blue red"))
                        .child(RichText::new("b", "green RED")),
                )
                .on_match_count(move |count, _, _| counts.borrow_mut().push(count)),
            )
        })
    };
    let mut harness = Harness::new(WIDTH, 100., build);
    harness.settle();
    assert_eq!(counts.borrow().as_slice(), &[3]);
    let scans = scan_count();
    // Moving only the find cursor neither rescans nor reports.
    *active.borrow_mut() = Some(2);
    harness
        .cx
        .update_window(harness.window.into(), |_, window, _| window.refresh())
        .unwrap();
    harness.settle();
    assert_eq!(scan_count(), scans, "activeIndex change must not rescan");
    assert_eq!(counts.borrow().as_slice(), &[3]);
    let washes = harness.service().painted_highlights();
    assert_eq!(washes.iter().filter(|w| w.active).count(), 1);
    assert!(washes.last().unwrap().active, "match 2 is the last");
    // A different query with the same count is a new result.
    *query.borrow_mut() = "e".to_string();
    harness
        .cx
        .update_window(harness.window.into(), |_, window, _| window.refresh())
        .unwrap();
    harness.settle();
    assert!(scan_count() > scans);
    assert_ne!(counts.borrow().len(), 1);
}

#[test]
fn nested_declarations_override_their_ancestor_even_when_empty() {
    let counts: Rc<RefCell<Vec<usize>>> = Rc::default();
    let build: Build = {
        let counts = counts.clone();
        Rc::new(move |_, _| {
            let counts = counts.clone();
            page(
                highlight(
                    "outer",
                    HighlightDeclaration::from_spec(spec("red")),
                    div()
                        .child(RichText::new("a", "red outer"))
                        .child(highlight(
                            "inner",
                            HighlightDeclaration::new(),
                            RichText::new("b", "red silenced"),
                        )),
                )
                .on_match_count(move |count, _, _| counts.borrow_mut().push(count)),
            )
        })
    };
    let mut harness = Harness::new(WIDTH, 100., build);
    harness.settle();
    let washes = harness.service().painted_highlights();
    assert_eq!(washes.len(), 1, "the silenced subtree paints no wash");
    assert_eq!(washes[0].text, "red outer");
    assert_eq!(
        counts.borrow().as_slice(),
        &[1],
        "the ancestor's count excludes the subtree"
    );
}

#[test]
fn native_text_and_retained_text_number_in_paint_order() {
    let spec = spec("x")
        .with_match_index_offset(10)
        .with_active_index(Some(11));
    let build: Build = Rc::new(move |_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec.clone()),
            div()
                .child(RichText::new("retained", "x one"))
                .child(RichText::new("native", "native x line").native())
                .child(RichText::new("retained2", "x again")),
        ))
    });
    let mut harness = Harness::new(WIDTH, 120., build);
    harness.settle();
    let washes = harness.service().painted_highlights();
    // x one (10), native x (11, active), x again (12): interleaved in paint order.
    assert_eq!(washes.len(), 3);
    assert_eq!(
        washes
            .iter()
            .map(|w| (w.text.as_str(), w.active))
            .collect::<Vec<_>>(),
        vec![
            ("x one", false),
            ("native x line", true),
            ("x again", false)
        ]
    );
    // The painted-text log includes the native-generated string.
    let painted = harness.service().painted_text();
    assert!(painted.iter().any(|entry| entry.text == "native x line"));
}

#[test]
fn a_cursor_above_the_mounted_window_marks_nothing() {
    let spec = spec("red")
        .with_match_index_offset(10)
        .with_active_index(Some(3));
    let build: Build = Rc::new(move |_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec.clone()),
            RichText::new("t", "red red"),
        ))
    });
    let mut harness = Harness::new(WIDTH, 60., build);
    harness.settle();
    assert!(
        harness
            .service()
            .painted_highlights()
            .iter()
            .all(|w| !w.active)
    );
}

#[test]
fn a_selection_wash_paints_above_a_search_wash() {
    let build: Build = Rc::new(|_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec("red")),
            selection_color(
                vespertine::Rgba {
                    r: 0.27,
                    g: 0.4,
                    b: 0.8,
                    a: 0.5,
                },
                RichText::new("t", "red red"),
            ),
        ))
    });
    let mut harness = Harness::new(WIDTH, 60., build);
    harness.settle();
    let before = harness.screenshot();
    let washed = rgb_at(&before, WIDTH, 2., 3.);
    harness.drag(at(0., 12.), at(200., 12.));
    let after = harness.screenshot();
    let selected = rgb_at(&after, WIDTH, 2., 3.);
    assert_ne!(selected, washed, "selection paints above the search wash");
    // The search wash is still visible beneath it: the result differs from selection over plain.
    let plain: Build = Rc::new(|_, _| {
        page(selection_color(
            vespertine::Rgba {
                r: 0.27,
                g: 0.4,
                b: 0.8,
                a: 0.5,
            },
            RichText::new("t", "red red"),
        ))
    });
    let mut plain_harness = Harness::new(WIDTH, 60., plain);
    plain_harness.drag(at(0., 12.), at(200., 12.));
    let reference = rgb_at(&plain_harness.screenshot(), WIDTH, 2., 3.);
    assert!(
        !near(selected, reference, 2),
        "{selected:?} vs {reference:?}"
    );
}

#[test]
fn the_search_helper_matches_what_the_declaration_paints() {
    let text = "Find red, red and RED.";
    let ranges = find_ranges(&[text], "red", false, false);
    assert_eq!(ranges, vec![5..8, 10..13, 18..21]);
    let build: Build = Rc::new(move |_, _| {
        page(highlight(
            "h",
            HighlightDeclaration::from_spec(spec("red")),
            RichText::new("t", text),
        ))
    });
    let mut harness = Harness::new(WIDTH, 60., build);
    harness.settle();
    let painted: Vec<_> = harness
        .service()
        .painted_highlights()
        .iter()
        .map(|w| w.start..w.end)
        .collect();
    assert_eq!(painted, ranges);
}

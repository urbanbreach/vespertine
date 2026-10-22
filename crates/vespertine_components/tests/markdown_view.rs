//! CMP-011 rendering: metrics, links, selection, tasks, tables, media fallbacks.

mod docs_support;

use docs_support::{Log, host, log, washes};
use std::rc::Rc;
use vespertine::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use vespertine_components::markdown_doc::{
    DiagramView, Markdown, MarkdownDocument, MarkdownOptions, TaskOptions,
};
#[cfg(feature = "math")]
use vespertine_components::math::MathStyle;
use vespertine_richtext::highlight;
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec};
use vespertine_test_host::{TestHost, require_backend};
use vespertine_theme::document::DocumentTheme;

fn doc(source: &'static str, options: MarkdownOptions) -> AnyElement {
    div()
        .p(px(20.))
        .child(Markdown::new("md", source).options(options))
        .into_any_element()
}

fn finding(query: &'static str, child: AnyElement) -> AnyElement {
    highlight(
        "find",
        HighlightDeclaration::from_spec(HighlightSpec::new(query)),
        child,
    )
    .into_any_element()
}

fn md_host(
    width: f64,
    height: f64,
    source: &'static str,
    options: impl Fn() -> MarkdownOptions + 'static,
) -> TestHost {
    host(width, height, move || doc(source, options()))
}

fn pixel(image: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    image.get_pixel((x * 2.) as u32, (y * 2.) as u32).0
}

fn close(a: [u8; 4], b: [u8; 3], tolerance: u8) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.abs_diff(*y) <= tolerance)
}

#[test]
fn a8_blocks_stack_with_the_twelve_pixel_gap_and_headings_use_the_tiers() {
    require_backend!();
    let mut host = host(400., 400., || {
        finding(
            "x",
            doc(
                "# x1\n\n## x2\n\n### x3\n\n#### x4\n\n##### x5\n\n###### x6\n\nplain x7\n",
                MarkdownOptions::default(),
            ),
        )
    });
    host.paint_frame();
    let washes = washes(&mut host);
    assert_eq!(washes.len(), 7);
    let tops: Vec<f32> = washes.iter().map(|(_, rects)| rects[0][1]).collect();
    let heights: Vec<f32> = washes.iter().map(|(_, rects)| rects[0][3]).collect();
    assert_eq!(
        heights,
        [27., 24., 22., 22., 22., 22., 22.],
        "levels 4 to 6 share tier 4"
    );
    let mut expected = 20.;
    for (index, height) in heights.iter().enumerate() {
        assert!(
            (tops[index] - expected).abs() < 0.6,
            "block {index}: {} vs {expected}",
            tops[index]
        );
        expected += height + 12.;
    }
}

#[test]
fn a8_quote_list_and_inline_code_numbers() {
    require_backend!();
    let mut host = md_host(
        400.,
        300.,
        "> Quoted\n\n- bullet\n\n3. ordered\n\ninline `code` end\n",
        MarkdownOptions::default,
    );
    host.paint_frame();
    let image = host.screenshot_image().unwrap();
    let theme = DocumentTheme::dark();
    let accent = theme.accent;
    let bg = [0x18u8, 0x18, 0x18];
    // The quote rail: accent at alpha 0.6 over the fill over the window background (20..22).
    let rail = pixel(&image, 21., 20. + 11.);
    let expected_rail = |a: f32| -> [u8; 3] {
        let blend = |c: f32, base: f32| ((c * a + base * (1. - a)) * 255.).round() as u8;
        let fill = |c: f32, base: f32| c * 0.05 + base * 0.95;
        let base = 0x18 as f32 / 255.;
        [
            blend(accent.r, fill(accent.r, base)),
            blend(accent.g, fill(accent.g, base)),
            blend(accent.b, fill(accent.b, base)),
        ]
    };
    assert!(
        close(rail, expected_rail(0.6), 6),
        "rail {rail:?} vs {:?}",
        expected_rail(0.6)
    );
    // The quote fill is accent at alpha 0.05 (not the plain background).
    let fill = pixel(&image, 200., 20. + 11.);
    let expected_fill = |c: f32, base: f32| ((c * 0.05 + base * 0.95) * 255.).round() as u8;
    let base = 0x18 as f32 / 255.;
    assert!(
        close(
            fill,
            [
                expected_fill(accent.r, base),
                expected_fill(accent.g, base),
                expected_fill(accent.b, base)
            ],
            3
        ),
        "fill {fill:?}"
    );
    // Right corners are rounded, left corners square: the top-right pixel of the box shows
    // the background, the top-left pixel (on the rail) does not.
    let top_right = pixel(&image, 20. + 360. - 0.5, 20.25);
    assert!(
        close(top_right, bg, 2),
        "rounded corner shows the background: {top_right:?}"
    );
    let top_left = pixel(&image, 20.25, 20.25);
    assert!(!close(top_left, bg, 8), "square left corner: {top_left:?}");
    // The bullet: a 5 x 5 accent circle (alpha 0.85) one pixel from the left, centered in the
    // first 22 px line of its item.
    let quote_bottom = 20. + 34. + 12.;
    let bullet = pixel(&image, 20. + 1. + 2.5, quote_bottom + 11.);
    assert!(
        bullet[2] > 0x90 && bullet[0] > 0x60,
        "bullet pixel {bullet:?} should be accent-colored"
    );
    let outside = pixel(&image, 20. + 1. + 2.5, quote_bottom + 11. - 6.);
    assert!(close(outside, bg, 3));
}

#[test]
fn a8_inline_code_paints_a_wash_that_never_moves_glyphs() {
    require_backend!();
    let measure = |source: &'static str| {
        let mut host = host(400., 100., move || {
            finding("end", doc(source, MarkdownOptions::default()))
        });
        host.paint_frame();
        let washes = washes(&mut host);
        (washes[0].1[0], host.screenshot_image().unwrap())
    };
    let (with_code, image) = measure("a `bb` end");
    let (plain, _) = measure("a bb end");
    assert_eq!(
        with_code[0], plain[0],
        "glyph x of the following text is unchanged"
    );
    // The pill is expanded 2 px horizontally and inset 2 px vertically: inside the row it
    // is the code wash over the background, above the inset it is the background.
    let theme = DocumentTheme::dark();
    let wash = theme.code_wash;
    let base = 0x18 as f32 / 255.;
    let expect = [
        ((wash.r * wash.a + base * (1. - wash.a)) * 255.).round() as u8,
        ((wash.g * wash.a + base * (1. - wash.a)) * 255.).round() as u8,
        ((wash.b * wash.a + base * (1. - wash.a)) * 255.).round() as u8,
    ];
    // Text starts at x = 20; "a " is about 9 px wide; the code run starts right after.
    let probe_y = 20. + 11.;
    let inside = pixel(&image, 20. + 9., probe_y - 8.);
    let outside_row = pixel(&image, 20. + 9., 20. + 1.);
    assert!(
        close(inside, expect, 12) || close(pixel(&image, 20. + 12., probe_y - 8.), expect, 12),
        "pill {inside:?} vs {expect:?}"
    );
    assert!(
        close(outside_row, [0x18, 0x18, 0x18], 4),
        "inset 2 px: {outside_row:?}"
    );
}

#[test]
fn a2_unsupported_media_shows_the_label_link() {
    require_backend!();
    let activations: Log<String> = log();
    let sink = activations.clone();
    let mut host = host(400., 100., move || {
        let sink = sink.clone();
        finding(
            "alt",
            doc("![alt text](https://x.test/p.png) tail", {
                let mut options = MarkdownOptions::default();
                options.link_options =
                    options
                        .link_options
                        .clone()
                        .on_activate(move |activation, _, _| {
                            sink.borrow_mut().push(activation.destination.clone())
                        });
                options
            }),
        )
    });
    host.paint_frame();
    let washes = washes(&mut host);
    assert_eq!(washes.len(), 1, "the label is shown as text");
    let rect = washes[0].1[0];
    host.mouse_move(f64::from(rect[0] + 3.), f64::from(rect[1] + 8.), None, "")
        .unwrap();
    host.paint_frame();
    host.mouse_down(f64::from(rect[0] + 3.), f64::from(rect[1] + 8.), 0, "")
        .unwrap();
    host.mouse_up(f64::from(rect[0] + 3.), f64::from(rect[1] + 8.), 0, "")
        .unwrap();
    host.paint_frame();
    assert_eq!(activations.borrow().as_slice(), ["https://x.test/p.png"]);
}

fn click(host: &mut TestHost, x: f32, y: f32) {
    host.mouse_move(f64::from(x), f64::from(y), None, "")
        .unwrap();
    host.paint_frame();
    host.mouse_down(f64::from(x), f64::from(y), 0, "").unwrap();
    host.paint_frame();
    host.mouse_up(f64::from(x), f64::from(y), 0, "").unwrap();
    host.paint_frame();
}

fn link_options(log: &Log<String>) -> MarkdownOptions {
    let sink = log.clone();
    let mut options = MarkdownOptions::default();
    options.link_options = options
        .link_options
        .clone()
        .on_activate(move |activation, _, _| {
            sink.borrow_mut().push(activation.destination.clone())
        });
    options
}

const LINKS: &str = "go https://a.example/one and https://b.example/two end";

#[test]
fn a5_links_distinguish_urls_trailing_space_drags_and_the_click_slop() {
    require_backend!();
    let log: Log<String> = log();
    let sink = log.clone();
    let mut host = host(500., 80., move || {
        finding("https://a.example/one", doc(LINKS, link_options(&sink)))
    });
    host.paint_frame();
    let first = washes(&mut host)[0].1[0];
    let (left, right, mid_y) = (first[0], first[0] + first[2], first[1] + first[3] / 2.);
    // The trailing space after the first URL is not a link.
    click(&mut host, right + 2.5, mid_y);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    click(&mut host, left + 10., mid_y);
    assert_eq!(log.borrow().as_slice(), ["https://a.example/one"]);
    // The second URL is a separate link.
    click(&mut host, right + 40., mid_y);
    assert_eq!(log.borrow().last().unwrap(), "https://b.example/two");
    log.borrow_mut().clear();
    // Movement of 4 px activates, more does not.
    host.mouse_move(f64::from(left + 10.), f64::from(mid_y), None, "")
        .unwrap();
    host.paint_frame();
    host.mouse_down(f64::from(left + 10.), f64::from(mid_y), 0, "")
        .unwrap();
    host.mouse_up(f64::from(left + 14.), f64::from(mid_y), 0, "")
        .unwrap();
    host.paint_frame();
    assert_eq!(log.borrow().len(), 1, "4 px is a click");
    host.mouse_down(f64::from(left + 10.), f64::from(mid_y), 0, "")
        .unwrap();
    host.mouse_up(f64::from(left + 16.), f64::from(mid_y), 0, "")
        .unwrap();
    host.paint_frame();
    assert_eq!(log.borrow().len(), 1, "6 px is not");
    // A drag that starts in plain text and ends over a link selects instead of navigating.
    docs_support::drag(
        &mut host,
        (22., f64::from(mid_y)),
        (f64::from(left + 30.), f64::from(mid_y)),
    );
    assert_eq!(
        log.borrow().len(),
        1,
        "a drag ending over a link does not navigate"
    );
}

#[test]
fn a5_disabling_selection_keeps_links_and_search_working() {
    require_backend!();
    let log: Log<String> = log();
    let sink = log.clone();
    let mut host = host(500., 80., move || {
        let mut options = link_options(&sink);
        options.selectable = false;
        finding("https://a.example/one", doc(LINKS, options))
    });
    host.paint_frame();
    let first = washes(&mut host);
    assert_eq!(first.len(), 1, "search still washes");
    let rect = first[0].1[0];
    click(&mut host, rect[0] + 10., rect[1] + 10.);
    assert_eq!(log.borrow().as_slice(), ["https://a.example/one"]);
    let selected = docs_support::drag(
        &mut host,
        (22., f64::from(rect[1] + 10.)),
        (200., f64::from(rect[1] + 10.)),
    );
    assert_eq!(selected, None, "nothing can be selected");
}

const MIXED: &str =
    "- first item\n- second item\n\n3. ordered\n\n```rust\nfn a() {}\n\nfn b() {}\n```\n\ntail\n";

#[test]
fn a7_selection_excludes_chrome_and_keeps_empty_code_lines() {
    require_backend!();
    let mut host = host(500., 400., || {
        let mut options = MarkdownOptions::default();
        options.code.on_copy = Some(Rc::new(|_, _, _, _| {}));
        options.code.on_toggle_wrap = Some(Rc::new(|_, _, _| {}));
        doc(MIXED, options)
    });
    host.paint_frame();
    let text = docs_support::drag(&mut host, (21., 31.), (60., 300.)).unwrap();
    assert!(
        text.starts_with("first item\nsecond item\nordered\nfn a() {}\n\nfn b() {}\ntai"),
        "{text:?}"
    );
    for chrome in ["rust", "3.", "Copy", "Copied"] {
        assert!(!text.contains(chrome), "{chrome} is chrome: {text:?}");
    }
}

#[test]
fn a7_a_drag_survives_rows_leaving_paint() {
    require_backend!();
    let first_row = Rc::new(std::cell::Cell::new(0usize));
    let cell = first_row.clone();
    let mut host = host(400., 300., move || {
        let rows = [
            "row **zero** text",
            "row one text",
            "row two text",
            "row three text",
        ];
        div()
            .p(px(10.))
            .flex()
            .flex_col()
            .children(
                rows.iter()
                    .enumerate()
                    .skip(cell.get())
                    .map(|(index, source)| {
                        let mut options = MarkdownOptions::default();
                        options.row_key = Some(format!("row{index}").into());
                        Markdown::new(format!("md{index}"), *source).options(options)
                    }),
            )
            .into_any_element()
    });
    host.paint_frame();
    host.mouse_move(11., 20., None, "").unwrap();
    host.paint_frame();
    host.mouse_down(11., 20., 0, "").unwrap();
    host.paint_frame();
    host.mouse_move(80., 32. + 22. + 22. + 4., Some(0), "")
        .unwrap();
    host.paint_frame();
    first_row.set(2);
    host.paint_frame();
    host.mouse_move(80., 32. + 22. + 4., Some(0), "").unwrap();
    host.paint_frame();
    let text = service_text(&mut host).unwrap();
    assert!(
        text.starts_with("row zero text\nrow one text\nrow two text\nrow th"),
        "{text:?}"
    );
    host.mouse_up(80., 54., 0, "").unwrap();
    host.paint_frame();
}

#[test]
fn a7_a_drag_selects_across_documents_with_default_keys() {
    require_backend!();
    let second = MarkdownDocument::from_source("beta text");
    let mut host = host(400., 200., move || {
        div()
            .p(px(10.))
            .flex()
            .flex_col()
            .child(Markdown::new("first", "alpha text"))
            .child(Markdown::document("second", second.clone()))
            .into_any_element()
    });
    let text = docs_support::drag(&mut host, (11., 20.), (300., 43.));
    assert_eq!(text.as_deref(), Some("alpha text\nbeta text"));
}

#[test]
fn a7_a_drag_selects_across_two_views_of_one_document_with_default_keys() {
    require_backend!();
    let shared = MarkdownDocument::from_source("alpha text");
    let mut host = host(400., 200., move || {
        div()
            .p(px(10.))
            .flex()
            .flex_col()
            .child(Markdown::document("upper", shared.clone()))
            .child(Markdown::document("lower", shared.clone()))
            .into_any_element()
    });
    let text = docs_support::drag(&mut host, (11., 20.), (300., 43.));
    assert_eq!(text.as_deref(), Some("alpha text\nalpha text"));
}

fn service_text(host: &mut TestHost) -> Option<String> {
    docs_support::service(host).selected_text()
}

#[test]
fn a7_a_task_toggle_receives_the_marker_range_and_checked_state() {
    require_backend!();
    let toggles: Log<(std::ops::Range<usize>, bool)> = log();
    let sink = toggles.clone();
    let mut host = host(400., 100., move || {
        let sink = sink.clone();
        let mut options = MarkdownOptions::default();
        options.task = Some(TaskOptions {
            on_toggle: Some(Rc::new(move |marker, _, _| {
                sink.borrow_mut()
                    .push((marker.range.clone(), marker.checked))
            })),
        });
        doc("- [ ] todo\n- [x] done\n", options)
    });
    host.paint_frame();
    click(&mut host, 28., 31.);
    click(&mut host, 28., 57.);
    assert_eq!(toggles.borrow().as_slice(), [(2..5, false), (13..16, true)]);
    // The marker is not shown as text and the checkbox does not start a selection.
    let painted = host.painted_text().join("|");
    assert!(
        !painted.contains("[ ]") && !painted.contains("[x]"),
        "{painted}"
    );
    assert!(painted.contains("todo") && painted.contains("done"));
}

#[test]
fn a7_without_task_options_the_literal_prefix_is_ordinary_text() {
    require_backend!();
    let mut host = md_host(
        400.,
        100.,
        "- [x] done\n- [ ] todo\n",
        MarkdownOptions::default,
    );
    host.paint_frame();
    let painted = host.painted_text();
    assert!(painted.iter().any(|text| text == "[x] done"), "{painted:?}");
    assert!(painted.iter().any(|text| text == "[ ] todo"));
}

#[test]
fn a2_tables_scroll_horizontally_when_floors_exceed_the_viewport() {
    require_backend!();
    let source = "| aaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbb | cccccccccccccccccccc | dddddddddddddddddddd | eeeeeeeeeeeeeeeeeeee |\n|---|:-:|--:|---|---|\n| 1 | 2 | 3 | 4 | 5 |\n| only |\n";
    let mut host = host(300., 200., move || {
        finding("aaaa", doc(source, MarkdownOptions::default()))
    });
    host.paint_frame();
    let before = washes(&mut host)[0].1[0][0];
    host.mouse_move(100., 40., None, "").unwrap();
    host.paint_frame();
    host.scroll_wheel(100., 40., -120., 0., "").unwrap();
    host.paint_frame();
    let after = washes(&mut host)[0].1[0][0];
    assert!(
        after < before - 50.,
        "{before} -> {after}: the table viewport panned"
    );
    // A vertical wheel gesture does not move it.
    host.scroll_wheel(100., 40., 0., -60., "").unwrap();
    host.paint_frame();
    assert_eq!(washes(&mut host)[0].1[0][0], after);
}

#[test]
fn a2_table_alignment_and_missing_cells() {
    require_backend!();
    let source = "| h1 | h2 | h3 |\n|:---|:---:|---:|\n| LL | CC | RR |\n| short |\n";
    // Right-aligned text ends at the cell's right padding edge; centered text is centered.
    let find = |query: &'static str| {
        let mut host = host_for(query, source);
        host.paint_frame();
        washes(&mut host)
    };
    let right = find("RR");
    let center = find("CC");
    let left = find("LL");
    let r = right[0].1[0];
    let c = center[0].1[0];
    let l = left[0].1[0];
    // The table fills the 560 px document width: three columns, flexible by natural width.
    assert!(l[0] < c[0] && c[0] < r[0]);
    assert!(
        (r[0] + r[2] - (20. + 560. - 12.)).abs() < 1.5,
        "right aligned ends at the cell's right padding edge: {r:?}"
    );
    // Centered: the text is centered in the middle column (20 + 560 / 2 = 300).
    assert!(((c[0] + c[2] / 2.) - 300.).abs() < 1.5, "centered: {c:?}");
}

fn host_for(query: &'static str, source: &'static str) -> TestHost {
    host(600., 200., move || {
        finding(query, doc(source, MarkdownOptions::default()))
    })
}

#[test]
fn a2_disabled_formulas_show_their_source_and_display_math_falls_back_to_a_math_card() {
    require_backend!();
    let mut host = md_host(400., 300., "inline $x^2$ here\n\n$$\na+b\n$$\n", || {
        // Without the `math` feature formulas are always shown as source.
        #[cfg(feature = "math")]
        let options = MarkdownOptions {
            math: MathStyle::new().enabled(false),
            ..MarkdownOptions::default()
        };
        #[cfg(not(feature = "math"))]
        let options = MarkdownOptions::default();
        options
    });
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        painted.iter().any(|text| text.contains("$x^2$")),
        "{painted:?}"
    );
    assert!(
        painted.iter().any(|text| text == "math"),
        "the card is labelled math: {painted:?}"
    );
    assert!(painted.iter().any(|text| text == "a+b"));
}

#[test]
fn a8_a_theme_paint_change_does_not_reparse() {
    require_backend!();
    let document = MarkdownDocument::from_source("# Title\n\nsome **text** and `code`\n");
    let flip = Rc::new(std::cell::Cell::new(false));
    let (doc_for_host, flip_for_host) = (document.clone(), flip.clone());
    let mut host = host(400., 200., move || {
        let mut theme = DocumentTheme::dark();
        if flip_for_host.get() {
            theme.text = vespertine::rgb(0xff0000);
            theme.code_wash = vespertine::rgb(0x00ff00);
        }
        div()
            .child(Markdown::document("md", doc_for_host.clone()).theme(theme))
            .into_any_element()
    });
    host.paint_frame();
    let parses = document.parse_count();
    assert!(parses >= 1);
    flip.set(true);
    host.paint_frame();
    host.paint_frame();
    assert_eq!(
        document.parse_count(),
        parses,
        "no reparse on a theme change"
    );
}

#[test]
fn a8_streaming_work_per_frame_is_proportional_to_the_tail_block() {
    require_backend!();
    let document =
        MarkdownDocument::from_source("# One\n\nsecond para\n\nthird para\n\nfourth para");
    let shared = document.clone();
    let mut host = host(400., 300., move || {
        div()
            .child(Markdown::document("md", shared.clone()).streaming(true))
            .into_any_element()
    });
    host.paint_frame();
    let cache = document.flatten_cache();
    let misses = cache.borrow().misses();
    assert!(misses >= 4);
    host.paint_frame();
    assert_eq!(
        cache.borrow().misses(),
        misses,
        "an unchanged document flattens nothing"
    );
    document.append(" grows");
    host.paint_frame();
    assert_eq!(
        cache.borrow().misses(),
        misses + 1,
        "only the tail block flattens"
    );
    assert_eq!(document.stable_prefix_blocks(), 3);
    let parses = document.parse_count();
    document.append(" more");
    assert_eq!(document.parse_count(), parses + 1);
    assert_eq!(
        document.source(),
        "# One\n\nsecond para\n\nthird para\n\nfourth para grows more"
    );
}

#[test]
fn a8_ending_the_stream_shows_the_unrepaired_source() {
    require_backend!();
    let document = MarkdownDocument::from_source("alpha\n\nbeta **open");
    let streaming = Rc::new(std::cell::Cell::new(true));
    let flag = streaming.clone();
    let mut host = host(400., 200., move || {
        div()
            .child(Markdown::document("md", document.clone()).streaming(flag.get()))
            .into_any_element()
    });
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        !painted.iter().any(|text| text.contains("**")),
        "{painted:?}"
    );
    streaming.set(false);
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        painted.iter().any(|text| text == "beta **open"),
        "{painted:?}"
    );
}

#[test]
fn a2_nested_lists_indent_by_the_marker_column_plus_gap_and_number_from_the_start() {
    require_backend!();
    let mut host = host(400., 200., || {
        finding(
            "x",
            doc(
                "3. x one\n4. x two\n   - x nested\n\n- x bullet\n",
                MarkdownOptions::default(),
            ),
        )
    });
    host.paint_frame();
    let washes = washes(&mut host);
    let xs: Vec<f32> = washes.iter().map(|(_, rects)| rects[0][0]).collect();
    assert_eq!(washes.len(), 4);
    // Item text starts after the 18 px marker column and the 8 px gap.
    assert!((xs[0] - (20. + 26.)).abs() < 0.6, "{xs:?}");
    assert!((xs[1] - (20. + 26.)).abs() < 0.6);
    // The nested list indents by the marker column plus the gap again.
    assert!((xs[2] - (20. + 52.)).abs() < 0.6, "{xs:?}");
    assert!((xs[3] - (20. + 26.)).abs() < 0.6);
    // Ordered markers count from the start number and are chrome.
    let painted = docs_support::service(&mut host).painted_text();
    let markers: Vec<_> = painted
        .iter()
        .filter(|text| text.chrome)
        .map(|text| text.text.clone())
        .collect();
    assert_eq!(markers, ["3.", "4."]);
}

#[test]
fn a8_the_code_card_numbers() {
    require_backend!();
    // A label-only header (no actions): bottom border, text 11, vertical padding 5.
    let mut host = host(400., 200., || {
        finding(
            "answer",
            doc(
                "```rust\nlet answer = 42;\n```\n",
                MarkdownOptions::default(),
            ),
        )
    });
    host.paint_frame();
    let wash = washes(&mut host)[0].1[0];
    // x: document 20 + card border 1 + body padding 12.
    assert!(
        (wash[0] - (20. + 1. + 12. + 4. * 7.5)).abs() < 12.,
        "{wash:?}"
    );
    let image = host.screenshot_image().unwrap();
    let theme = DocumentTheme::dark();
    // The card border is the border token over the window background at its left edge.
    let edge = pixel(&image, 20.25, 70.);
    assert!(
        !close(edge, [0x18, 0x18, 0x18], 2),
        "border painted: {edge:?}"
    );
    // The card fill is white at alpha 0.035 over the window background.
    let fill = pixel(&image, 200., wash[1] + 4.);
    let expected = (0x18 as f32 + (255. - 0x18 as f32) * 0.035).round() as u8;
    assert!(close(fill, [expected; 3], 4), "{fill:?} vs {expected}");
    // Controls make the header 28 px high.
    let mut host = host_with_controls();
    host.paint_frame();
    let wash = washes(&mut host)[0].1[0];
    // card top 20 + border 1 + header 28 (its bottom border included) + body padding 10.
    assert!((wash[1] - (20. + 1. + 28. + 10.)).abs() < 0.6, "{wash:?}");
    let _ = theme;
}

fn host_with_controls() -> TestHost {
    host(400., 200., || {
        let mut options = MarkdownOptions::default();
        options.code.on_copy = Some(Rc::new(|_, _, _, _| {}));
        finding("answer", doc("```rust\nlet answer = 42;\n```\n", options))
    })
}

fn mermaid_host(view: impl Fn() -> DiagramView + 'static, frames: Log<String>) -> TestHost {
    host(500., 300., move || {
        let (frames, view) = (frames.clone(), view());
        let view = Rc::new(std::cell::RefCell::new(Some(view)));
        let mut options = MarkdownOptions::default();
        options.media.diagram = Some(Rc::new(move |_source, frame, _, _, _| {
            frames.borrow_mut().push(frame.to_owned());
            view.borrow_mut().take().unwrap_or(DiagramView::Source)
        }));
        options.row_key = Some("row7".into());
        doc("intro\n\n```Mermaid\ngraph TD\n  A-->B\n```\n", options)
    })
}

#[test]
fn a2_mermaid_fences_go_to_the_diagram_handler_with_a_frame_id() {
    require_backend!();
    let frames: Log<String> = log();
    let mut host = mermaid_host(|| DiagramView::Source, frames.clone());
    host.paint_frame();
    assert_eq!(frames.borrow()[0], "row7-mermaid-1");
    let painted = host.painted_text();
    assert!(painted.iter().any(|text| text == "Mermaid"), "{painted:?}");
    assert!(painted.iter().any(|text| text == "graph TD"));
    // Without a handler the fence is an ordinary code block.
    let mut plain = md_host(
        500.,
        300.,
        "```mermaid\ngraph TD\n```\n",
        MarkdownOptions::default,
    );
    plain.paint_frame();
    assert!(plain.painted_text().iter().any(|text| text == "graph TD"));
}

#[test]
fn a2_nested_mermaid_fences_get_distinct_frame_ids() {
    require_backend!();
    let frames: Log<String> = log();
    let sink = frames.clone();
    let mut host = host(500., 400., move || {
        let sink = sink.clone();
        let mut options = MarkdownOptions::default();
        options.media.diagram = Some(Rc::new(move |_source, frame, _, _, _| {
            sink.borrow_mut().push(frame.to_owned());
            DiagramView::Source
        }));
        options.row_key = Some("row7".into());
        doc(
            "> ```mermaid\n> graph TD\n> ```\n>\n> ```mermaid\n> graph LR\n> ```\n",
            options,
        )
    });
    host.paint_frame();
    let frames: std::collections::BTreeSet<String> = frames.borrow().iter().cloned().collect();
    assert_eq!(frames.len(), 2, "one frame id per fence: {frames:?}");
    assert!(
        frames
            .iter()
            .all(|frame| frame.starts_with("row7-mermaid-0"))
    );
}

#[test]
fn a2_a_failed_diagram_keeps_the_source_and_shows_a_warning_marker() {
    require_backend!();
    let frames: Log<String> = log();
    let mut host = mermaid_host(
        || DiagramView::Failed("Diagram could not be rendered".into()),
        frames,
    );
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        painted.iter().any(|text| text == "graph TD"),
        "the source is never hidden"
    );
}

#[test]
fn a2_a_rendered_diagram_toggles_with_its_source() {
    require_backend!();
    let toggled = Rc::new(std::cell::Cell::new(0));
    let show_source = Rc::new(std::cell::Cell::new(false));
    let (count, flag) = (toggled.clone(), show_source.clone());
    let frames: Log<String> = log();
    let mut host = mermaid_host(
        move || {
            let count = count.clone();
            DiagramView::Diagram {
                body: div().child("DIAGRAM BODY").into_any_element(),
                show_source: flag.get(),
                toggle: Rc::new(move |_, _| count.set(count.get() + 1)),
            }
        },
        frames,
    );
    host.paint_frame();
    let painted = host.painted_text().join("|");
    assert!(
        painted.contains("DIAGRAM BODY") && !painted.contains("graph TD"),
        "{painted}"
    );
    // The toggle action sits in the card header (the only action: right-aligned).
    host.mouse_move(463., 20. + 22. + 12. + 1. + 14., None, "")
        .unwrap();
    host.paint_frame();
    host.mouse_down(463., 20. + 22. + 12. + 1. + 14., 0, "")
        .unwrap();
    host.mouse_up(463., 20. + 22. + 12. + 1. + 14., 0, "")
        .unwrap();
    host.paint_frame();
    assert_eq!(toggled.get(), 1);
    show_source.set(true);
    host.paint_frame();
    let painted = host.painted_text().join("|");
    assert!(
        painted.contains("graph TD") && !painted.contains("DIAGRAM BODY"),
        "{painted}"
    );
}

#[cfg(feature = "math")]
#[test]
fn r13_typeset_inline_formulas_reserve_slots_and_copy_as_delimited_source() {
    require_backend!();
    let mut host = md_host(
        500.,
        120.,
        "before $\\frac{a}{b}$ after",
        MarkdownOptions::default,
    );
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        painted.iter().all(|text| !text.contains('$')),
        "the placeholder characters are painted, never the source: {painted:?}"
    );
    assert!(painted.iter().any(|text| text.contains('\u{a0}')));
    let copied = docs_support::drag(&mut host, (21., 31.), (470., 31.)).expect("a selection");
    assert_eq!(copied, "before $\\frac{a}{b}$ after");
}

#[cfg(feature = "math")]
#[test]
fn r13_display_formulas_are_blocks_of_their_own_that_copy_with_double_dollars() {
    require_backend!();
    let mut host = md_host(
        640.,
        160.,
        "above\n\n$$x^2+y^2$$\n\nbelow",
        MarkdownOptions::default,
    );
    host.paint_frame();
    let painted = host.painted_text();
    assert!(
        painted.iter().all(|text| !text.contains('$')),
        "{painted:?}"
    );
    // 20 + 22 (above) + 12 gap, then a line box of at least 22 and the formula centered in 600.
    // A drag across the slot (the formula is centered in the 600 px column) is one unit of the
    // original text.
    let copied = docs_support::drag(&mut host, (200., 66.), (440., 66.)).expect("a selection");
    assert_eq!(copied, "$$x^2+y^2$$");
}

#[cfg(feature = "math")]
#[test]
fn r13_a_tall_inline_formula_opens_the_line_height_of_its_paragraph() {
    require_backend!();
    let next_y = |source: &'static str| {
        let mut host = host_for("next", source);
        host.paint_frame();
        washes(&mut host)[0].1[0][1]
    };
    let plain = next_y("x y\n\nnext");
    let tall = next_y("x $\\frac{\\frac{a}{b}}{\\frac{c}{d}}$ y\n\nnext");
    assert!(tall > plain + 4., "{plain} -> {tall}");
    let short = next_y("x $a$ y\n\nnext");
    assert_eq!(
        short, plain,
        "a paragraph with small formulas keeps its rhythm"
    );
}

#[cfg(feature = "math")]
#[test]
fn r14_a_column_holding_a_formula_floors_at_its_widest_unbreakable_word() {
    require_backend!();
    let formula = "$a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t+u+v+w+x+y+z$";
    let pans = |source: &'static str| {
        let mut host = host(260., 160., move || {
            finding("tag", doc(source, MarkdownOptions::default()))
        });
        host.paint_frame();
        let before = washes(&mut host)[0].1[0][0];
        host.mouse_move(100., 40., None, "").unwrap();
        host.paint_frame();
        host.scroll_wheel(100., 40., -120., 0., "").unwrap();
        host.paint_frame();
        washes(&mut host)[0].1[0][0] < before - 20.
    };
    let with_formula: &'static str =
        Box::leak(format!("| tag | f |\n|---|---|\n| tag | {formula} |\n").into_boxed_str());
    let with_text: &'static str = Box::leak(
        format!(
            "| tag | f |\n|---|---|\n| tag | {} |\n",
            "a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t+u+v+w+x+y+z".replace('+', "")
        )
        .into_boxed_str(),
    );
    assert!(
        pans(with_formula),
        "the formula column keeps its width and the table scrolls"
    );
    assert!(
        !pans(with_text),
        "plain text of a similar width compresses to the 96 px floor"
    );
}

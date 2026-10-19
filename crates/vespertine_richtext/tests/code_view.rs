//! The code view as a native document widget: its lines are selectable runs (gutters are chrome)
//! and take part in search washes.

mod support;

use std::rc::Rc;
use support::{Build, Harness, at};
use vespertine::{IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec};
use vespertine_richtext::{CodeView, RichText, highlight, text_root};

const SOURCE: &str = "fn main() {\n\n    run();\n    run();\n}";

fn code(gutter: bool) -> vespertine::AnyElement {
    let view = CodeView::new(SOURCE)
        .id("code")
        .language("rust")
        .font_family("Liberation Mono")
        .font_size(px(14.))
        .line_height(px(20.));
    let body = div().flex().flex_row();
    let body = if gutter {
        body.child(
            div()
                .w(px(30.))
                .font_family("Liberation Mono")
                .text_size(px(14.))
                .line_height(px(20.))
                .children(
                    (1..=5usize).map(|n| RichText::new(("gutter", n), format!("{n}")).chrome()),
                ),
        )
    } else {
        body
    };
    body.child(view).into_any_element()
}

fn page(child: vespertine::AnyElement) -> vespertine::AnyElement {
    text_root(
        div()
            .size_full()
            .bg(rgb(0x181818))
            .text_color(rgb(0xeeeeee))
            .child(child),
    )
    .into_any_element()
}

#[test]
fn selecting_across_code_lines_keeps_blank_lines_and_excludes_the_gutter() {
    let build: Build = Rc::new(|_, _| page(code(true)));
    let mut harness = Harness::new(300., 140., build);
    harness.drag(at(30., 10.), at(80., 90.));
    let text = harness.service().selected_text().unwrap();
    assert!(text.starts_with("fn main() {\n\n    run();"), "{text:?}");
    assert!(text.contains("\n\n"), "the blank line survives");
    assert!(
        !text.contains('1') && !text.contains('2'),
        "gutter numbers stay out: {text:?}"
    );
    let painted = harness.service().painted_text();
    assert_eq!(painted.iter().filter(|entry| entry.chrome).count(), 5);
    assert_eq!(
        harness.service().registered_runs(),
        5,
        "one run per code line"
    );
}

#[test]
fn search_washes_apply_to_the_strings_code_lines_paint() {
    let build: Build = Rc::new(|_, _| {
        page(
            highlight(
                "find",
                HighlightDeclaration::from_spec(
                    HighlightSpec::new("run").with_active_index(Some(1)),
                ),
                code(false),
            )
            .into_any_element(),
        )
    });
    let mut harness = Harness::new(300., 140., build);
    harness.settle();
    let washes = harness.service().painted_highlights();
    assert_eq!(washes.len(), 2, "{washes:?}");
    assert_eq!(washes[0].text, "    run();");
    assert!(!washes[0].active && washes[1].active);
    assert!(washes.iter().all(|wash| (wash.start, wash.end) == (4, 7)));
}

//! CMP-009 editor-level tests: the Markdown layer attached to the multiline editor.

mod common_area;

use common_area::fixture;
use vespertine_components::markdown::MarkdownOptions;
use vespertine_test_host::require_backend;

const LONG_ITEM: &str =
    "- an item with a long text that has to wrap over several rows of the editor";

#[test]
fn a2_styling_never_removes_delimiters_or_alters_copied_source() {
    require_backend!();
    let source = "**bold** and ~~gone~~ and `code`\nother line";
    let mut fx = fixture(400.0, 200.0, false, move |area| {
        area.value(source).with_markdown(MarkdownOptions::default())
    });
    fx.focus();
    // The caret is on the second line: every delimiter of the first line is hidden.
    assert_eq!(
        fx.read(|area| area.display_text().to_owned()),
        "bold and gone and code\nother line"
    );
    fx.keys("ctrl-a ctrl-c");
    assert_eq!(fx.clipboard_text().as_deref(), Some(source));
    assert_eq!(fx.text(), source, "the source is never rewritten");
    // Moving the caret onto the line shows every delimiter again.
    fx.select(3..3);
    assert_eq!(fx.read(|area| area.display_text().to_owned()), source);
    // The caret mapping stays exact on decorated lines.
    fx.select(source.len()..source.len());
    let end = fx.read(|area| area.caret_rect().unwrap());
    fx.select(0..0);
    fx.keys("down");
    assert!(fx.read(|area| area.caret_rect().unwrap()).top() > end.top() - end.size.height);
}

#[test]
fn enter_continues_lists_and_an_empty_item_leaves_the_list() {
    require_backend!();
    let mut fx = fixture(400.0, 200.0, false, |area| {
        area.value("1. one")
            .with_markdown(MarkdownOptions::default())
    });
    fx.focus();
    fx.keys("enter");
    assert_eq!(fx.text(), "1. one\n2. ");
    fx.keys("enter");
    assert_eq!(
        fx.text(),
        "1. one\n",
        "an empty item is replaced by the outdented prefix"
    );
    fx.set_value("> - [x] done");
    fx.keys("enter");
    assert_eq!(fx.text(), "> - [x] done\n> - [ ] ");
    fx.set_value("- item\r\nnext");
    fx.select(6..6);
    fx.keys("enter");
    assert_eq!(fx.text(), "- item\r\n- \r\nnext");
    // No edit in code: a plain newline.
    fx.set_value("```\n- item");
    fx.keys("enter");
    assert_eq!(fx.text(), "```\n- item\n");
}

#[test]
fn a7_tab_indents_selected_list_lines_in_one_undo_step_and_propagates_otherwise() {
    require_backend!();
    let mut fx = fixture(400.0, 200.0, false, |area| {
        area.value("- a\n- b\n- c")
            .with_markdown(MarkdownOptions::default())
    });
    fx.focus();
    fx.keys("ctrl-a");
    fx.keys("tab");
    assert_eq!(fx.text(), "  - a\n  - b\n  - c");
    assert_eq!(fx.selection(), 2..17, "endpoints shift by the added widths");
    fx.keys("shift-tab");
    assert_eq!(fx.text(), "- a\n- b\n- c");
    fx.keys("tab");
    assert_eq!(
        fx.read(|area| area.core().undo_len()),
        3,
        "one undo step per operation"
    );
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "- a\n- b\n- c");
    // A non-list line in the selection: unchanged, and the key moves focus on.
    fx.set_value("- a\nplain");
    fx.keys("ctrl-a");
    fx.keys("tab");
    assert_eq!(fx.text(), "- a\nplain");
}

#[test]
fn a8_wrapped_list_rows_are_indented_and_hit_testing_follows() {
    require_backend!();
    let mut fx = fixture(220.0, 300.0, false, |area| {
        area.value(LONG_ITEM)
            .with_markdown(MarkdownOptions::default())
    });
    fx.select(LONG_ITEM.len()..LONG_ITEM.len());
    fx.select(0..0);
    let rows = fx.read(|area| area.visual_rows());
    assert!(rows.len() >= 3, "the item wraps: {rows:?}");
    let bounds = fx.read(|area| area.viewport_bounds().unwrap());
    // The first row starts at the left edge with the bullet; continuation rows are indented.
    let first = fx.point_of(0);
    let continuation = fx.point_of(rows[1].source_start);
    assert_eq!(first.x, bounds.left());
    assert!(
        continuation.x > bounds.left() + vespertine::px(4.0),
        "{continuation:?}"
    );
    assert!(
        continuation.x - bounds.left() <= bounds.size.width * 0.4 + vespertine::px(0.5),
        "the indent is capped at 40% of the width"
    );
    // Clicks on continuation rows map to the right source offsets.
    for index in [rows[1].source_start + 3, rows[2].source_start + 5] {
        let at = fx.row_point(index, 0.5);
        assert_eq!(fx.read(|area| area.index_for_point(at)), Some(index));
    }
}

#[test]
fn a6_sources_over_the_limit_are_plain_while_continuation_still_works() {
    require_backend!();
    let filler = "- **é**\n".repeat(20_000);
    assert!(filler.len() > 131_072);
    let source = filler.clone();
    let mut fx = fixture(400.0, 200.0, false, move |area| {
        area.value(&source)
            .with_markdown(MarkdownOptions::default())
    });
    fx.focus();
    let display = fx.read(|area| area.display_text().len());
    assert_eq!(display, filler.len(), "no decorations above the limit");
    fx.keys("ctrl-end");
    fx.type_text("- item");
    fx.keys("enter");
    assert!(fx.text().ends_with("- item\n- "));
}

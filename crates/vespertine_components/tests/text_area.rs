//! CMP-008 acceptance tests: the multiline editor in the offscreen test host.

mod common_area;

use common_area::{Fixture, fixture};
use vespertine::{ClipboardItem, EntityInputHandler as _, point, px};
use vespertine_components::text::{
    SubmitMode, TextAreaEvent,
    area::{ExternalOutcome, ExternalPhase, Replacement},
};
use vespertine_richtext::TextService;
use vespertine_test_host::require_backend;

const WRAPPED: &str = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu";

#[test]
fn a1_composition_commits_and_one_undo_restores_the_pre_composition_source() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| area.value("abc"));
    fx.focus();
    fx.select(1..1);
    for marked in ["に", "日本"] {
        let marked = marked.to_owned();
        fx.with(|area, window, cx| {
            area.replace_and_mark_text_in_range(None, &marked, None, window, cx)
        });
    }
    fx.with(|area, window, cx| area.replace_text_in_range(None, "日本語", window, cx));
    assert_eq!(fx.text(), "a日本語bc");
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "abc");
    assert_eq!(fx.selection(), 1..1);
    // The same with a non-empty selection being replaced.
    fx.select(1..2);
    fx.with(|area, window, cx| area.replace_and_mark_text_in_range(None, "に", None, window, cx));
    fx.with(|area, window, cx| area.replace_text_in_range(None, "日本語", window, cx));
    assert_eq!(fx.text(), "a日本語c");
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "abc");
    assert_eq!(fx.selection(), 1..2);
    fx.keys("ctrl-shift-z");
    assert_eq!(fx.text(), "a日本語c");
}

#[test]
fn a2_selecting_across_a_soft_wrap_gives_the_expected_rectangles_and_range() {
    require_backend!();
    let mut fx = fixture(200.0, 200.0, false, |area| area.value(WRAPPED));
    fx.focus();
    fx.select(3..3);
    let rows = fx.read(|area| area.visual_rows());
    assert!(rows.len() >= 3, "the line wraps: {rows:?}");
    fx.keys("shift-down");
    let range = fx.selection();
    assert_eq!(range.start, 3);
    assert!(range.end > rows[1].source_start && range.end <= rows[2].source_start + 1);
    let (rects, bounds) = fx.read(|area| (area.selection_rects(), area.viewport_bounds().unwrap()));
    assert_eq!(
        rects.len(),
        2,
        "first row to the right edge and last row from the left: {rects:?}"
    );
    assert_eq!(rects[0].right(), bounds.right());
    assert_eq!(rects[1].left(), bounds.left());
    assert_eq!(f32::from(rects[0].size.height), 20.0);
    assert_eq!(rects[1].top() - rects[0].top(), px(20.0));

    // The mouse produces the same source range.
    fx.select(0..0);
    let start = fx.row_point(3, 0.5);
    let end = fx.row_point(range.end, 0.5);
    fx.press_at(start, 1, false);
    fx.drag_at(end);
    fx.release_at(end);
    assert_eq!(fx.selection(), range);
    let mouse_rects = fx.read(|area| area.selection_rects());
    assert_eq!(mouse_rects, rects);
}

#[test]
fn a2_click_past_a_row_end_keeps_the_row_by_affinity_and_never_splits_crlf() {
    require_backend!();
    let mut fx = fixture(200.0, 200.0, false, |area| area.value(WRAPPED));
    fx.focus();
    let rows = fx.read(|area| area.visual_rows());
    let first_end = rows[1].source_start;
    let past_end = point(px(190.0), fx.point_of(0).y + px(10.0));
    fx.press_at(past_end, 1, false);
    fx.release_at(past_end);
    assert_eq!(fx.selection(), first_end..first_end);
    let caret = fx.read(|area| area.caret_rect().unwrap());
    assert_eq!(
        caret.top(),
        rows_top(&mut fx, 0),
        "the caret stays on the upper row"
    );
    // The same offset with downstream affinity lives on the lower row.
    fx.keys("right left");
    let caret = fx.read(|area| area.caret_rect().unwrap());
    assert_eq!(caret.top(), rows_top(&mut fx, 1));

    fx.set_value("first line\r\nsecond line");
    let past_end = point(px(190.0), fx.point_of(0).y + px(10.0));
    fx.press_at(past_end, 1, false);
    fx.release_at(past_end);
    assert_eq!(
        fx.selection(),
        10..10,
        "a click past the end lands before the CR"
    );
    fx.keys("down");
    fx.keys("home");
    assert_eq!(fx.selection(), 12..12);
}

fn rows_top(fx: &mut Fixture, row: usize) -> vespertine::Pixels {
    let bounds = fx.read(|area| area.viewport_bounds().unwrap());
    bounds.top() + px(20.0 * row as f32)
}

#[test]
fn a3_drag_autoscroll_steps_follow_the_distance_and_stale_generations_do_nothing() {
    require_backend!();
    let lines: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut fx = fixture(300.0, 400.0, false, move |area| {
        area.value(&lines).rows(1, 5)
    });
    fx.focus();
    fx.select(0..0);
    let bounds = fx.read(|area| area.viewport_bounds().unwrap());
    let start = point(bounds.left() + px(0.5), bounds.top() + px(10.0));
    fx.press_at(start, 1, false);
    let generation = fx.read(|area| area.pointer_generation());
    for (distance, expected) in [(10.0, 2.0), (100.0, 20.0), (1000.0, 20.0)] {
        let below = point(bounds.left() + px(30.0), bounds.bottom() + px(distance));
        fx.drag_at(below);
        let before = fx.read(|area| f32::from(area.scroll_offset()));
        fx.host.advance_executor_time(16.0).unwrap();
        fx.host.flush();
        let after = fx.read(|area| f32::from(area.scroll_offset()));
        assert!(
            (after - before - expected).abs() < 0.01,
            "{distance} px below: {before} -> {after}, expected {expected}"
        );
    }
    let selection = fx.selection();
    assert_eq!(selection.start, 0);
    assert!(selection.end > 20, "the selection end follows the pointer");
    // Re-entering stops the ticks.
    fx.drag_at(point(bounds.left() + px(30.0), bounds.top() + px(30.0)));
    let stopped = fx.read(|area| area.scroll_offset());
    fx.host.advance_executor_time(160.0).unwrap();
    assert_eq!(fx.read(|area| area.scroll_offset()), stopped);
    // Releasing ends the gesture; a tick of the old generation does nothing.
    fx.release_at(start);
    let moved = fx.with(|area, window, cx| area.autoscroll_tick(generation, window, cx));
    assert!(!moved);
    assert_eq!(fx.read(|area| area.scroll_offset()), stopped);
}

#[test]
fn a4_redo_invalidation_paste_steps_and_typing_coalescing() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| area);
    fx.focus();
    for ch in ["a", "b", "c"] {
        fx.type_text(ch);
    }
    assert_eq!(
        fx.read(|area| area.core().undo_len()),
        1,
        "typing within 700 ms is one step"
    );
    fx.keys("enter");
    fx.type_text("d");
    assert_eq!(
        fx.read(|area| area.core().undo_len()),
        3,
        "a newline starts a new step"
    );
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "abc\n");
    fx.type_text("x");
    assert_eq!(
        fx.read(|area| area.core().redo_len()),
        0,
        "a new edit invalidates redo"
    );
    fx.with(|area, window, cx| {
        area.paste_item(&ClipboardItem::new_string("p".into()), window, cx);
        area.paste_item(&ClipboardItem::new_string("q".into()), window, cx);
    });
    assert_eq!(fx.text(), "abc\nxpq");
    let steps = fx.read(|area| area.core().undo_len());
    fx.keys("ctrl-z ctrl-z");
    assert_eq!(fx.text(), "abc\nx");
    assert_eq!(steps - fx.read(|area| area.core().undo_len()), 2);
}

#[test]
fn a5_modal_editor_isolates_its_selection_and_copy_from_the_document() {
    require_backend!();
    let mut fx = fixture(400.0, 300.0, true, |area| area.value("editor text here"));
    fx.focus();
    let start = fx.row_point(0, 1.0);
    let end = fx.row_point(6, 1.0);
    fx.press_at(start, 1, false);
    fx.drag_at(end);
    fx.release_at(end);
    assert_eq!(fx.selection(), 0..6);
    let window = fx.host.window();
    let selected = fx
        .host
        .app()
        .update_window(window, |_, window, cx| {
            TextService::of(window, cx).selected_text()
        })
        .unwrap();
    assert_eq!(selected, None, "the document selection stays empty");
    fx.keys("ctrl-c");
    assert_eq!(fx.clipboard_text().as_deref(), Some("editor"));
}

#[test]
fn a6_auto_grow_stops_at_ten_rows_and_scrolls_internally() {
    require_backend!();
    let mut fx = fixture(300.0, 400.0, false, |area| area);
    for (rows, expected) in [(1, 20.0), (5, 100.0), (10, 200.0), (25, 200.0)] {
        let value = vec!["row"; rows].join("\n");
        fx.set_value(&value);
        fx.host.flush();
        assert_eq!(
            f32::from(fx.read(|area| area.measured_height())),
            expected,
            "{rows} rows"
        );
    }
    // Wrapped rows count the same as hard lines.
    fx.set_value(&WRAPPED.repeat(6));
    let rows = fx.read(|area| area.visual_rows().len());
    assert!(rows > 10);
    assert_eq!(f32::from(fx.read(|area| area.measured_height())), 200.0);

    fx.set_value(&vec!["row"; 25].join("\n"));
    assert_eq!(f32::from(fx.read(|area| area.content_height())), 500.0);
    fx.with(|area, _, cx| area.set_scroll_offset(px(999.0), cx));
    assert_eq!(
        f32::from(fx.read(|area| area.scroll_offset())),
        300.0,
        "maximum scroll"
    );
    // Caret follow: minimal adjustment from the top to the last row and back.
    fx.focus();
    fx.keys("ctrl-home");
    assert_eq!(f32::from(fx.read(|area| area.scroll_offset())), 0.0);
    fx.keys("ctrl-end");
    assert_eq!(f32::from(fx.read(|area| area.scroll_offset())), 300.0);
    fx.keys("up");
    assert_eq!(
        f32::from(fx.read(|area| area.scroll_offset())),
        300.0,
        "visible rows do not scroll"
    );
    fx.keys("ctrl-home");
    assert_eq!(f32::from(fx.read(|area| area.scroll_offset())), 0.0);

    // min_rows 0 is clamped to 1; max_rows to at least min_rows.
    let mut fx = fixture(300.0, 400.0, false, |area| area.rows(0, 0));
    fx.set_value("a\nb\nc");
    assert_eq!(f32::from(fx.read(|area| area.measured_height())), 20.0);
}

#[test]
fn a7_overflow_edges_follow_the_settled_height() {
    require_backend!();
    let mut fx = fixture(300.0, 400.0, false, |area| area);
    fx.set_value(&vec!["row"; 25].join("\n"));
    fx.with(|area, _, cx| area.set_scroll_offset(px(0.0), cx));
    let edges = fx.read(|area| area.overflow_edges());
    assert!(!edges.top_hidden && edges.bottom_hidden);
    fx.with(|area, _, cx| area.set_scroll_offset(px(100.0), cx));
    let edges = fx.read(|area| area.overflow_edges());
    assert!(edges.top_hidden && edges.bottom_hidden);
    fx.with(|area, _, cx| area.set_scroll_offset(px(300.0), cx));
    let edges = fx.read(|area| area.overflow_edges());
    assert!(edges.top_hidden && !edges.bottom_hidden);
    // Judged against the settled height: 500 px of content in a settled 499.5 px viewport is
    // within the 1 px tolerance.
    fx.with(|area, _, cx| {
        area.set_scroll_offset(px(0.0), cx);
        area.set_settled_viewport_height(Some(px(499.5)), cx);
    });
    let edges = fx.read(|area| area.overflow_edges());
    assert!(!edges.top_hidden && !edges.bottom_hidden);
}

#[test]
fn a8_external_producer_replaces_a_selected_range_as_one_undo_edit() {
    require_backend!();
    let original = "👩🏽‍💻 replace café";
    let mut fx = fixture(300.0, 200.0, false, move |area| area.value(original));
    fx.focus();
    let start = original.find("replace").unwrap();
    let selection = start..start + "replace".len();
    fx.select(selection.clone());
    fx.with(|area, _, cx| area.begin_external_replacement(None, cx));
    assert_eq!(
        fx.read(|area| area.external_phase()),
        ExternalPhase::Starting
    );
    for text in ["你好", "bonjour 🌍"] {
        let outcome = fx.with(|area, window, cx| area.apply_provisional(text, window, cx));
        assert_eq!(outcome, ExternalOutcome::Applied);
    }
    assert_eq!(
        fx.read(|area| area.external_phase()),
        ExternalPhase::Listening
    );
    let outcome = fx.with(|area, window, cx| area.finalize("salut 🌍", window, cx));
    assert_eq!(outcome, ExternalOutcome::Applied);
    assert_eq!(fx.text(), "👩🏽‍💻 salut 🌍 café");
    assert_eq!(
        fx.read(|area| area.core().undo_len()),
        1,
        "exactly one undo step"
    );
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), original);
    assert_eq!(fx.selection(), selection);
    fx.keys("ctrl-shift-z");
    assert_eq!(fx.text(), "👩🏽‍💻 salut 🌍 café");

    // An empty final keeps the latest provisional text.
    fx.set_value(original);
    fx.select(selection.clone());
    fx.with(|area, _, cx| area.begin_external_replacement(None, cx));
    fx.with(|area, window, cx| area.apply_provisional("hola", window, cx));
    fx.with(|area, window, cx| area.finalize("  ", window, cx));
    assert!(fx.text().contains("hola"));

    // A manual edit after a partial cancels the transaction; a stale final is ignored.
    fx.set_value(original);
    fx.select(selection.clone());
    fx.with(|area, _, cx| area.begin_external_replacement(None, cx));
    let generation = fx.read(|area| area.external_generation());
    fx.with(|area, window, cx| area.apply_provisional("hola", window, cx));
    let cursor = fx.read(|area| area.cursor_offset());
    fx.with(|area, window, cx| area.replace_range(cursor..cursor, "!", window, cx));
    assert_ne!(fx.read(|area| area.external_generation()), generation);
    let outcome =
        fx.with(|area, window, cx| area.finalize_for_generation(generation, "never", window, cx));
    assert_eq!(outcome, ExternalOutcome::Stale);
    assert!(!fx.text().contains("never"));
    fx.keys("ctrl-z");
    assert!(fx.text().contains("hola") && !fx.text().contains('!'));
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), original);

    // A draft swap rejects late results.
    fx.select(selection);
    fx.with(|area, _, cx| area.begin_external_replacement(None, cx));
    let generation = fx.read(|area| area.external_generation());
    fx.set_value("another draft");
    let outcome = fx.with(|area, window, cx| {
        area.apply_provisional_for_generation(generation, "late", window, cx)
    });
    assert!(matches!(
        outcome,
        ExternalOutcome::Stale | ExternalOutcome::Inactive
    ));
    assert_eq!(fx.text(), "another draft");
}

#[test]
fn a9_atomic_projection_snaps_widens_and_is_crossed_by_one_press() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| area.value("ab0123456789cd"));
    fx.with(|area, _, cx| {
        area.set_projection(
            vec![Replacement {
                range: 2..12,
                display: "CHIP".into(),
                atomic: true,
            }],
            cx,
        )
    });
    fx.focus();
    assert_eq!(fx.read(|area| area.display_text().to_owned()), "abCHIPcd");
    let projection = fx.read(|area| area.projection().clone());
    assert_eq!(projection.source_to_display(7), 2);
    assert_eq!(projection.source_to_display(12), 6);
    // The caret at 40% / 60% of the chip snaps to the start / end.
    fx.select(6..6);
    assert_eq!(fx.selection(), 2..2);
    fx.select(8..8);
    assert_eq!(fx.selection(), 12..12);
    // A selection touching the chip is widened.
    fx.select(1..5);
    assert_eq!(fx.selection(), 1..12);
    fx.select(2..2);
    fx.keys("right");
    assert_eq!(fx.selection(), 12..12, "one Right press crosses the chip");
    fx.keys("backspace");
    assert_eq!(fx.text(), "abcd", "Backspace after a chip deletes it whole");
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "ab0123456789cd");
    // Double click selects the chip.
    let at = fx.row_point(2, 5.0);
    fx.press_at(at, 2, false);
    fx.release_at(at);
    assert_eq!(fx.selection(), 2..12);
}

#[test]
fn a10_newline_follows_the_line_terminator_and_the_submit_mode() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| area.value("one\r\ntwo"));
    fx.focus();
    fx.select(3..3);
    fx.keys("enter");
    assert_eq!(fx.text(), "one\r\n\r\ntwo", "a CRLF line breaks with CRLF");
    fx.set_value("one\ntwo");
    fx.select(1..1);
    fx.keys("enter");
    assert_eq!(fx.text(), "o\nne\ntwo");

    for mode in [SubmitMode::EnterSubmits, SubmitMode::ModifiedEnter] {
        fx.with(move |area, _, cx| area.set_submit_mode(mode, cx));
        fx.set_value("x");
        fx.take_events();
        fx.keys("shift-enter");
        assert_eq!(
            fx.text(),
            "x\n",
            "Shift+Enter always inserts a newline in {mode:?}"
        );
        fx.take_events();
        fx.keys("enter");
        let submitted_by_enter = fx
            .take_events()
            .iter()
            .any(|event| matches!(event, TextAreaEvent::Submitted(_)));
        assert_eq!(submitted_by_enter, mode == SubmitMode::EnterSubmits);
        fx.keys("ctrl-enter");
        let submitted_by_chord = fx
            .take_events()
            .iter()
            .any(|event| matches!(event, TextAreaEvent::Submitted(_)));
        assert_eq!(submitted_by_chord, mode == SubmitMode::ModifiedEnter);
    }
    // An open completion owns Enter.
    fx.with(|area, _, cx| area.set_submit_mode(SubmitMode::EnterSubmits, cx));
    fx.with(|area, _, _| area.set_completion_open(true));
    fx.take_events();
    let before = fx.text();
    fx.keys("enter");
    let events = fx.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TextAreaEvent::AcceptCompletion))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TextAreaEvent::Submitted(_)))
    );
    assert_eq!(fx.text(), before);
}

#[test]
fn a11_wheel_is_consumed_when_scrollable_and_triple_click_selects_the_line() {
    require_backend!();
    let lines = vec!["row"; 25].join("\n");
    let mut fx = fixture(300.0, 400.0, false, move |area| area.value(&lines));
    fx.with(|area, _, cx| area.set_scroll_offset(px(0.0), cx));
    let bounds = fx.read(|area| area.viewport_bounds().unwrap());
    let (x, y) = (
        f64::from(f32::from(bounds.left() + px(20.0))),
        f64::from(f32::from(bounds.top() + px(40.0))),
    );
    fx.host.scroll_wheel(x, y, 0.0, -50.0, "").unwrap();
    assert_eq!(f32::from(fx.read(|area| area.scroll_offset())), 50.0);
    fx.host.scroll_wheel(x, y, 0.0, -9999.0, "").unwrap();
    assert_eq!(f32::from(fx.read(|area| area.scroll_offset())), 300.0);
    let before = fx.wheel.get();
    fx.host.scroll_wheel(x, y, 0.0, -50.0, "").unwrap();
    assert_eq!(
        fx.wheel.get(),
        before,
        "a wheel at the scroll limit is still consumed"
    );

    fx.set_value("a\nb");
    let before = fx.wheel.get();
    fx.host.scroll_wheel(x, y, 0.0, -50.0, "").unwrap();
    assert_eq!(
        fx.wheel.get(),
        before + 1,
        "a non-scrollable editor lets the wheel propagate"
    );

    // Three clicks select the line with its trailing LF, except on the last line.
    fx.set_value("first\nsecond\nthird");
    fx.focus();
    let at = fx.row_point(2, 1.0);
    fx.press_at(at, 3, false);
    fx.release_at(at);
    assert_eq!(fx.selection(), 0..6);
    let last = fx.row_point(14, 1.0);
    fx.press_at(last, 3, false);
    fx.release_at(last);
    assert_eq!(fx.selection(), 13..18);
    fx.with(|area, _, _| {
        let _ = area;
    });
}

#[test]
fn a11_triple_click_selects_all_when_configured() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| {
        area.value("first\nsecond").triple_click_selects_all(true)
    });
    fx.focus();
    let at = fx.row_point(2, 1.0);
    fx.press_at(at, 3, false);
    fx.release_at(at);
    assert_eq!(fx.selection(), 0..12);
}

#[test]
fn vertical_movement_keeps_the_preferred_column_and_edges_go_to_the_document_ends() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| {
        area.value("long first line\nab\nlong third line")
    });
    fx.focus();
    fx.select(10..10);
    fx.keys("down");
    assert_eq!(fx.selection(), 18..18, "the short line ends at its end");
    fx.keys("down");
    let third = fx.selection().start;
    assert!(
        (24..=29).contains(&third),
        "the preferred column is reused: {third}"
    );
    fx.keys("down");
    assert_eq!(
        fx.selection(),
        34..34,
        "Down from the last row goes to the end"
    );
    fx.keys("up up up");
    assert_eq!(
        fx.selection(),
        0..0,
        "Up from the first row goes to offset 0"
    );
    fx.keys("shift-down");
    assert!(fx.selection().end > 0);
    fx.keys("ctrl-shift-end");
    assert_eq!(fx.selection(), 0..34);
}

#[test]
fn read_only_and_disabled_editors_keep_the_core_semantics() {
    require_backend!();
    let mut fx = fixture(300.0, 200.0, false, |area| {
        area.value("hello").read_only(true)
    });
    fx.focus();
    fx.keys("ctrl-a ctrl-c");
    assert_eq!(fx.clipboard_text().as_deref(), Some("hello"));
    for keys in ["backspace", "ctrl-x", "ctrl-v", "enter", "ctrl-z"] {
        fx.keys(keys);
    }
    fx.type_text("x");
    assert_eq!(fx.text(), "hello");
    let state = fx.host.app().update(|_| ());
    let _ = state;
    let menu = {
        let area = fx.area.clone();
        fx.host.app().update(|cx| area.read(cx).edit_menu_state(cx))
    };
    assert!(menu.copy && !menu.cut && !menu.paste && menu.select_all);
}

//! ELM-001: variable-height virtual lists.

use super::test_support::*;
use super::*;
use crate as vespertine;
use crate::TestAppContext;

fn approx(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.01,
        "expected {expected}, got {actual}"
    );
}

// R1, R2: logical dataset, mounted window, placeholders.

#[vespertine::test]
fn r1_logical_count_and_estimate_validation(_cx: &mut TestAppContext) {
    let state = ListState::new(40, ListAlignment::Top, px(0.));
    assert_eq!(state.item_count(), 40);
    assert!(!state.is_windowed());

    // A count without an estimate does not window the list.
    state.set_logical_item_count(Some(1000.));
    assert_eq!(state.item_count(), 40);

    // Only a strictly positive finite estimate activates it.
    for estimate in [px(0.), px(-3.), px(f32::NAN), px(f32::INFINITY)] {
        state.set_estimated_item_height(Some(estimate));
        assert!(
            !state.is_windowed(),
            "estimate {estimate:?} must be ignored"
        );
    }
    state.set_estimated_item_height(Some(px(24.)));
    assert!(state.is_windowed());
    assert_eq!(state.item_count(), 1000);

    // Counts are truncated; negative, NaN and infinite counts are not configured.
    state.set_logical_item_count(Some(12.9));
    assert_eq!(state.item_count(), 12);
    for invalid in [-1., f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        state.set_logical_item_count(Some(invalid));
        assert_eq!(state.item_count(), 40, "count {invalid} is invalid");
        assert!(!state.is_windowed());
    }
    state.set_logical_item_count(Some(0.));
    assert_eq!(state.item_count(), 0);
    state.set_logical_item_count(None);
    assert_eq!(state.item_count(), 40);
}

#[vespertine::test]
fn r2_window_start_clamped_and_overflowing_rows_ignored(_cx: &mut TestAppContext) {
    let state = ListState::new(40, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(10.)));
    state.set_logical_item_count(Some(45.));
    state.set_window_start(30);
    assert_eq!(state.window_start(), 30);
    // Rows 30..45 take part; the 25 mounted rows beyond the dataset are ignored.
    set_measured(&state, &[Some(20.); 40]);
    assert_eq!(state.item_offset(45), px(30. * 10. + 15. * 20.));
    assert_eq!(state.item_offset(1000), px(30. * 10. + 15. * 20.));
    assert_eq!(state.item_offset(30), px(300.));
    assert_eq!(state.item_offset(31), px(320.));

    state.set_window_start(1000);
    assert_eq!(state.window_start(), 45);
    state.set_window_start(0);
    assert_eq!(state.window_start(), 0);
}

#[vespertine::test]
fn r2_r3_placeholders_use_estimate_with_minimum_height(_cx: &mut TestAppContext) {
    let state = ListState::new(2, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(0.5)));
    state.set_logical_item_count(Some(10.));
    state.set_window_start(4);
    // Placeholders are at least one pixel high; mounted unknown rows use the estimate itself.
    assert_eq!(state.item_offset(4), px(4.));
    assert_eq!(state.item_offset(6), px(5.));
    assert_eq!(state.item_offset(10), px(9.));
    // A measured row replaces its estimate.
    set_measured(&state, &[Some(30.), None]);
    assert_eq!(state.item_offset(5), px(34.));
    assert_eq!(state.item_offset(10), px(4. + 30. + 0.5 + 4.));
}

#[vespertine::test]
fn r3_height_resolution_measurement_hint_estimate_unknown(_cx: &mut TestAppContext) {
    // Without an estimate unknown rows contribute zero.
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    assert_eq!(state.item_offset(3), px(0.));
    // A hint counts as the row height.
    state.reset_with_uniform_height(3, px(40.));
    assert_eq!(state.item_offset(3), px(120.));
    // A measurement overrides the hint.
    {
        let mut inner = state.0.borrow_mut();
        let replacement = ListItem::Measured {
            size: size(px(10.), px(25.)),
            focus_handle: None,
        };
        inner.replace_mounted_item(1, replacement);
    }
    assert_eq!(state.item_offset(3), px(105.));
    // With an estimate, rows with neither use it.
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(7.)));
    assert_eq!(state.item_offset(3), px(21.));
}

#[vespertine::test]
fn placeholders_paint_nothing_and_windowed_list_slides(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(20.)));
    state.set_logical_item_count(Some(100.));
    state.set_window_start(20);
    state.scroll_to(at(10, 0.));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    // The viewport shows rows 10..15, which are placeholders: nothing is rendered or painted.
    harness.draw(cx, 100., 100.);
    assert!(harness.painted().is_empty());
    assert!(harness.rendered().is_empty());
    assert_eq!(anchor(&state), (10, 0.));
    // Sliding the window onto the viewport renders the rows at their logical positions.
    state.slide_window(5, 20);
    harness.draw(cx, 100., 100.);
    assert_eq!(harness.painted(), vec![10, 11, 12, 13, 14]);
    approx(harness.bounds(10).unwrap().top().as_f32(), 0.);
    approx(harness.bounds(14).unwrap().top().as_f32(), 80.);
    assert_eq!(anchor(&state), (10, 0.));
}

// R4, R5: lazy measurement and hints.

#[vespertine::test]
fn r4_rows_in_overdraw_are_measured_once(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(100, ListAlignment::Top, px(100.));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    harness.draw(cx, 100., 100.);
    // Visible rows 0..5 plus overdraw rows up to 100px below the viewport.
    assert_eq!(harness.rendered(), (0..10).collect::<Vec<_>>());
    assert_eq!(harness.painted(), (0..5).collect::<Vec<_>>());
    let measured = state.0.borrow().items.summary().measured_count;
    assert_eq!(measured, 10);
    // The overdraw rows are cached: only the visible rows are rendered again.
    harness.draw(cx, 100., 100.);
    assert_eq!(harness.rendered(), (0..5).collect::<Vec<_>>());
}

#[vespertine::test]
fn r4_short_content_moves_scroll_top_up_and_measure_all(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    state.scroll_to(at(9, 0.));
    let harness = Harness::fixed(cx, &state, |_| 30.);
    harness.draw(cx, 100., 100.);
    // Ten rows of 30px fill the viewport only with rows 6..10 (120px, offset 20).
    assert_eq!(anchor(&state), (6, 20.));

    // Hints and estimates never trigger eager measurement.
    let hinted = ListState::new(50, ListAlignment::Top, px(0.)).with_uniform_item_height(px(30.));
    let hinted_harness = Harness::fixed(cx, &hinted, |_| 30.);
    hinted_harness.draw(cx, 100., 90.);
    assert_eq!(hinted_harness.rendered(), vec![0, 1, 2]);
    // Measure-all measures everything in the first layout.
    let all = ListState::new(50, ListAlignment::Top, px(0.)).measure_all();
    let all_harness = Harness::fixed(cx, &all, |_| 30.);
    all_harness.draw(cx, 100., 90.);
    assert_eq!(all_harness.rendered().len(), 50 + 3);
    assert_eq!(all.0.borrow().items.summary().measured_count, 50);
}

#[vespertine::test]
fn r5_a3_uniform_hint_survives_first_layout(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(100, ListAlignment::Top, px(0.)).with_uniform_item_height(px(40.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 160.);
    assert_eq!(state.item_offset(80), px(3200.));
    assert_eq!(state.max_offset_for_scrollbar().y, px(100. * 40. - 160.));

    // As rows are rendered their real heights replace the hints.
    let state = ListState::new(100, ListAlignment::Top, px(0.)).with_uniform_item_height(px(40.));
    let harness = Harness::fixed(cx, &state, |_| 48.);
    harness.draw(cx, 100., 160.);
    assert_eq!(state.item_offset(4), px(4. * 48.));
    assert_eq!(state.item_offset(80), px(4. * 48. + 76. * 40.));
}

// R6, R7: padding and content width.

#[vespertine::test]
fn r6_a2_horizontal_padding_drives_every_measurement_path(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    // Rendering: x origin and width follow the padding, and the wrapped (width dependent)
    // height is recomputed with the content width.
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let harness = Harness::new(cx, &state, |_| Row::Ratio(2.));
    harness.set_padding(cx, [20., 0., 40., 0.]);
    harness.draw(cx, 300., 1500.);
    let row = harness.bounds(0).unwrap();
    assert_eq!(
        (
            row.left().as_f32(),
            row.size.width.as_f32(),
            row.size.height.as_f32()
        ),
        (20., 240., 120.)
    );
    // Offscreen hints: only the padding changes; the rows not laid out keep nonzero hints.
    harness.set_padding(cx, [10., 0., 40., 0.]);
    harness.draw(cx, 300., 300.);
    let row = harness.bounds(0).unwrap();
    assert_eq!(
        (
            row.left().as_f32(),
            row.size.width.as_f32(),
            row.size.height.as_f32()
        ),
        (10., 250., 125.)
    );
    // Rows 0..2 were remeasured (125), rows 3.. keep their previous heights as hints (120).
    assert_eq!(state.item_offset(8), px(3. * 125. + 5. * 120.));
    assert!(state.item_offset(10) > px(0.));

    // Eager measurement uses the content width.
    let all = ListState::new(5, ListAlignment::Top, px(0.)).measure_all();
    let all_harness = Harness::new(cx, &all, |_| Row::Ratio(2.));
    all_harness.set_padding(cx, [20., 0., 40., 0.]);
    all_harness.draw(cx, 300., 100.);
    for item in all.0.borrow().items.iter() {
        let size = item.size().expect("measured");
        assert_eq!((size.width.as_f32(), size.height.as_f32()), (240., 120.));
    }

    // Lazy measurement of rows in the overdraw band uses the content width.
    let lazy = ListState::new(6, ListAlignment::Top, px(300.));
    let lazy_harness = Harness::new(cx, &lazy, |_| Row::Ratio(2.));
    lazy_harness.set_padding(cx, [20., 0., 40., 0.]);
    lazy_harness.draw(cx, 300., 100.);
    let measured_widths: Vec<f32> = lazy
        .0
        .borrow()
        .items
        .iter()
        .filter_map(|item| item.size())
        .map(|size| size.width.as_f32())
        .collect();
    assert!(measured_widths.len() >= 3, "{measured_widths:?}");
    assert!(measured_widths.iter().all(|width| *width == 240.));

    // Backward anchor correction (walking up from the end) uses the content width too.
    let back = ListState::new(6, ListAlignment::Top, px(0.));
    back.scroll_to_end();
    let back_harness = Harness::new(cx, &back, |_| Row::Ratio(2.));
    back_harness.set_padding(cx, [20., 0., 40., 0.]);
    back_harness.draw(cx, 300., 200.);
    let bottom = back_harness.bounds(5).unwrap();
    assert_eq!(
        (bottom.left().as_f32(), bottom.size.width.as_f32()),
        (20., 240.)
    );
    assert_eq!(bottom.size.height.as_f32(), 120.);
    // Content width never goes negative.
    back_harness.set_padding(cx, [200., 0., 200., 0.]);
    back_harness.draw(cx, 300., 200.);
    assert_eq!(back_harness.bounds(5).unwrap().size.width, px(0.));
}

#[vespertine::test]
fn r6_vertical_padding_extends_scroll_extent_and_offsets_rows(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    harness.set_padding(cx, [5., 10., 5., 30.]);
    harness.draw(cx, 100., 300.);
    assert_eq!(harness.bounds(0).unwrap().top(), px(10.));
    assert_eq!(harness.bounds(1).unwrap().top(), px(30.));
    let extent = state.logical_scroll_top_and_extent();
    assert_eq!(extent.content_height, px(200. + 10. + 30.));
}

#[vespertine::test]
fn r7_a4_width_change_keeps_old_heights_as_hints(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let harness = Harness::new(cx, &state, |_| Row::Ratio(2.));
    harness.draw(cx, 200., 1200.);
    assert_eq!(state.item_offset(10), px(1000.));

    harness.draw(cx, 100., 150.);
    // Rows 0..3 are laid out again at the new width; the others keep a hint of their old size.
    assert_eq!(state.item_offset(5), px(3. * 50. + 2. * 100.));
    {
        let inner = state.0.borrow();
        let row = inner.mounted_item(5).unwrap();
        assert!(row.size().is_none());
        assert_eq!(row.size_hint().unwrap().height, px(100.));
    }
    // Hints are replaced as the rows are laid out.
    state.scroll_to(at(3, 0.));
    harness.draw(cx, 100., 1000.);
    assert_eq!(state.item_offset(10), px(500.));
}

#[vespertine::test]
fn r7_splice_does_not_keep_hints_for_replaced_rows(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(6, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    harness.draw(cx, 100., 200.);
    state.remeasure_items(1..3);
    {
        // R10: only the revised rows are remeasured, and they keep a hint meanwhile.
        let inner = state.0.borrow();
        assert!(inner.mounted_item(0).unwrap().size().is_some());
        assert!(inner.mounted_item(1).unwrap().size().is_none());
        assert_eq!(
            inner.mounted_item(1).unwrap().size_hint().unwrap().height,
            px(20.)
        );
        assert!(inner.mounted_item(3).unwrap().size().is_some());
    }
    state.splice(1..3, 2);
    let inner = state.0.borrow();
    assert!(inner.mounted_item(1).unwrap().size_hint().is_none());
    assert!(inner.mounted_item(2).unwrap().size_hint().is_none());
    assert!(inner.mounted_item(0).unwrap().size().is_some());
    assert!(inner.mounted_item(3).unwrap().size().is_some());
}

// R8..R12: anchors, splices and prepends.

#[vespertine::test]
fn r8_scroll_to_end_normalizes_and_applies_after_splices(_cx: &mut TestAppContext) {
    let state = ListState::new(5, ListAlignment::Top, px(0.));
    state.scroll_to(at(50, 12.));
    assert_eq!(anchor(&state), (5, 0.));
    // Signed offsets are kept for layout to resolve.
    state.scroll_to(at(3, -12.));
    assert_eq!(anchor(&state), (3, -12.));
    // The anchor applies to the rows spliced in the same update.
    state.splice(5..5, 4);
    state.scroll_to(at(8, 0.));
    assert_eq!(anchor(&state), (8, 0.));
}

#[vespertine::test]
fn r9_negative_offset_resolves_during_layout(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |ix| 10. + ix as f32 * 10.);
    // Rows 3, 2, 1 are unmeasured predecessors of row 4 (heights 40, 30, 20).
    state.scroll_to(at(4, -45.));
    harness.draw(cx, 100., 100.);
    // 45 = 40 (row 3) + 5 into row 2: the viewport top is 25 pixels into row 2 (height 30).
    assert_eq!(anchor(&state), (2, 25.));
    approx(harness.bounds(4).unwrap().top().as_f32(), 5. + 40.);
    // Clamps at the first row.
    state.scroll_to(at(1, -500.));
    harness.draw(cx, 100., 100.);
    assert_eq!(anchor(&state), (0, 0.));
}

#[vespertine::test]
fn r12_a5_prepend_unknown_rows_keeps_anchor_below_viewport_top(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 300.);

    // Three predecessors of unknown height 24, 48 and 72 are inserted above the anchor row.
    harness.set_row(cx, |ix| match ix {
        0 => Row::Fixed(24.),
        1 => Row::Fixed(48.),
        2 => Row::Fixed(72.),
        _ => Row::Fixed(40.),
    });
    state.splice(0..0, 3);
    state.scroll_to(at(3, -36.));
    harness.draw(cx, 100., 150.);
    // The anchor sits 36px below the viewport top, which is 36px from the bottom of the 72px row.
    assert_eq!(anchor(&state), (2, 36.));
    assert_eq!(harness.bounds(3).unwrap().top(), px(36.));
    assert_eq!(harness.bounds(2).unwrap().top(), px(-36.));

    // Predecessors covering less than the offset clamp at the first row.
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 300.);
    harness.set_row(cx, |ix| match ix {
        0 => Row::Fixed(24.),
        1 => Row::Fixed(48.),
        _ => Row::Fixed(40.),
    });
    state.splice(0..0, 2);
    state.scroll_to(at(2, -100.));
    harness.draw(cx, 100., 300.);
    assert_eq!(anchor(&state), (0, 0.));
    assert_eq!(harness.bounds(2).unwrap().top(), px(72.));
}

#[vespertine::test]
fn r11_r12_a7_splices_keep_the_anchor_row(_cx: &mut TestAppContext) {
    let fresh = |count: usize, ix: usize, offset: f32| {
        let state = ListState::new(count, ListAlignment::Top, px(0.));
        state.scroll_to(at(ix, offset));
        state
    };

    // Before the anchor: the row index shifts by the net change.
    let state = fresh(10, 5, 7.);
    state.splice(0..2, 5);
    assert_eq!(anchor(&state), (8, 7.));
    let state = fresh(10, 5, 7.);
    state.splice(1..4, 0);
    assert_eq!(anchor(&state), (2, 7.));
    // After the anchor: untouched.
    let state = fresh(10, 5, 7.);
    state.splice(7..10, 1);
    assert_eq!(anchor(&state), (5, 7.));
    // Inside a replaced range: the first row of the range, offset zero.
    let state = fresh(10, 5, 7.);
    state.splice(4..7, 2);
    assert_eq!(anchor(&state), (4, 0.));
    // Removing the anchor row selects the row that now occupies its index.
    let state = fresh(10, 5, 7.);
    state.splice(5..6, 0);
    assert_eq!(anchor(&state), (5, 0.));
    // Removal at the end: the new last row.
    let state = fresh(10, 9, 3.);
    state.splice(8..10, 0);
    assert_eq!(anchor(&state), (7, 0.));
    let state = fresh(10, 9, 3.);
    state.splice(9..10, 0);
    assert_eq!(anchor(&state), (8, 0.));
    // An empty list has no anchor.
    let state = fresh(4, 2, 3.);
    state.splice(0..4, 0);
    assert!(state.0.borrow().logical_scroll_top.is_none());

    // Prepending at the very top leaves the top in view.
    let state = fresh(5, 0, 0.);
    state.splice(0..0, 3);
    assert_eq!(anchor(&state), (0, 0.));
    // Prepending while scrolled preserves the anchor by identity.
    let state = fresh(5, 2, 0.);
    state.splice(0..0, 3);
    assert_eq!(anchor(&state), (5, 0.));
    let state = fresh(5, 0, 5.);
    state.splice(0..0, 3);
    assert_eq!(anchor(&state), (3, 5.));
    // While following the tail the top-of-list exception does not apply.
    let state = fresh(5, 0, 0.);
    state.set_follow_mode(FollowMode::Tail);
    state.0.borrow_mut().logical_scroll_top = Some(at(0, 0.));
    state.splice(0..0, 3);
    assert_eq!(anchor(&state), (3, 0.));
}

#[vespertine::test]
fn r10_splice_outside_range_keeps_records_and_focus_handles(cx: &mut TestAppContext) {
    let handles: Vec<FocusHandle> = cx.update(|cx| (0..4).map(|_| cx.focus_handle()).collect());
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.splice_focusable(0..0, handles.iter().cloned().map(Some));
    set_measured(&state, &[Some(10.), Some(20.), Some(30.), Some(40.)]);
    {
        // Re-attach the focus handles to the replaced records.
        let mut inner = state.0.borrow_mut();
        let items: Vec<ListItem> = inner
            .items
            .iter()
            .zip(&handles)
            .map(|(item, handle)| ListItem::Measured {
                size: item.size().unwrap(),
                focus_handle: Some(handle.clone()),
            })
            .collect();
        inner.items = SumTree::from_iter(items, ());
    }
    state.splice(1..2, 3);
    let inner = state.0.borrow();
    assert_eq!(inner.items.summary().count, 6);
    assert_eq!(
        inner.mounted_item(0).unwrap().focus_handle().unwrap().id,
        handles[0].id
    );
    assert_eq!(
        inner.mounted_item(0).unwrap().size().unwrap().height,
        px(10.)
    );
    assert!(inner.mounted_item(1).unwrap().size().is_none());
    assert_eq!(
        inner.mounted_item(4).unwrap().focus_handle().unwrap().id,
        handles[2].id
    );
    assert_eq!(
        inner.mounted_item(4).unwrap().size().unwrap().height,
        px(30.)
    );
    assert_eq!(
        inner.mounted_item(5).unwrap().focus_handle().unwrap().id,
        handles[3].id
    );
}

// R13: keyed reconciliation.

fn keyed_state(keys: &[char]) -> ListState {
    let state = ListState::new(keys.len(), ListAlignment::Top, px(0.));
    let heights: Vec<Option<f32>> = (0..keys.len()).map(|ix| Some(10. + ix as f32)).collect();
    set_measured(&state, &heights);
    state
}

fn heights(state: &ListState) -> Vec<Option<f32>> {
    state
        .0
        .borrow()
        .items
        .iter()
        .map(|item| item.size().map(|size| size.height.as_f32()))
        .collect()
}

#[vespertine::test]
fn r13_a7_keyed_reconciliation_restores_the_anchor(_cx: &mut TestAppContext) {
    let old: Vec<char> = "abcdef".chars().collect();

    // Filtering with a surviving anchor key: the row moves, the offset is kept.
    let state = keyed_state(&old);
    state.scroll_to(at(2, 4.));
    let new: Vec<char> = "ace".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (1, 4.));
    // Measured heights follow the keys.
    assert_eq!(heights(&state), vec![Some(10.), Some(12.), Some(14.)]);

    // Filtering with the anchor key removed: the surviving row at that index, offset 0.
    let state = keyed_state(&old);
    state.scroll_to(at(2, 4.));
    let new: Vec<char> = "abef".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (2, 0.));

    // The fallback is clamped to the new last row; an empty result has no anchor.
    let state = keyed_state(&old);
    state.scroll_to(at(5, 4.));
    let new: Vec<char> = "ab".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (1, 0.));
    state.reconcile_keys(&new, &[]);
    assert!(state.0.borrow().logical_scroll_top.is_none());

    // Reorder: the identity of the anchor row survives the display order change.
    let state = keyed_state(&old);
    state.scroll_to(at(2, 4.));
    let new: Vec<char> = "fedcba".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (3, 4.));
    assert_eq!(
        heights(&state),
        vec![
            Some(15.),
            Some(14.),
            Some(13.),
            Some(12.),
            Some(11.),
            Some(10.)
        ]
    );

    // Insertions are unmeasured without hints; surviving rows keep their records.
    let state = keyed_state(&old);
    state.scroll_to(at(2, 4.));
    let new: Vec<char> = "xyabcdef".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (4, 4.));
    assert_eq!(heights(&state)[..3], [None, None, Some(10.)]);
    assert!(
        state
            .0
            .borrow()
            .mounted_item(0)
            .unwrap()
            .size_hint()
            .is_none()
    );

    // Anchored at the end (scroll to end) stays at the end.
    let state = keyed_state(&old);
    state.scroll_to_end();
    let new: Vec<char> = "abcdefgh".chars().collect();
    state.reconcile_keys(&old, &new);
    assert_eq!(anchor(&state), (8, 0.));

    // Identical sequences leave the state alone, including a pending scroll.
    let state = keyed_state(&old);
    state.scroll_to(at(2, 4.));
    state.remeasure_items(2..3);
    state.reconcile_keys(&old, &old);
    assert!(state.0.borrow().pending_scroll.is_some());
    assert_eq!(anchor(&state), (2, 4.));
}

#[vespertine::test]
fn r13_keyed_reconciliation_keeps_focus_handles(cx: &mut TestAppContext) {
    let handles: Vec<FocusHandle> = cx.update(|cx| (0..3).map(|_| cx.focus_handle()).collect());
    let old = ['a', 'b', 'c'];
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.splice_focusable(0..0, handles.iter().cloned().map(Some));
    state.reconcile_keys(&old, &['c', 'a', 'z']);
    let inner = state.0.borrow();
    assert_eq!(
        inner.mounted_item(0).unwrap().focus_handle().unwrap().id,
        handles[2].id
    );
    assert_eq!(
        inner.mounted_item(1).unwrap().focus_handle().unwrap().id,
        handles[0].id
    );
    assert!(inner.mounted_item(2).unwrap().focus_handle().is_none());
}

// R14, R15: height changes and configuration.

#[vespertine::test]
fn r14_a9_height_changes_update_extent_and_hitboxes_in_one_cycle(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 30.);
    harness.draw(cx, 100., 120.);
    assert_eq!(
        state.logical_scroll_top_and_extent().content_height,
        px(120.)
    );

    harness.set_row(cx, |ix| Row::Fixed(if ix == 2 { 90. } else { 30. }));
    state.remeasure_items(2..3);
    harness.draw(cx, 100., 400.);
    assert_eq!(
        state.logical_scroll_top_and_extent().content_height,
        px(9. * 30. + 90.)
    );
    assert_eq!(harness.bounds(2).unwrap().size.height, px(90.));
    assert_eq!(harness.bounds(3).unwrap().top(), px(150.));
}

#[vespertine::test]
fn r15_identical_configuration_keeps_native_state(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(20, ListAlignment::Top, px(50.));
    let harness = Harness::fixed(cx, &state, |_| 30.);
    state.scroll_to(at(3, 5.));
    harness.draw(cx, 100., 100.);
    state.set_follow_tail(false);
    assert!(!state.is_following_tail());
    state.set_alignment(ListAlignment::Top);
    state.set_overdraw(px(50.));
    state.set_estimated_item_height(None);
    state.set_logical_item_count(None);
    state.set_window_start(0);
    state.set_trailing_reservation(None);
    assert_eq!(anchor(&state), (3, 5.));
    assert_eq!(state.0.borrow().items.summary().measured_count, 8);
    // Reconfiguration preserves the logical anchor.
    state.set_estimated_item_height(Some(px(30.)));
    state.set_overdraw(px(0.));
    state.set_alignment(ListAlignment::Bottom);
    assert_eq!(anchor(&state), (3, 5.));
    // Only entering tail following moves it.
    state.set_follow_tail(true);
    assert_eq!(anchor(&state), (20, 0.));
    state.pause_following_tail();
    state.set_follow_tail(true);
    assert!(
        !state.is_following_tail(),
        "enabling again must not restart following"
    );
    state.set_follow_tail(false);
    assert!(!state.is_following_tail());
}

#[vespertine::test]
fn r15_alignment_change_keeps_implicit_anchor(_cx: &mut TestAppContext) {
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    state.set_alignment(ListAlignment::Bottom);
    assert_eq!(anchor(&state), (0, 0.));
    let state = ListState::new(10, ListAlignment::Bottom, px(0.));
    assert_eq!(anchor(&state), (10, 0.));
    state.set_alignment(ListAlignment::Top);
    assert_eq!(anchor(&state), (10, 0.));
}

// R16: windowing.

#[vespertine::test]
fn r16_visible_range_is_reported_with_inclusive_start_and_exclusive_end(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(100, ListAlignment::Top, px(0.));
    let ranges = Rc::new(RefCell::new(Vec::new()));
    {
        let ranges = ranges.clone();
        state.set_scroll_handler(move |event, _, _| {
            ranges
                .borrow_mut()
                .push((event.visible_range.clone(), event.count));
        });
    }
    let harness = Harness::fixed(cx, &state, |_| 20.);
    harness.draw(cx, 100., 100.);
    cx.run_until_parked();
    assert_eq!(*ranges.borrow(), vec![(0..5, 100)]);
    // Unchanged frames report nothing new.
    harness.draw(cx, 100., 100.);
    cx.run_until_parked();
    assert_eq!(ranges.borrow().len(), 1);
    // A programmatic scroll changes the range.
    state.scroll_to(at(10, 10.));
    harness.draw(cx, 100., 100.);
    cx.run_until_parked();
    assert_eq!(ranges.borrow().last().unwrap().0, 10..16);
}

#[vespertine::test]
fn r16_sliding_the_window_preserves_the_logical_anchor_and_hints(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(40, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(24.)));
    state.set_logical_item_count(Some(1000.));
    state.set_window_start(50);
    state.scroll_to(at(50, 3.));
    let harness = Harness::fixed(cx, &state, |_| 30.);
    harness.draw(cx, 100., 300.);
    // Rows 50..60 are measured (30px) and everything else uses the estimate.
    assert_eq!(state.item_offset(60), px(50. * 24. + 10. * 30.));
    // Row 60 is partly visible and measured as well.
    assert_eq!(state.item_offset(70), px(50. * 24. + 11. * 30. + 9. * 24.));

    state.slide_window(60, 40);
    assert_eq!(state.window_start(), 60);
    assert_eq!(anchor(&state), (50, 3.));
    // Rows that left the window retain their measured height as a hint.
    assert_eq!(state.item_offset(60), px(50. * 24. + 10. * 30.));
    // Rows still mounted keep their measurements (row 60 was laid out).
    assert_eq!(state.0.borrow().items.summary().measured_count, 1);

    // Sliding back picks the retained hints up again.
    state.slide_window(50, 40);
    assert_eq!(state.item_offset(60), px(50. * 24. + 10. * 30.));
    harness.draw(cx, 100., 300.);
    assert_eq!(anchor(&state), (50, 3.));
    assert_eq!(harness.painted(), (50..61).collect::<Vec<_>>());
}

#[vespertine::test]
fn r16_window_start_and_keyed_reconcile_keep_the_logical_anchor(_cx: &mut TestAppContext) {
    let state = ListState::new(4, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(10.)));
    state.set_logical_item_count(Some(100.));
    state.set_window_start(10);
    state.scroll_to(at(11, 2.));
    set_measured(&state, &[Some(5.), Some(6.), Some(7.), Some(8.)]);
    let old = [10, 11, 12, 13];
    state.reconcile_keys_in_window(12, &old, &[12, 13, 14, 15]);
    assert_eq!(anchor(&state), (11, 2.));
    assert_eq!(state.window_start(), 12);
    assert_eq!(heights(&state), vec![Some(7.), Some(8.), None, None]);
    // Rows 10 and 11 left the window with their measured heights.
    assert_eq!(state.item_offset(12), px(10. * 10. + 5. + 6.));
}

// R17: measured-average extents.

#[vespertine::test]
fn r17_a6_measured_average_scroll_extents(_cx: &mut TestAppContext) {
    let state = ListState::new(10, ListAlignment::Top, px(0.));
    // Before any measurement the query has no value, even with hints.
    assert_eq!(state.measured_average_scroll_extents(), None);
    state.reset_with_uniform_height(10, px(40.));
    assert_eq!(state.measured_average_scroll_extents(), None);

    let state = ListState::new(10, ListAlignment::Top, px(0.));
    let mut heights = vec![None; 10];
    heights[0] = Some(20.);
    heights[1] = Some(60.);
    set_measured(&state, &heights);
    state.0.borrow_mut().last_layout_bounds =
        Some(Bounds::new(point(px(0.), px(0.)), size(px(10.), px(100.))));
    // H = 80, m = 2, u = 8, a = 40: estimated total 400, maximum 300.
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(300.), px(0.)))
    );
    // The current distance counts unmeasured rows before the anchor at the average.
    state.scroll_to(at(5, 5.));
    // 20 + 60 + 3 * 40 + 5
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(300.), px(205.)))
    );
    // Capped at the maximum; a negative offset is treated as zero before layout.
    state.scroll_to(at(9, 50.));
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(300.), px(300.)))
    );
    state.scroll_to(at(5, -30.));
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(300.), px(200.)))
    );
    // Vertical padding extends the extent.
    state.0.borrow_mut().last_padding = Some(Edges {
        top: px(10.),
        right: px(0.),
        bottom: px(20.),
        left: px(0.),
    });
    state.scroll_to(at(0, 0.));
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(330.), px(10.)))
    );
    // Never forces measurement or moves the viewport.
    assert_eq!(state.0.borrow().items.summary().measured_count, 2);

    // A bottom aligned list without an explicit anchor reports current equal to maximum.
    let bottom = ListState::new(10, ListAlignment::Bottom, px(0.));
    set_measured(&bottom, &heights);
    bottom.0.borrow_mut().last_layout_bounds =
        Some(Bounds::new(point(px(0.), px(0.)), size(px(10.), px(100.))));
    assert_eq!(
        bottom.measured_average_scroll_extents(),
        Some((px(300.), px(300.)))
    );
}

#[vespertine::test]
fn r17_extents_count_placeholders_as_unmeasured(_cx: &mut TestAppContext) {
    let state = ListState::new(2, ListAlignment::Top, px(0.));
    state.set_estimated_item_height(Some(px(24.)));
    state.set_logical_item_count(Some(10.));
    state.set_window_start(4);
    set_measured(&state, &[Some(10.), Some(30.)]);
    state.0.borrow_mut().last_layout_bounds =
        Some(Bounds::new(point(px(0.), px(0.)), size(px(10.), px(50.))));
    // a = 20: total 40 + 8 * 20 = 200.
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(150.), px(0.)))
    );
    state.scroll_to(at(5, 0.));
    // Four placeholders and one measured row before row 5.
    assert_eq!(
        state.measured_average_scroll_extents(),
        Some((px(150.), px(4. * 20. + 10.)))
    );
}

// R18: item queries.

#[vespertine::test]
fn r18_item_queries_do_not_change_the_scroll_position(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(20, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    state.scroll_to(at(3, 10.));
    harness.draw(cx, 100., 100.);
    let before = anchor(&state);
    assert_eq!(state.item_count(), 20);
    assert_eq!(state.viewport_bounds().size, size(px(100.), px(100.)));
    assert_eq!(state.item_is_above_viewport(2), Some(true));
    assert_eq!(state.item_is_above_viewport(3), Some(false));
    assert_eq!(state.item_is_below_viewport(5), Some(false));
    assert_eq!(state.item_is_below_viewport(4), Some(false));
    // Row 6 has not been measured, so nothing is known about it.
    assert_eq!(state.item_is_below_viewport(6), None);
    let bounds = state.bounds_for_item(3).unwrap();
    assert_eq!((bounds.top(), bounds.size.height), (px(-10.), px(40.)));
    // Rows that are not measured have no bounds.
    assert!(state.bounds_for_item(19).is_none());
    let top = state.logical_scroll_top_and_extent();
    assert_eq!(
        (top.offset.item_ix, top.offset.offset_in_item),
        (3, px(10.))
    );
    assert_eq!(anchor(&state), before);
}

#[vespertine::test]
fn r18_reveal_item_moves_minimally(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(20, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 100.);
    // Row 2 is partly below the viewport: scroll just enough to show it completely.
    state.scroll_to_reveal_item(2);
    harness.draw(cx, 100., 100.);
    assert_eq!(state.bounds_for_item(2).unwrap().bottom(), px(100.));
    // Already visible rows do not move the list.
    let before = anchor(&state);
    state.scroll_to_reveal_item(2);
    state.scroll_to_reveal_item(1);
    assert_eq!(anchor(&state), before);
    // Rows above come to the top.
    state.scroll_to_reveal_item(0);
    assert_eq!(anchor(&state), (0, 0.));
}

// R19: tail following.

#[vespertine::test]
fn r19_a10_tail_following_cycle(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10, ListAlignment::Top, px(0.)).measure_all();
    let harness = Harness::fixed(cx, &state, |_| 40.);
    state.set_follow_tail(true);
    harness.draw(cx, 100., 200.);
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    // Appending while following keeps the end in view.
    state.splice(10..10, 5);
    harness.draw(cx, 100., 200.);
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    assert_eq!(harness.bounds(14).unwrap().bottom(), px(200.));
    // Scrolling up stops following.
    state.scroll_by(px(-100.));
    harness.draw(cx, 100., 200.);
    assert!(!state.is_following_tail());
    assert_eq!(state.is_scrolled_to_end(), Some(false));
    // Returning to the bottom resumes it.
    state.scroll_to_end();
    harness.draw(cx, 100., 200.);
    assert!(state.is_following_tail());
    state.splice(15..15, 2);
    harness.draw(cx, 100., 200.);
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    // Pausing freezes the list while keeping tail mode, even when something other than
    // the user grows the last row.
    state.pause_following_tail();
    let frozen = anchor(&state);
    harness.set_row(cx, |ix| Row::Fixed(if ix == 16 { 300. } else { 40. }));
    state.remeasure_items(16..17);
    harness.draw(cx, 100., 200.);
    assert_eq!(anchor(&state), frozen);
    assert!(!state.is_following_tail());
    assert_eq!(state.is_scrolled_to_end(), Some(false));
    // Leaving tail mode clears the state.
    state.set_follow_mode(FollowMode::Normal);
    assert!(!state.is_following_tail());
}

// R20: focus.

#[vespertine::test]
fn r20_a11_focused_rows_keep_rendering_and_new_focus_scrolls_into_view(cx: &mut TestAppContext) {
    let handles: Vec<FocusHandle> = cx.update(|cx| (0..20).map(|_| cx.focus_handle()).collect());
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.splice_focusable(0..0, handles.iter().cloned().map(Some));
    let (harness, cx) = Harness::in_window(cx, &state, |_| Row::Fixed(40.), (100., 120.));
    harness.track_focus(cx, handles.clone());
    harness.redraw(cx);

    cx.update(|window, cx| window.focus(&handles[1], cx));
    harness.redraw(cx);
    assert_eq!(anchor(&state), (0, 0.), "visible focus does not scroll");
    cx.simulate_keystrokes("a");
    assert_eq!(
        harness.key_presses(cx),
        1,
        "visible focused row receives keys"
    );

    // The focused row leaves the viewport but keeps rendering and receiving keys.
    state.scroll_to(at(15, 0.));
    harness.redraw(cx);
    assert!(harness.rendered().contains(&1), "{:?}", harness.rendered());
    assert_eq!(anchor(&state), (15, 0.));
    cx.simulate_keystrokes("a");
    assert_eq!(harness.key_presses(cx), 2);

    // A newly focused off-screen row is scrolled into view.
    state.scroll_to(at(0, 0.));
    harness.redraw(cx);
    cx.update(|window, cx| window.focus(&handles[18], cx));
    harness.redraw(cx);
    let bounds = harness.bounds(18).expect("focused row painted");
    assert!(
        bounds.top() >= px(0.) && bounds.bottom() <= px(120.),
        "{bounds:?}"
    );
    assert_eq!(state.item_is_above_viewport(18), Some(false));
    assert_eq!(state.item_is_below_viewport(18), Some(false));
}

#[vespertine::test]
fn r20_focus_handles_survive_splices(cx: &mut TestAppContext) {
    let handles: Vec<FocusHandle> = cx.update(|cx| (0..3).map(|_| cx.focus_handle()).collect());
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.splice_focusable(0..0, handles.iter().cloned().map(Some));
    state.splice(0..1, 2);
    state.splice(4..4, 1);
    let inner = state.0.borrow();
    assert!(inner.mounted_item(0).unwrap().focus_handle().is_none());
    assert_eq!(
        inner.mounted_item(2).unwrap().focus_handle().unwrap().id,
        handles[1].id
    );
    assert_eq!(
        inner.mounted_item(3).unwrap().focus_handle().unwrap().id,
        handles[2].id
    );
}

// R4 / A1: windowed list with 100000 rows.

#[vespertine::test]
fn a1_windowed_100000_rows_paint_only_viewport_plus_overdraw(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(40, ListAlignment::Top, px(512.));
    state.set_estimated_item_height(Some(px(24.)));
    state.set_logical_item_count(Some(100_000.));
    state.set_window_start(50);
    state.scroll_to(at(50, 0.));
    let harness = Harness::fixed(cx, &state, |_| 24.);
    // (viewport + overdraw) / shortest row, plus the anchor row.
    let bound = ((360. + 512.) / 24.0_f32).ceil() as usize + 1;

    harness.draw(cx, 560., 360.);
    assert_eq!(harness.painted(), (50..65).collect::<Vec<_>>());
    assert!(
        harness.rendered().len() <= bound,
        "{}",
        harness.rendered().len()
    );
    assert_eq!(state.item_offset(100_000), px(2_400_000.));

    // Scroll to the final row and slide the window as the visible range demands.
    state.scroll_to(at(99_999, 0.));
    state.slide_window(99_960, 40);
    harness.draw(cx, 560., 360.);
    assert_eq!(harness.painted(), (99_985..100_000).collect::<Vec<_>>());
    assert!(
        harness.rendered().len() <= bound,
        "{}",
        harness.rendered().len()
    );
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    assert_eq!(state.window_start(), 99_960);
}

// LAY: FID-ELM-001-2 geometry.

#[vespertine::test]
fn fid_elm_001_2_geometry(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.set_padding(cx, [20., 10., 40., 10.]);
    harness.draw(cx, 260., 180.);
    for (ix, top) in [(0, 10.), (1, 50.), (2, 90.)] {
        let bounds = harness.bounds(ix).unwrap();
        assert_eq!(
            (
                bounds.left().as_f32(),
                bounds.top().as_f32(),
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32()
            ),
            (20., top, 200., 40.)
        );
    }
}

// R10, R11, R14 / A8: a large list keeps its top row across splices and remeasures.

#[vespertine::test]
fn a8_ten_thousand_rows_hold_the_top_row_across_splices_and_remeasures(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(10_000, ListAlignment::Top, px(0.));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    state.scroll_to(at(5000, 17.));
    harness.draw(cx, 100., 200.);
    assert_eq!(anchor(&state), (5000, 17.));

    // A splice before the anchor shifts its index; the same row and offset stay at the top.
    state.splice(100..110, 20);
    assert_eq!(state.item_count(), 10_010);
    harness.draw(cx, 100., 200.);
    assert_eq!(anchor(&state), (5010, 17.));
    state.splice(9000..9005, 1);
    harness.draw(cx, 100., 200.);
    assert_eq!(anchor(&state), (5010, 17.));

    // A partial remeasure keeps the pixel offset into the anchor row.
    harness.set_row(cx, |ix| Row::Fixed(if ix == 5010 { 100. } else { 40. }));
    state.remeasure_items(5010..5011);
    harness.draw(cx, 100., 200.);
    assert_eq!(anchor(&state), (5010, 17.));
    assert_eq!(state.bounds_for_item(5010).unwrap().size.height, px(100.));

    // A full remeasure keeps the fractional position within the anchor row: 17 / 100.
    harness.set_row(cx, |ix| Row::Fixed(if ix == 5010 { 200. } else { 80. }));
    state.remeasure();
    harness.draw(cx, 100., 200.);
    let (item_ix, offset) = anchor(&state);
    assert_eq!(item_ix, 5010);
    approx(offset, 34.);
}

//! CMP-019 pure-logic acceptance tests: resize and reorder math (A1, A2), splice planner (A4),
//! scroll anchors (A5), gutter geometry (A8) and alignment/header rules (A9).

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};
use vespertine::{ListAlignment, ListOffset, ListState, px};
use vespertine_components::table::{
    Align, Column, GroupSpec, GutterGeometry, IndicatorEdge, ResizeDrag, ResizeSide, TableRow,
    TableState, Transition, capture_anchor, drop_index, fit_gutter, fit_width, flatten_groups,
    handle_positions, header_visible, indicator, morph, natural_width, natural_widths, plan_splice,
    reorder, resize_after_primary, resize_pair, resolve_align, resolve_anchor, snap_width,
    sync_rows,
};

fn ids(rows: &[TableRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

fn side(start: f32, min: f32, max: f32) -> ResizeSide {
    ResizeSide { start, min, max }
}

// ---- A1: resize math -----------------------------------------------------------------------

#[test]
fn a1_a_pair_keeps_its_total_and_clamps_the_delta_to_both_bounds() {
    let left = side(200.0, 100.0, 400.0);
    let right = side(120.0, 80.0, 300.0);
    // +50 would give 250/70: right is clamped to its minimum, so the delta is limited to 40.
    assert_eq!(resize_pair(left, right, 50.0), (240.0, 80.0));
    // -150 is limited to -100.
    assert_eq!(resize_pair(left, right, -150.0), (100.0, 220.0));
    assert_eq!(resize_pair(left, right, 10.0), (210.0, 110.0));
    assert_eq!(resize_pair(left, right, 0.0), (200.0, 120.0));
    // The total never changes: no drag can create overflow.
    for delta in [-500.0, -60.0, 0.0, 33.0, 500.0] {
        let (l, r) = resize_pair(left, right, delta);
        assert_eq!(l + r, 320.0);
        assert!((100.0..=400.0).contains(&l) && (80.0..=300.0).contains(&r));
    }
    // The right column's maximum limits a drag to the left.
    assert_eq!(
        resize_pair(left, side(120.0, 80.0, 150.0), -100.0),
        (170.0, 150.0)
    );
}

#[test]
fn a1_with_the_primary_column_on_the_left_only_the_right_column_changes() {
    let right = side(120.0, 80.0, 300.0);
    assert_eq!(resize_after_primary(right, 30.0), 90.0);
    assert_eq!(
        resize_after_primary(right, 100.0),
        80.0,
        "clamped to the minimum"
    );
    assert_eq!(resize_after_primary(right, -50.0), 170.0);
    assert_eq!(
        resize_after_primary(right, -500.0),
        300.0,
        "clamped to the maximum"
    );
    let drag = ResizeDrag::new(
        "msg",
        "author",
        400.0,
        side(300.0, 80.0, f32::INFINITY),
        right,
        true,
    );
    assert_eq!(drag.apply(430.0), vec![("author".to_owned(), 90.0)]);
    let pair = ResizeDrag::new("a", "b", 400.0, side(200.0, 100.0, 400.0), right, false);
    assert_eq!(
        pair.apply(450.0),
        vec![("a".to_owned(), 240.0), ("b".to_owned(), 80.0)]
    );
}

#[test]
fn handles_sit_on_dividers_between_adjacent_resizable_columns() {
    let widths = [100.0, 50.0, 70.0, 30.0];
    assert_eq!(
        handle_positions(&widths, &[true, true, true, true]),
        [(0, 100.0), (1, 150.0), (2, 220.0)]
    );
    assert_eq!(
        handle_positions(&widths, &[true, true, false, true]),
        [(0, 100.0)]
    );
    assert!(handle_positions(&[100.0], &[true]).is_empty());
}

#[test]
fn natural_widths_share_the_remainder_equally_and_honor_minimums() {
    let id = Column::new("id", "Id").width(60.0);
    let msg = Column::new("msg", "Message");
    let other = Column::new("other", "Other").min_width(150.0);
    let cols = [&id, &msg, &other];
    // 600 - 60 = 540 split equally: 270 each, both above their minimums.
    assert_eq!(
        natural_widths(&cols, &BTreeMap::new(), 600.0),
        [60.0, 270.0, 270.0]
    );
    // 260 - 60 = 200: the other column freezes at 150, the primary gets the rest (50 < 80 min).
    assert_eq!(
        natural_widths(&cols, &BTreeMap::new(), 260.0),
        [60.0, 80.0, 150.0]
    );
    // A stored width overrides the descriptor.
    let mut widths = BTreeMap::new();
    widths.insert("id".to_owned(), 100.0);
    assert_eq!(natural_widths(&cols, &widths, 600.0)[0], 100.0);
}

// ---- A2: reorder ---------------------------------------------------------------------------

fn order() -> Vec<String> {
    ["a", "b", "c", "d"]
        .iter()
        .map(|id| (*id).to_owned())
        .collect()
}

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn a2_reorder_moves_the_dragged_column_to_the_targets_place() {
    assert_eq!(
        reorder(&order(), "a", "c"),
        Some(strings(&["b", "c", "a", "d"]))
    );
    assert_eq!(
        reorder(&order(), "d", "b"),
        Some(strings(&["a", "d", "b", "c"]))
    );
    assert_eq!(
        reorder(&order(), "b", "a"),
        Some(strings(&["b", "a", "c", "d"]))
    );
    assert_eq!(
        reorder(&order(), "a", "b"),
        Some(strings(&["b", "a", "c", "d"]))
    );
}

#[test]
fn a2_dragging_onto_itself_or_an_unknown_id_changes_nothing() {
    assert_eq!(reorder(&order(), "b", "b"), None);
    assert_eq!(reorder(&order(), "b", "zzz"), None);
    assert_eq!(reorder(&order(), "zzz", "b"), None);
}

#[test]
fn a2_hidden_columns_keep_their_relative_place() {
    // `b` is hidden; dragging `a` over `c` places it after `c`, `b` keeps its slot before it.
    assert_eq!(
        reorder(&order(), "a", "c"),
        Some(strings(&["b", "c", "a", "d"]))
    );
    let order = strings(&["a", "hidden", "c"]);
    assert_eq!(
        reorder(&order, "c", "a"),
        Some(strings(&["c", "a", "hidden"]))
    );
}

#[test]
fn a2_the_drop_index_is_the_first_column_whose_midpoint_exceeds_the_mapped_x() {
    let widths = [100.0, 100.0, 100.0];
    assert_eq!(drop_index(0.0, 300.0, &widths), 0);
    assert_eq!(drop_index(49.9, 300.0, &widths), 0);
    assert_eq!(
        drop_index(50.0, 300.0, &widths),
        1,
        "the midpoint itself does not exceed"
    );
    assert_eq!(drop_index(149.9, 300.0, &widths), 1);
    assert_eq!(drop_index(250.0, 300.0, &widths), 2);
    assert_eq!(
        drop_index(999.0, 300.0, &widths),
        2,
        "beyond the right edge: the last column"
    );
    assert_eq!(drop_index(-20.0, 300.0, &widths), 0);
    // A compressed layout (rendered 200 for a preferred 400) keeps the mapping linear.
    assert_eq!(drop_index(60.0, 200.0, &[200.0, 200.0]), 1);
    assert_eq!(drop_index(40.0, 200.0, &[200.0, 200.0]), 0);
    assert_eq!(drop_index(10.0, 0.0, &widths), 0);
}

#[test]
fn a2_the_indicator_is_leading_when_moving_earlier_and_trailing_when_moving_later() {
    let visible = ["a", "b", "c"];
    assert_eq!(
        indicator(&visible, "b", 0),
        Some((0, IndicatorEdge::Leading))
    );
    assert_eq!(
        indicator(&visible, "b", 2),
        Some((2, IndicatorEdge::Trailing))
    );
    assert_eq!(indicator(&visible, "b", 1), None);
    assert_eq!(indicator(&visible, "zzz", 1), None);
}

// ---- A4: splice planner --------------------------------------------------------------------

#[test]
fn a4_appending_a_page_yields_only_the_appended_range_with_or_without_the_footer() {
    assert_eq!(
        plan_splice(&["a", "b", "c"], &["a", "b", "c", "d"]),
        Some((3..3, 1))
    );
    assert_eq!(
        plan_splice(&["a", "b", "c"], &["a", "b", "c", "d", "footer"]),
        Some((3..3, 2))
    );
    assert_eq!(
        plan_splice(&["a", "b", "c", "footer"], &["a", "b", "c", "d", "footer"]),
        Some((3..3, 1))
    );
}

#[test]
fn a4_identical_sequences_yield_nothing_and_a_middle_replacement_keeps_prefix_and_suffix() {
    assert_eq!(plan_splice(&["a", "b"], &["a", "b"]), None);
    assert_eq!(plan_splice::<&str>(&[], &[]), None);
    assert_eq!(
        plan_splice(&["a", "b", "c", "d"], &["a", "x", "y", "d"]),
        Some((1..3, 2))
    );
    assert_eq!(plan_splice(&["a", "b", "c"], &["a"]), Some((1..3, 0)));
    assert_eq!(plan_splice(&["a"], &[]), Some((0..1, 0)));
    // Equal ids with a changed content revision are not equal rows.
    let old = [TableRow::item("1"), TableRow::item("2")];
    let new = [TableRow::item("1"), TableRow::item("2").with_revision(1)];
    assert_eq!(plan_splice(&old, &new), Some((1..2, 1)));
}

#[test]
fn flattening_keeps_collapsed_group_headings_and_adds_the_footer_only_when_more_exists() {
    let groups = vec![
        GroupSpec::new("may", vec![TableRow::item("1"), TableRow::item("2")]),
        GroupSpec::new("apr", vec![TableRow::item("3")]),
    ];
    let none = HashSet::new();
    assert_eq!(
        ids(&flatten_groups(&groups, &none, true)),
        [
            "group:may",
            "item:1",
            "item:2",
            "group:apr",
            "item:3",
            "footer:more"
        ]
    );
    let collapsed: HashSet<String> = ["may".to_owned()].into();
    assert_eq!(
        ids(&flatten_groups(&groups, &collapsed, false)),
        ["group:may", "group:apr", "item:3"]
    );
    // An empty list shows no footer even when more data exists.
    assert!(flatten_groups(&[], &none, true).is_empty());
    // Ungrouped rows have no headings.
    let plain = vec![GroupSpec::ungrouped(vec![TableRow::item("1")])];
    assert_eq!(
        ids(&flatten_groups(&plain, &none, true)),
        ["item:1", "footer:more"]
    );
    let row = TableRow::group("may");
    assert_eq!(row.group_key(), Some("may"));
    assert_eq!(TableRow::item("7").item_id(), Some("7"));
}

// ---- A5: anchors ---------------------------------------------------------------------------

fn items(range: std::ops::Range<usize>) -> Vec<TableRow> {
    range.map(|n| TableRow::item(&n.to_string())).collect()
}

fn list_over(rows: &[TableRow], item: &str, offset: f32) -> ListState {
    let list = ListState::new(rows.len(), ListAlignment::Top, px(100.));
    let ix = rows.iter().position(|row| row.id == item).unwrap();
    list.scroll_to(ListOffset {
        item_ix: ix,
        offset_in_item: px(offset),
    });
    list
}

fn top(list: &ListState) -> (usize, f32) {
    let top = list.logical_scroll_top();
    (top.item_ix, f32::from(top.offset_in_item))
}

#[test]
fn a5_item_7_keeps_its_pixel_position_through_every_kind_of_change() {
    let mut rows = items(0..20);
    rows.push(TableRow::footer());
    let list = list_over(&rows, "item:7", 11.0);
    assert_eq!(top(&list), (7, 11.0));

    // Appending a page.
    let mut next = items(0..30);
    next.push(TableRow::footer());
    sync_rows(&list, &rows, &next);
    assert_eq!(top(&list), (7, 11.0));
    assert_eq!(list.item_count(), 31);
    rows = next;

    // Collapsing an earlier group: three rows before item 7 disappear.
    let mut next = items(3..30);
    next.insert(0, TableRow::group("may"));
    next.push(TableRow::footer());
    sync_rows(&list, &rows, &next);
    let ix = next.iter().position(|row| row.id == "item:7").unwrap();
    assert_eq!(top(&list), (ix, 11.0));
    rows = next;

    // Filtering so that row 7 remains.
    let mut next = vec![
        TableRow::item("2"),
        TableRow::item("7"),
        TableRow::item("9"),
    ];
    next.push(TableRow::footer());
    sync_rows(&list, &rows, &next);
    assert_eq!(top(&list), (1, 11.0));
    rows = next;

    // Changing the sort: the order changes, the anchor row keeps its offset.
    let next = vec![
        TableRow::item("9"),
        TableRow::item("7"),
        TableRow::item("2"),
        TableRow::footer(),
    ];
    sync_rows(&list, &rows, &next);
    assert_eq!(top(&list), (1, 11.0));
}

#[test]
fn a5_a_filtered_out_anchor_falls_to_the_nearest_surviving_later_row_at_offset_zero() {
    let rows = items(0..10);
    let list = list_over(&rows, "item:7", 11.0);
    let mut next = items(0..7);
    next.extend(items(9..10));
    sync_rows(&list, &rows, &next);
    assert_eq!(
        top(&list),
        (7, 0.0),
        "item:9 is the nearest surviving later row"
    );
    // With no later row left the nearest earlier one is used.
    let list = list_over(&rows, "item:7", 11.0);
    sync_rows(&list, &rows, &items(0..5));
    assert_eq!(top(&list), (4, 0.0));
    // With nothing surviving the list stays at its default position.
    let list = list_over(&rows, "item:7", 11.0);
    sync_rows(&list, &rows, &items(20..25));
    assert_eq!(top(&list), (0, 0.0));
    let anchor = capture_anchor(&list_over(&rows, "item:3", 4.0), &rows).unwrap();
    assert_eq!(
        (anchor.id.as_str(), f32::from(anchor.offset)),
        ("item:3", 4.0)
    );
    assert_eq!(resolve_anchor(&anchor, &rows, &items(20..22)), None);
}

#[test]
fn a5_an_empty_list_that_receives_rows_lands_at_the_top() {
    let list = ListState::new(0, ListAlignment::Top, px(100.));
    sync_rows(&list, &[], &items(0..12));
    assert_eq!(list.item_count(), 12);
    assert_eq!(top(&list), (0, 0.0));
    // And a list that becomes empty and fills again.
    let rows = items(0..12);
    let list = list_over(&rows, "item:7", 3.0);
    sync_rows(&list, &rows, &[]);
    assert_eq!(list.item_count(), 0);
    sync_rows(&list, &[], &items(0..5));
    assert_eq!(top(&list), (0, 0.0));
}

#[test]
fn a5_identical_rows_are_not_touched() {
    let rows = items(0..10);
    let list = list_over(&rows, "item:7", 11.0);
    assert_eq!(sync_rows(&list, &rows, &rows), None);
    assert_eq!(top(&list), (7, 11.0));
}

#[test]
fn a5_a_transition_resolves_the_anchor_again_and_a_newer_user_scroll_wins() {
    let rows = items(0..20);
    let list = list_over(&rows, "item:7", 11.0);
    let next = items(5..20);
    let anchor = sync_rows(&list, &rows, &next).unwrap();
    let transition = Transition::begin(&list, anchor);
    // The list drifted while the transition ran (for example re-measured rows): settling
    // restores the click-time anchor on the settled rows.
    list.scroll_to(ListOffset {
        item_ix: 1,
        offset_in_item: px(0.),
    });
    let drifted = Transition {
        restored: (1, px(0.)),
        ..transition.clone()
    };
    let settled = drifted.settle(&list, &next).unwrap();
    assert_eq!(settled.id, "item:7");
    assert_eq!(top(&list), (2, 11.0));
    // A user scroll after the anchor was restored wins over the click-time anchor.
    list.scroll_to(ListOffset {
        item_ix: 6,
        offset_in_item: px(2.),
    });
    let settled = transition.settle(&list, &next).unwrap();
    assert_eq!(settled.id, "item:11");
    assert_eq!(top(&list), (6, 2.0));
}

// ---- A8: gutter geometry -------------------------------------------------------------------

#[test]
fn a8_natural_widths_follow_the_lane_model() {
    assert_eq!(natural_width(4), 67.0);
    assert_eq!(natural_width(1), 31.0);
    assert_eq!(natural_width(2), 43.0);
    // At most 48 px: used as it is.
    let tiny = fit_gutter(2, 400.0, 0.0, None);
    assert_eq!(
        (tiny.width, tiny.lane_spacing, tiny.compact),
        (43.0, 12.0, false)
    );
    // A single lane always uses its natural geometry.
    assert_eq!(fit_gutter(1, 60.0, 0.0, None), GutterGeometry::natural(1));
}

#[test]
fn a8_the_budgets_pick_the_smallest_of_natural_content_and_share() {
    assert_eq!(fit_width(4, 300.0, 0.0), 67.0);
    assert_eq!(fit_width(4, 150.0, 0.0), 51.0);
    assert_eq!(snap_width(51.0, 4), 50.0);
    assert_eq!(snap_width(67.0, 4), 67.0, "natural widths are not snapped");
    // The content budget never drops below 48 px.
    assert_eq!(fit_width(4, 100.0, 40.0), 48.0);
    // Optional columns eat into the content budget.
    assert_eq!(fit_width(4, 300.0, 160.0), 60.0);
}

#[test]
fn a8_a_fitted_gutter_compresses_the_lane_spacing() {
    // 12 lanes (natural 163) in a 400 px table: the share budget 136 compresses the spacing to
    // (136 - 31) / 11 while the primary column keeps 264 px.
    let fitted = fit_gutter(12, 400.0, 0.0, None);
    assert!(!fitted.compact);
    assert_eq!(fitted.width, 136.0);
    assert!((fitted.lane_spacing - 105.0 / 11.0).abs() < 1e-4);
}

#[test]
fn a8_the_gutter_becomes_a_compact_rail_below_160_and_leaves_it_at_184_with_hysteresis() {
    // Container 300: primary 233, full lanes.
    let wide = fit_gutter(4, 300.0, 0.0, None);
    assert!(!wide.compact);
    assert_eq!((wide.width, wide.lane_spacing), (67.0, 12.0));
    // Container 217: primary 217 - 67 = 150 < 160: compact (all lanes at the same x).
    let compact = fit_gutter(4, 217.0, 0.0, Some(&wide));
    assert!(compact.compact);
    assert_eq!((compact.width, compact.lane_spacing), (31.0, 0.0));
    assert_eq!(compact.lane_x(0, 1.0), compact.lane_x(3, 1.0));
    // Container 240: primary 173 is below 184: it stays compact (hysteresis).
    assert!(fit_gutter(4, 240.0, 0.0, Some(&compact)).compact);
    // Container 251: primary 184 and spacing 12: back to full lanes.
    let back = fit_gutter(4, 251.0, 0.0, Some(&compact));
    assert!(!back.compact);
    assert_eq!(back.width, 67.0);
    // Entering needs a primary column below 160 (container 226 gives 159).
    assert!(fit_gutter(4, 226.0, 0.0, None).compact);
    assert!(!fit_gutter(4, 227.0, 0.0, None).compact);
    // A container of 150 fits a width of 50 but the primary column (100) is far too small.
    assert!(fit_gutter(4, 150.0, 0.0, None).compact);
}

#[test]
fn a8_a_tiny_lane_spacing_also_forces_the_compact_rail_and_blocks_leaving_it() {
    // 20 lanes in a 300 px table: width 102, spacing (102 - 31) / 19 = 3.7 < 4 although the
    // primary column keeps 198 px.
    let packed = fit_gutter(20, 300.0, 0.0, None);
    assert!(packed.compact);
    // 12 lanes in the same table fit comfortably: spacing 6.45.
    let roomy = fit_gutter(12, 300.0, 0.0, None);
    assert!(!roomy.compact);
    assert_eq!(roomy.width, 102.0);
    // Leaving needs spacing of at least 5 as well as a 184 px primary column.
    assert!(fit_gutter(20, 300.0, 0.0, Some(&packed)).compact);
    let compact12 = GutterGeometry::compact_rail(12);
    assert!(!fit_gutter(12, 300.0, 0.0, Some(&compact12)).compact);
}

#[test]
fn a8_non_natural_widths_snap_down_and_small_changes_keep_the_previous_geometry() {
    // 12 lanes in a 405 px table: share budget 137.7 snaps down to 136.
    assert_eq!(fit_gutter(12, 405.0, 0.0, None).width, 136.0);
    // A previous geometry within 2 px is kept (no oscillation while dragging a divider).
    let previous = GutterGeometry {
        lanes: 12,
        width: 137.0,
        lane_spacing: 106.0 / 11.0,
        compact: false,
    };
    assert_eq!(fit_gutter(12, 405.0, 0.0, Some(&previous)), previous);
    // A difference of 2 px or more takes the new geometry.
    let far = GutterGeometry {
        width: 133.0,
        ..previous
    };
    assert_eq!(fit_gutter(12, 405.0, 0.0, Some(&far)).width, 136.0);
    // A different lane count never keeps it.
    let other = GutterGeometry {
        lanes: 11,
        ..previous
    };
    assert_eq!(fit_gutter(12, 405.0, 0.0, Some(&other)).width, 136.0);
}

#[test]
fn a8_lane_positions_start_16_px_from_the_left_and_snap_to_the_device_grid() {
    let geometry = GutterGeometry::natural(4);
    assert_eq!(geometry.lane_x(0, 1.0), 16.0);
    assert_eq!(geometry.lane_x(1, 1.0), 28.0);
    assert_eq!(geometry.lane_x(3, 2.0), 52.0);
    let fitted = GutterGeometry {
        lanes: 4,
        width: 50.0,
        lane_spacing: 19.0 / 3.0,
        compact: false,
    };
    let x = fitted.lane_x(1, 2.0);
    assert_eq!(x * 2.0, (x * 2.0).round(), "on the device pixel grid");
}

#[test]
fn a8_the_morph_interpolates_and_switches_topology_at_the_ends() {
    let full = GutterGeometry::natural(4);
    let compact = GutterGeometry::compact_rail(4);
    let middle = morph(&full, &compact, 0.5);
    assert_eq!(middle.width, (67.0 + 31.0) / 2.0);
    assert_eq!(middle.lane_spacing, 6.0);
    // Entering: the compact rail takes over only at progress 0.999.
    assert!(!morph(&full, &compact, 0.998).compact_topology);
    assert!(morph(&full, &compact, 0.999).compact_topology);
    assert!(morph(&full, &compact, 1.0).compact_topology);
    // Leaving: the full topology returns at the first frame.
    assert!(!morph(&compact, &full, 0.0).compact_topology);
    assert!(!morph(&compact, &full, 0.001).compact_topology);
    assert!(!morph(&compact, &full, 0.5).compact_topology);
    // No change: compact stays compact.
    assert!(morph(&compact, &compact, 0.0).compact_topology);
    assert_eq!(
        morph(&full, &compact, 7.0).width,
        31.0,
        "progress is clamped"
    );
}

#[test]
fn a8_focus_opacities() {
    use vespertine_components::table::gutter::{
        FOCUSED_STROKE, HIT_RADIUS, UNFOCUSED_PATH_OPACITY, UNFOCUSED_ROW_OPACITY, path_opacity,
        row_opacity,
    };
    assert_eq!((FOCUSED_STROKE, HIT_RADIUS), (2.25, 5.5));
    assert_eq!(path_opacity(None, 3), 1.0);
    assert_eq!(path_opacity(Some(1), 1), 1.0);
    assert_eq!(path_opacity(Some(1), 2), UNFOCUSED_PATH_OPACITY);
    assert_eq!(UNFOCUSED_PATH_OPACITY, 0.24);
    assert_eq!(row_opacity(false, false), 1.0);
    assert_eq!(row_opacity(true, true), 1.0);
    assert_eq!(row_opacity(true, false), UNFOCUSED_ROW_OPACITY);
    assert_eq!(UNFOCUSED_ROW_OPACITY, 0.6);
}

// ---- A9: alignment and header --------------------------------------------------------------

#[test]
fn a9_numeric_cells_default_to_end_alignment_unless_the_descriptor_says_otherwise() {
    assert_eq!(resolve_align(None, true), Align::End);
    assert_eq!(resolve_align(None, false), Align::Start);
    assert_eq!(resolve_align(Some(Align::Start), true), Align::Start);
    assert_eq!(resolve_align(Some(Align::Center), true), Align::Center);
    assert_eq!(resolve_align(Some(Align::End), false), Align::End);
}

#[test]
fn a9_the_header_is_omitted_when_every_label_is_empty() {
    let labelled = [Column::new("a", ""), Column::new("b", "Name")];
    assert!(header_visible(&labelled));
    let blank = [Column::new("a", ""), Column::new("b", "")];
    assert!(!header_visible(&blank));
    let hidden_label = [
        Column::new("a", ""),
        Column::new("b", "Name").visible(false),
    ];
    assert!(!header_visible(&hidden_label));
    assert!(!header_visible(&[]));
}

#[test]
fn column_builder_defaults_and_effective_bounds() {
    let column = Column::new("id", "Id");
    assert!(column.resizable && column.reorderable && column.visible);
    assert_eq!(column.effective_min(true), 80.0);
    assert_eq!(column.effective_min(false), 0.0);
    assert_eq!(column.effective_max(true), f32::INFINITY);
    let bounded = Column::new("id", "Id")
        .width(50.0)
        .min_width(30.0)
        .max_width(90.0)
        .align(Align::End);
    assert_eq!(bounded.effective_min(false), 30.0);
    assert_eq!(bounded.effective_max(false), 90.0);
    assert_eq!(bounded.align, Some(Align::End));
    let _ = TableState::new(vec![], None);
}

#[test]
fn a8_the_table_morphs_between_geometries_over_the_morph_duration() {
    use vespertine_components::table::gutter::hit_node;
    use vespertine_components::table::state::GUTTER_MORPH_DURATION;
    let state = TableState::new(vec![], None);
    let t0 = Instant::now();
    let full = GutterGeometry::natural(4);
    let compact = GutterGeometry::compact_rail(4);
    // The first geometry is shown as it is.
    assert_eq!(state.step_gutter(full, t0), (full, false));
    // A new target starts a morph from what was shown.
    let (shown, animating) = state.step_gutter(compact, t0);
    assert!(animating);
    assert_eq!(shown.width, 67.0);
    let (half, animating) = state.step_gutter(compact, t0 + GUTTER_MORPH_DURATION / 2);
    assert!(animating);
    assert_eq!((half.width, half.lane_spacing), (49.0, 6.0));
    assert!(
        !half.compact,
        "the rail takes over only at the end when entering"
    );
    let (done, animating) = state.step_gutter(
        compact,
        t0 + GUTTER_MORPH_DURATION + Duration::from_millis(1),
    );
    assert!(!animating);
    assert_eq!(done, compact);
    // Leaving: the full topology is drawn from the first frame.
    let (shown, _) = state.step_gutter(full, t0 + Duration::from_secs(1));
    assert!(!shown.compact);
    // Node hits use the 5.5 px radius.
    assert!(hit_node(10.0, 10.0, 15.0, 12.0));
    assert!(!hit_node(10.0, 10.0, 16.0, 10.0));
}

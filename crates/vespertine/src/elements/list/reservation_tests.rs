//! LAY-002: layout-time trailing viewport reservation.

use super::test_support::*;
use super::*;
use crate as vespertine;
use crate::TestAppContext;

const FID_HEIGHTS: [f32; 5] = [40., 40., 60., 50., 30.];

fn fid_row(ix: usize) -> f32 {
    FID_HEIGHTS[ix]
}

fn extra(state: &ListState) -> f32 {
    state.0.borrow().reservation_extra.as_f32()
}

fn total(state: &ListState) -> f32 {
    state.item_offset(usize::MAX).as_f32()
}

/// A five row list with the FID-LAY-002 heights, reservation applied by one frame at 300px.
fn fid_list(
    cx: &mut crate::VisualTestContext,
    reservation: Option<(usize, f32)>,
) -> (ListState, Harness) {
    let state = ListState::new(5, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(reservation.map(|(start, inset)| (start, px(inset))));
    let harness = Harness::fixed(cx, &state, fid_row);
    harness.draw(cx, 380., 300.);
    (state, harness)
}

#[vespertine::test]
fn r1_fid_reservation_grows_only_the_last_layout_box(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    // Natural suffix 140, minimum 280: the last row's layout box gets 140 more pixels ...
    assert_eq!(extra(&state), 140.);
    assert_eq!(state.bounds_for_item(4).unwrap().size.height, px(170.));
    // ... while its element keeps its natural painted size.
    assert_eq!(harness.bounds(4).unwrap().size.height, px(30.));
    assert_eq!(harness.bounds(4).unwrap().top(), px(190.));
    assert_eq!(total(&state), 360.);
    assert_eq!(state.max_offset_for_scrollbar().y, px(60.));
    for (ix, top) in [(0, 0.), (1, 40.), (2, 80.), (3, 140.)] {
        assert_eq!(harness.bounds(ix).unwrap().top().as_f32(), top);
    }
}

#[vespertine::test]
fn r2_extra_is_consumed_or_restored_not_accumulated(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    // Relaying out does not accumulate: the final row is remeasured naturally each time.
    for _ in 0..3 {
        harness.draw(cx, 380., 300.);
        assert_eq!(extra(&state), 140.);
        assert_eq!(total(&state), 360.);
    }
    // A taller final row consumes the extra.
    harness.set_row(cx, |ix| {
        Row::Fixed(if ix == 4 { 130. } else { fid_row(ix) })
    });
    state.remeasure_items(4..5);
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 40.);
    assert_eq!(total(&state), 40. + 40. + 280.);
    // Shrinking it restores the extra.
    harness.set_row(cx, |ix| Row::Fixed(if ix == 4 { 10. } else { fid_row(ix) }));
    state.remeasure_items(4..5);
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 160.);
    assert_eq!(total(&state), 40. + 40. + 280.);
}

#[vespertine::test]
fn a2_suffix_smaller_equal_and_larger_than_the_minimum(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    // Smaller: FID scene.
    let (state, _) = fid_list(cx, Some((2, 20.)));
    assert_eq!((extra(&state), total(&state)), (140., 360.));
    // Equal: suffix 60 + 50 + 30 + 140 = 280.
    let state = ListState::new(5, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((2, px(20.))));
    let harness = Harness::fixed(cx, &state, |ix| [40., 40., 60., 50., 170.][ix]);
    harness.draw(cx, 380., 300.);
    assert_eq!((extra(&state), total(&state)), (0., 360.));
    // Larger.
    let state = ListState::new(5, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((2, px(20.))));
    let harness = Harness::fixed(cx, &state, |ix| [40., 40., 60., 50., 300.][ix]);
    harness.draw(cx, 380., 300.);
    assert_eq!((extra(&state), total(&state)), (0., 490.));
    assert_eq!(state.bounds_for_item(4).unwrap().size.height, px(300.));
}

#[vespertine::test]
fn a2_disabling_and_changing_the_reservation(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    assert_eq!(total(&state), 360.);

    // Changing it removes the old extra at once and applies the new one at the next layout.
    state.set_trailing_reservation(Some((2, px(120.))));
    assert_eq!(extra(&state), 0.);
    assert_eq!(total(&state), 220.);
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 40.);
    assert_eq!(total(&state), 260.);

    // A start inside the list counts only the suffix.
    state.set_trailing_reservation(Some((4, px(0.))));
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 270.);
    assert_eq!(total(&state), 40. + 40. + 60. + 50. + 300.);

    // Disabling removes the extra.
    state.set_trailing_reservation(None);
    assert_eq!(extra(&state), 0.);
    harness.draw(cx, 380., 300.);
    assert_eq!((extra(&state), total(&state)), (0., 220.));
    assert_eq!(state.bounds_for_item(4).unwrap().size.height, px(30.));
}

#[vespertine::test]
fn a2_empty_lists_and_starts_at_or_beyond_the_row_count(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((0, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    harness.draw(cx, 100., 100.);
    assert_eq!((extra(&state), total(&state)), (0., 0.));
    state.splice(0..0, 2);
    harness.draw(cx, 100., 100.);
    assert_eq!((extra(&state), total(&state)), (60., 100.));

    // A start at or beyond the row count reserves nothing.
    for start in [2, 3, 100] {
        state.set_trailing_reservation(Some((start, px(0.))));
        harness.draw(cx, 100., 100.);
        assert_eq!((extra(&state), total(&state)), (0., 40.), "start {start}");
    }
}

#[vespertine::test]
fn a2_negative_and_over_viewport_insets(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    // A negative inset reserves more than a viewport: minimum 300 + 50 = 350.
    let (state, _) = fid_list(cx, Some((2, -50.)));
    assert_eq!(extra(&state), 350. - 140.);
    // An inset greater than the viewport reserves nothing.
    let (state, _) = fid_list(cx, Some((2, 400.)));
    assert_eq!(extra(&state), 0.);
    // An inset equal to the viewport as well.
    let (state, _) = fid_list(cx, Some((2, 300.)));
    assert_eq!(extra(&state), 0.);
}

#[vespertine::test]
fn a2_resizing_the_viewport(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    assert_eq!(extra(&state), 140.);
    harness.draw(cx, 380., 200.);
    // Minimum 180.
    assert_eq!(extra(&state), 40.);
    harness.draw(cx, 380., 600.);
    assert_eq!(extra(&state), 580. - 140.);
    harness.draw(cx, 380., 250.);
    assert_eq!(extra(&state), 230. - 140.);
}

#[vespertine::test]
fn a2_splices_remove_the_previous_extra_first(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    assert_eq!(total(&state), 360.);

    // Appending a row: the last row's size hint loses the extra and is no longer measured.
    state.splice(5..5, 1);
    assert_eq!(extra(&state), 0.);
    {
        let inner = state.0.borrow();
        let old_last = inner.mounted_item(4).unwrap();
        assert!(old_last.size().is_none());
        assert_eq!(old_last.size_hint().unwrap().height, px(30.));
    }
    assert_eq!(total(&state), 40. + 40. + 60. + 50. + 30.);
    harness.set_row(cx, |ix| Row::Fixed(*FID_HEIGHTS.get(ix).unwrap_or(&20.)));
    harness.draw(cx, 380., 300.);
    // Natural suffix 160, minimum 280.
    assert_eq!(extra(&state), 120.);
    assert_eq!(total(&state), 40. + 40. + 60. + 50. + 30. + 20. + 120.);

    // Splicing the last row away strips the extra from the old last row as well (clamped at 0).
    state.splice(5..6, 0);
    assert_eq!(extra(&state), 0.);
    assert_eq!(total(&state), 220.);
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 140.);
}

#[vespertine::test]
fn a2_focus_handle_of_the_last_row_survives_a_strip(cx: &mut TestAppContext) {
    let handle = cx.update(|cx| cx.focus_handle());
    let state = ListState::new(0, ListAlignment::Top, px(0.));
    state.splice_focusable(0..0, [None, Some(handle.clone())]);
    state.set_trailing_reservation(Some((0, px(0.))));
    state.0.borrow_mut().reservation_extra = px(50.);
    set_measured_with_focus(&state, handle.clone());
    state.set_trailing_reservation(None);
    let inner = state.0.borrow();
    let last = inner.mounted_item(1).unwrap();
    assert_eq!(last.size_hint().unwrap().height, px(30.));
    assert_eq!(last.focus_handle().unwrap().id, handle.id);
}

fn set_measured_with_focus(state: &ListState, handle: FocusHandle) {
    state.0.borrow_mut().items = SumTree::from_iter(
        [
            ListItem::Measured {
                size: size(px(10.), px(20.)),
                focus_handle: None,
            },
            ListItem::Measured {
                size: size(px(10.), px(80.)),
                focus_handle: Some(handle),
            },
        ],
        (),
    );
}

#[vespertine::test]
fn a3_jumping_across_unmeasured_rows_fabricates_no_gap(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(1000, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((990, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    state.scroll_to(at(500, 0.));
    harness.draw(cx, 100., 100.);
    // The pass never reaches the last row: nothing is reserved and nothing else is measured.
    assert_eq!(extra(&state), 0.);
    assert_eq!(state.0.borrow().items.summary().measured_count, 5);
    assert_eq!(harness.rendered().len(), 5);

    // Jumping to the end: rows 990..995 have neither a measurement nor a hint, so the
    // reservation is deferred instead of manufacturing a gap.
    state.scroll_to_end();
    harness.draw(cx, 100., 100.);
    assert_eq!(extra(&state), 0.);
    assert_eq!(state.0.borrow().items.summary().measured_count, 10);
    let mut rendered = harness.rendered();
    rendered.sort_unstable();
    assert_eq!(rendered, vec![995, 996, 997, 998, 999]);
    // Rows 995..1000 are the last 100px of the content.
    assert_eq!(state.bounds_for_item(999).unwrap().bottom(), px(100.));

    // With hints for the unmeasured rows the reservation is applied (hints are estimates).
    let state = ListState::new(1000, ListAlignment::Top, px(0.)).with_uniform_item_height(px(20.));
    state.set_trailing_reservation(Some((990, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 20.);
    state.scroll_to_end();
    harness.draw(cx, 100., 100.);
    // Natural suffix 10 rows * 20 = 200 >= 100.
    assert_eq!(extra(&state), 0.);
    state.set_trailing_reservation(Some((990, px(-300.))));
    harness.draw(cx, 100., 100.);
    assert_eq!(extra(&state), 400. - 200.);
    assert_eq!(state.0.borrow().items.summary().measured_count, 5);
}

#[vespertine::test]
fn r5_walking_up_to_fill_a_short_viewport_recalculates_the_reservation(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(6, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((0, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 200.);
    assert_eq!((extra(&state), total(&state)), (0., 200.));
    // Anchored at the last row with a viewport that needs the earlier rows: the reservation
    // is recalculated after the walk as natural rows become available.
    state.scroll_to(at(5, 0.));
    harness.draw(cx, 100., 300.);
    assert_eq!(extra(&state), 60.);
    assert_eq!(total(&state), 300.);
    assert_eq!(anchor(&state), (0, 0.));
    // The reservation never forces off-screen rows to be rendered.
    let state = ListState::new(100, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((0, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    harness.draw(cx, 100., 200.);
    assert_eq!(harness.rendered(), vec![0, 1, 2, 3, 4]);
}

#[vespertine::test]
fn a4_equal_reservation_is_a_noop_and_a_different_one_resets_the_extra(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    let measured = state.0.borrow().items.summary().measured_count;
    state.set_trailing_reservation(Some((2, px(20.))));
    assert_eq!(extra(&state), 140.);
    assert_eq!(state.0.borrow().items.summary().measured_count, measured);
    assert!(state.0.borrow().mounted_item(4).unwrap().size().is_some());

    state.set_trailing_reservation(Some((2, px(21.))));
    assert_eq!(extra(&state), 0.);
    assert!(state.0.borrow().mounted_item(4).unwrap().size().is_none());
    harness.draw(cx, 380., 300.);
    assert_eq!(extra(&state), 139.);

    // Both absent is a no-op too.
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(None);
    assert!(state.0.borrow().reservation.is_none());
}

#[vespertine::test]
fn a5_offset_and_filled_queries(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(3, ListAlignment::Top, px(0.));
    {
        let mut inner = state.0.borrow_mut();
        inner.items = SumTree::from_iter(
            [
                ListItem::Measured {
                    size: size(px(10.), px(10.)),
                    focus_handle: None,
                },
                ListItem::Unmeasured {
                    size_hint: Some(size(px(0.), px(40.))),
                    estimate: px(0.),
                    focus_handle: None,
                },
                ListItem::Unmeasured {
                    size_hint: None,
                    estimate: px(0.),
                    focus_handle: None,
                },
            ],
            (),
        );
    }
    state.scroll_to(at(1, 5.));
    // Measured heights first, then hints; unknown rows contribute zero; beyond the count is
    // the total. The viewport does not move.
    let offsets: Vec<f32> = (0..=5).map(|ix| state.item_offset(ix).as_f32()).collect();
    assert_eq!(offsets, vec![0., 10., 50., 50., 50., 50.]);
    assert_eq!(anchor(&state), (1, 5.));

    // Without a reservation the filled query is false.
    assert!(!state.trailing_reservation_filled());
    // Before any layout the viewport height is zero, so any natural height fills it.
    state.set_trailing_reservation(Some((1, px(0.))));
    assert!(state.trailing_reservation_filled());
    // Natural suffix 40 (hint + unknown) against viewport 100.
    state.0.borrow_mut().last_layout_bounds =
        Some(Bounds::new(point(px(0.), px(0.)), size(px(10.), px(100.))));
    assert!(!state.trailing_reservation_filled());
    state.set_trailing_reservation(Some((1, px(60.))));
    assert!(state.trailing_reservation_filled());
    state.set_trailing_reservation(Some((1, px(59.))));
    assert!(!state.trailing_reservation_filled());
    assert_eq!(anchor(&state), (1, 5.));

    // After a layout the applied extra is excluded from the natural height.
    let (state, _) = fid_list(cx, Some((2, 20.)));
    assert_eq!(state.item_offset(5).as_f32(), 360.);
    assert_eq!(state.item_offset(4).as_f32(), 190.);
    assert!(!state.trailing_reservation_filled());
    state.set_trailing_reservation(Some((2, px(160.))));
    assert!(state.trailing_reservation_filled());
}

#[vespertine::test]
fn r8_end_of_list_quantities_include_the_extra(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let (state, harness) = fid_list(cx, Some((2, 20.)));
    // Maximum scroll 360 - 300.
    assert_eq!(state.max_offset_for_scrollbar().y, px(60.));
    state.scroll_to_end();
    harness.draw(cx, 380., 300.);
    assert_eq!(anchor(&state), (1, 20.));
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    // The last row's bounds are its layout box.
    assert_eq!(state.bounds_for_item(4).unwrap().size.height, px(170.));
    // Tail following sticks to the reserved end.
    let state = ListState::new(5, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((2, px(20.))));
    state.set_follow_tail(true);
    let harness = Harness::fixed(cx, &state, fid_row);
    harness.draw(cx, 380., 300.);
    assert!(state.is_following_tail());
    assert_eq!(
        -state.scroll_px_offset_for_scrollbar().y,
        state.max_offset_for_scrollbar().y
    );
}

/// A1: appending consumes the reservation; the scroll extent stays at the reserved height until
/// natural content exceeds it.
#[vespertine::test]
fn a1_appending_rows_consumes_the_reservation_without_a_transient_gap(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(3, ListAlignment::Top, px(500.));
    state.set_trailing_reservation(Some((0, px(0.))));
    let harness = Harness::fixed(cx, &state, |_| 40.);
    let mut extents = Vec::new();
    for count in 3..=10 {
        if count > 3 {
            state.splice(count - 1..count - 1, 1);
        }
        harness.draw(cx, 100., 300.);
        extents.push((
            state
                .logical_scroll_top_and_extent()
                .content_height
                .as_f32(),
            state.max_offset_for_scrollbar().y.as_f32(),
        ));
    }
    assert_eq!(
        extents,
        vec![
            (300., 0.),
            (300., 0.),
            (300., 0.),
            (300., 0.),
            (300., 0.),
            (320., 20.),
            (360., 60.),
            (400., 100.),
        ]
    );
}

/// A window whose root is one identified list of 20 px rows filling a 100 px viewport, drawn
/// with row replay enabled.
struct RetainedRows {
    state: ListState,
}

impl crate::Render for RetainedRows {
    fn render(
        &mut self,
        _: &mut crate::Window,
        _: &mut crate::Context<Self>,
    ) -> impl crate::IntoElement {
        list(self.state.clone(), |_, _, _| {
            crate::div().h(px(20.)).w_full().into_any_element()
        })
        .id("rows")
        .w_full()
        .h(px(100.))
    }
}

#[vespertine::test]
fn clearing_the_reservation_after_a_retained_walk_up_leaves_the_natural_extent(
    cx: &mut TestAppContext,
) {
    let state = ListState::new(1, ListAlignment::Top, px(0.));
    state.set_trailing_reservation(Some((0, px(0.))));
    let window = cx.open_window(size(px(100.), px(100.)), {
        let state = state.clone();
        move |_, _| RetainedRows { state }
    });
    let draw = |cx: &mut TestAppContext| {
        // The root view is rebuilt; its rows' recorded output stays replayable.
        window
            .update(cx, |_, window, cx| {
                window.set_retention_enabled(true);
                cx.notify();
            })
            .unwrap();
        crate::AppContext::update_window(cx, window.into(), |_, window, cx| {
            window.draw(cx).clear(cx)
        })
        .unwrap();
    };
    draw(cx);
    draw(cx);
    assert_eq!((extra(&state), total(&state)), (80., 100.));
    // Anchored past the last row, the pass walks up to the reserved row, which must be
    // measured naturally rather than replayed with its padded height.
    state.scroll_to_end();
    draw(cx);
    assert_eq!((extra(&state), total(&state)), (80., 100.));
    assert!(!state.trailing_reservation_filled());
    state.set_trailing_reservation(None);
    draw(cx);
    assert_eq!(total(&state), 20.);
    assert_eq!(state.max_offset_for_scrollbar().y, px(0.));
}

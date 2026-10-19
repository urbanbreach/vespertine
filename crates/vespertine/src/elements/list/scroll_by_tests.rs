//! ELM-002: incremental scroll-by from negative item offsets.

use super::*;

pub(super) fn state_with_heights(heights: &[f32]) -> ListState {
    let state = ListState::new(heights.len(), ListAlignment::Top, px(0.));
    state.0.borrow_mut().items = SumTree::from_iter(
        heights.iter().map(|&height| ListItem::Measured {
            size: size(px(10.), px(height)),
            focus_handle: None,
        }),
        (),
    );
    state
}

fn scrolled_from(state: &ListState, item_ix: usize, offset: f32, distance: f32) -> (usize, f32) {
    state.scroll_to(ListOffset {
        item_ix,
        offset_in_item: px(offset),
    });
    state.scroll_by(px(distance));
    let top = state.logical_scroll_top();
    (top.item_ix, top.offset_in_item.as_f32())
}

#[test]
fn a1_negative_anchor_offset_resolves_backward() {
    let state = state_with_heights(&[20.; 5]);
    for (distance, expected) in [
        (1., (1, 11.)),
        (9., (1, 19.)),
        (10., (2, 0.)),
        (11., (2, 1.)),
        (-1., (1, 9.)),
    ] {
        assert_eq!(
            scrolled_from(&state, 2, -10., distance),
            expected,
            "distance {distance}"
        );
    }
}

#[test]
fn a2_clamps_to_zero() {
    let state = state_with_heights(&[20.; 5]);
    assert_eq!(scrolled_from(&state, 1, 5., -100.), (0, 0.));
    assert_eq!(scrolled_from(&state, 0, 0., -5.), (0, 0.));
}

#[test]
fn a3_nonnegative_offsets() {
    let state = state_with_heights(&[20.; 5]);
    assert_eq!(scrolled_from(&state, 2, 5., 10.), (2, 15.));
    assert_eq!(scrolled_from(&state, 2, 5., 20.), (3, 5.));
    assert_eq!(scrolled_from(&state, 2, 5., -10.), (1, 15.));
    assert_eq!(scrolled_from(&state, 1, 0., 20.), (2, 0.));
    assert_eq!(scrolled_from(&state, 2, 0., -20.), (1, 0.));
}

#[test]
fn a4_zero_distance_and_follow_mode() {
    let state = state_with_heights(&[20.; 5]);
    assert_eq!(scrolled_from(&state, 2, -10., 0.), (2, -10.));

    state.set_follow_mode(FollowMode::Tail);
    state.0.borrow_mut().logical_scroll_top = Some(ListOffset {
        item_ix: 1,
        offset_in_item: px(0.),
    });
    state.scroll_by(px(5.));
    assert!(state.is_following_tail());
    state.scroll_by(px(-1.));
    assert!(!state.is_following_tail());
}

#[test]
fn a5_variable_heights_sweep_matches_prefix_sums() {
    let heights = [20., 50., 10., 30.];
    let state = state_with_heights(&heights);
    let starts = [0., 20., 70., 80., 110.];
    for (anchor_ix, anchor_offset) in [(1, -15.), (2, -5.), (3, -9.), (2, -10.), (3, -30.)] {
        let old = starts[anchor_ix] + anchor_offset;
        for distance in -60..=60 {
            let distance = distance as f32;
            if distance == 0. {
                // R2: a zero distance leaves the anchor unchanged.
                assert_eq!(
                    scrolled_from(&state, anchor_ix, anchor_offset, 0.),
                    (anchor_ix, anchor_offset)
                );
                continue;
            }
            let target = (old + distance).max(0.);
            let (ix, offset) = scrolled_from(&state, anchor_ix, anchor_offset, distance);
            let expected_ix = starts
                .iter()
                .rposition(|&start| start <= target)
                .unwrap()
                .min(heights.len());
            let (expected_ix, expected_offset) = if target >= 110. {
                (4, target - 110.)
            } else {
                (expected_ix, target - starts[expected_ix])
            };
            assert_eq!(
                (ix, offset),
                (expected_ix, expected_offset),
                "anchor ({anchor_ix}, {anchor_offset}) distance {distance}"
            );
        }
    }
}

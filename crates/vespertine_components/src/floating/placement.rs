//! Contained placement inside caller-supplied bounds (CMP-002 R18, R19).
//!
//! A contained menu is placed after layout from its measured size, so every function here is a
//! pure computation over geometry; the anchored menu builders feed it the measured
//! size.

use vespertine::{Bounds, Pixels, Point, Size, point, px, size};

/// Gap between the trigger and a contained menu.
pub const CONTAINED_GAP: Pixels = px(6.);
/// Upper bound of a contained menu's height.
pub const MAX_CONTAINED_HEIGHT: Pixels = px(320.);
/// Inset of the default containment limits of a page or dialog.
pub const CONTAINMENT_INSET: Pixels = px(8.);

/// Which end of the trigger a contained menu aligns to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContainedAlign {
    /// The menu's left edge at the trigger's left edge; the anchor x is the trigger's left.
    #[default]
    Start,
    /// The menu's right edge at the trigger's right edge; the anchor x is the trigger's right.
    End,
}

/// The result of [`place_contained`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainedPlacement {
    /// Window origin of the menu.
    pub origin: Point<Pixels>,
    /// The menu sits below the trigger (otherwise above it).
    pub below: bool,
}

/// Places a menu of `menu` size.
///
/// `anchor` is the trigger's bottom edge at its left edge (start alignment) or right edge (end
/// alignment). The menu goes below (`anchor.y + gap`) when it fits above `limits.bottom`,
/// otherwise above (`anchor.y − trigger_height − gap − menu.height`); the origin is then clamped
/// into `limits`, so the menu never slides over the trigger while a side has room.
pub fn place_contained(
    anchor: Point<Pixels>,
    trigger_height: Pixels,
    limits: Bounds<Pixels>,
    menu: Size<Pixels>,
    align: ContainedAlign,
) -> ContainedPlacement {
    let (anchor_x, anchor_y) = (f32::from(anchor.x), f32::from(anchor.y));
    let (width, height) = (f32::from(menu.width), f32::from(menu.height));
    let gap = f32::from(CONTAINED_GAP);
    let (left, top) = (f32::from(limits.left()), f32::from(limits.top()));
    let (right, bottom) = (f32::from(limits.right()), f32::from(limits.bottom()));

    let below_y = anchor_y + gap;
    let above_y = anchor_y - f32::from(trigger_height) - gap - height;
    let below = below_y + height <= bottom;
    let y = if below { below_y } else { above_y };
    let x = match align {
        ContainedAlign::Start => anchor_x,
        ContainedAlign::End => anchor_x - width,
    };
    ContainedPlacement {
        origin: point(
            px(x.clamp(left, (right - width).max(left))),
            px(y.clamp(top, (bottom - height).max(top))),
        ),
        below,
    }
}

/// Maximum size of a contained menu: height `clamp((limits.height − trigger height) / 2 − 6, 1,
/// 320)` (half the usable height guarantees room on one side), width `limits.width`.
pub fn contained_max_size(limits: Bounds<Pixels>, trigger_height: Pixels) -> Size<Pixels> {
    let height = ((f32::from(limits.size.height) - f32::from(trigger_height)) / 2.
        - f32::from(CONTAINED_GAP))
    .max(1.)
    .min(f32::from(MAX_CONTAINED_HEIGHT));
    size(limits.size.width, px(height))
}

/// Available height of the scrolling rows of a menu with fixed heading chrome: the card's
/// maximum height minus the chrome height, at least 1.
pub fn rows_max_height(card_max_height: Pixels, chrome_height: Pixels) -> Pixels {
    px((f32::from(card_max_height) - f32::from(chrome_height)).max(1.))
}

/// Default containment limits of a page or dialog: its bounds inset by 8 px on every side, with
/// width and height at least 1.
pub fn containment_limits(container: Bounds<Pixels>) -> Bounds<Pixels> {
    let inset = f32::from(CONTAINMENT_INSET);
    Bounds::new(
        point(
            px(f32::from(container.origin.x) + inset),
            px(f32::from(container.origin.y) + inset),
        ),
        size(
            px((f32::from(container.size.width) - 2. * inset).max(1.)),
            px((f32::from(container.size.height) - 2. * inset).max(1.)),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    /// A6.
    #[test]
    fn a6_end_aligned_vertical_choices() {
        let limits = limits(100., 80., 800., 600.);
        let menu = size(px(260.), px(280.));
        for anchor_y in [120., 350., 650.] {
            let placed = place_contained(
                point(px(870.), px(anchor_y)),
                px(34.),
                limits,
                menu,
                ContainedAlign::End,
            );
            let (x, y) = (f32::from(placed.origin.x), f32::from(placed.origin.y));
            assert_eq!(x, 610., "end alignment: anchor.x - width");
            assert!(x >= 100. && x + 260. <= 900. && y >= 80. && y + 280. <= 680.);
            let below_ok = y >= anchor_y + 6.;
            let above_ok = y + 280. <= anchor_y - 40.;
            assert!(below_ok || above_ok, "anchor {anchor_y}: y = {y}");
            assert_eq!(placed.below, below_ok);
        }
    }

    #[test]
    fn a6_start_alignment_and_horizontal_clamp() {
        let limits = limits(100., 80., 800., 600.);
        let menu = size(px(260.), px(280.));
        let at = |x: f32| {
            place_contained(
                point(px(x), px(200.)),
                px(34.),
                limits,
                menu,
                ContainedAlign::Start,
            )
            .origin
            .x
        };
        assert_eq!(at(300.), px(300.));
        assert_eq!(at(800.), px(640.));
        assert_eq!(at(10.), px(100.));
    }

    /// A7: a 30-row menu of 32 px rows.
    #[test]
    fn a7_tall_menu_is_capped_and_sits_above() {
        let limits = limits(100., 80., 600., 400.);
        let max = contained_max_size(limits, px(34.));
        assert_eq!(max.height, px(177.));
        assert_eq!(max.width, px(600.));
        let placed = place_contained(
            point(px(300.), px(334.)),
            px(34.),
            limits,
            size(px(240.), max.height),
            ContainedAlign::Start,
        );
        assert!(!placed.below);
        // Trigger top is 300: at least 6 px clearance above it.
        assert!(f32::from(placed.origin.y) + 177. <= 300. - 6.);
        assert!(f32::from(placed.origin.y) >= 80.);
    }

    #[test]
    fn menu_larger_than_limits_pins_to_the_top_left() {
        let limits = limits(10., 20., 100., 50.);
        let placed = place_contained(
            point(px(60.), px(40.)),
            px(10.),
            limits,
            size(px(300.), px(200.)),
            ContainedAlign::End,
        );
        assert_eq!(placed.origin, point(px(10.), px(20.)));
    }

    #[test]
    fn max_height_clamps() {
        let max = |h: f32, t: f32| contained_max_size(limits(0., 0., 500., h), px(t)).height;
        assert_eq!(max(2000., 34.), px(320.));
        assert_eq!(max(40., 34.), px(1.));
        assert_eq!(max(100., 40.), px(24.));
    }

    #[test]
    fn rows_height_subtracts_chrome_and_stays_positive() {
        assert_eq!(rows_max_height(px(200.), px(48.)), px(152.));
        assert_eq!(rows_max_height(px(40.), px(48.)), px(1.));
    }

    #[test]
    fn containment_limits_inset_by_eight() {
        let inset = containment_limits(limits(0., 0., 400., 300.));
        assert_eq!(inset, limits(8., 8., 384., 284.));
        let tiny = containment_limits(limits(0., 0., 4., 4.));
        assert_eq!(tiny.size, size(px(1.), px(1.)));
    }
}

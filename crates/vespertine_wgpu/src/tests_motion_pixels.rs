//! Renderer-level pixel tests for ANM-002 A8: both fidelity scenes (FID-ANM-002-1 and
//! FID-ANM-002-2) through the production painting path on the offscreen test host, with their
//! motion clocks frozen at the stated sample point (500 ms) and compared against the scene
//! description under the pixel tolerance of the scene protocol.

use crate::tests_host::RenderHost;
use image::{Rgba, RgbaImage};
use std::time::Duration;
use vespertine::motion::{
    Easing, MotionDescription, MotionExt as _, MotionInitial, MotionProps, MotionTransition,
};
use vespertine::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, px, rgb};

/// Scale factor of the fidelity scene environment.
const SCALE: u32 = 2;
const FROZEN_AT: Duration = Duration::from_millis(500);

fn linear_one_second() -> MotionTransition {
    MotionTransition::new().duration(1.0).easing(Easing::LINEAR)
}

/// FID-ANM-002-1: a width track 0 -> 100 over one second, frozen at 500 ms.
fn width_scene() -> AnyElement {
    let description = MotionDescription::new(MotionProps::new().width(100.0))
        .initial(MotionInitial::Style(MotionProps::new().width(0.0)))
        .transition(linear_one_second());
    div()
        .size_full()
        .bg(rgb(0xFFFFFF))
        .child(
            div()
                .absolute()
                .left(px(20.))
                .top(px(40.))
                .h(px(40.))
                .rounded(px(4.))
                .bg(rgb(0x3366CC))
                .with_motion("width-track", description)
                .frozen_at(FROZEN_AT),
        )
        .into_any_element()
}

fn card(top: f32, fill: u32, label: &'static str) -> vespertine::Div {
    div()
        .absolute()
        .left(px(280.))
        .top(px(top))
        .w(px(240.))
        .h(px(48.))
        .pl(px(16.))
        .pt(px(14.))
        .rounded(px(8.))
        .bg(rgb(fill))
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(rgb(0xF0F1F3))
        .whitespace_nowrap()
        .child(label)
}

/// FID-ANM-002-2: an opacity track 0 -> 1 over one second on the first card, frozen at 500 ms,
/// beside a stationary card.
fn opacity_scene() -> AnyElement {
    let description = MotionDescription::new(MotionProps::new().opacity(1.0))
        .initial(MotionInitial::Style(MotionProps::new().opacity(0.0)))
        .transition(linear_one_second());
    div()
        .size_full()
        .bg(rgb(0x181A1F))
        .font_family("Noto Sans")
        .child(
            card(216., 0x405470, "Moving input")
                .with_motion("opacity-track", description)
                .frozen_at(FROZEN_AT),
        )
        .child(card(300., 0x30343D, "Stationary"))
        .into_any_element()
}

/// A filled rounded rectangle of the scene description, in logical pixels.
struct Fill {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    color: u32,
}

fn channels(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

/// Whether the device pixel's center lies inside the rounded rectangle.
fn covers(fill: &Fill, px_x: u32, px_y: u32) -> bool {
    let s = SCALE as f32;
    let (x, y) = ((px_x as f32 + 0.5) / s, (px_y as f32 + 0.5) / s);
    let (right, bottom) = (fill.x + fill.width, fill.y + fill.height);
    if x < fill.x || x >= right || y < fill.y || y >= bottom {
        return false;
    }
    // Distance into the corner circle, when the point lies in a corner square.
    let corner_x = if x < fill.x + fill.radius {
        Some(fill.x + fill.radius)
    } else if x > right - fill.radius {
        Some(right - fill.radius)
    } else {
        None
    };
    let corner_y = if y < fill.y + fill.radius {
        Some(fill.y + fill.radius)
    } else if y > bottom - fill.radius {
        Some(bottom - fill.radius)
    } else {
        None
    };
    match (corner_x, corner_y) {
        (Some(cx), Some(cy)) => (x - cx).powi(2) + (y - cy).powi(2) <= fill.radius.powi(2),
        _ => true,
    }
}

/// The scene as specified: the window background with each fill painted in order. Text is not
/// part of this image.
fn described(width: u32, height: u32, background: u32, fills: &[Fill]) -> RgbaImage {
    let [br, bg, bb] = channels(background);
    RgbaImage::from_fn(width * SCALE, height * SCALE, |x, y| {
        let [r, g, b] = fills
            .iter()
            .rev()
            .find(|fill| covers(fill, x, y))
            .map_or([br, bg, bb], |fill| channels(fill.color));
        Rgba([r, g, b, 255])
    })
}

/// The pixel comparison of the scene protocol: fails when more than 0.5% of the pixels differ by
/// more than 8/255 in any channel. `skip` excludes regions the description does not specify at
/// pixel level (the label text boxes).
#[track_caller]
fn assert_within_protocol_tolerance(
    actual: &RgbaImage,
    expected: &RgbaImage,
    skip: impl Fn(u32, u32) -> bool,
) {
    let differing = differing_pixels(actual, expected, skip);
    let total = (actual.width() * actual.height()) as usize;
    assert!(
        differing * 1000 <= total * 5,
        "{differing} of {total} pixels differ by more than 8/255"
    );
}

fn differing_pixels(
    actual: &RgbaImage,
    expected: &RgbaImage,
    skip: impl Fn(u32, u32) -> bool,
) -> usize {
    assert_eq!(actual.dimensions(), expected.dimensions());
    actual
        .enumerate_pixels()
        .filter(|(x, y, pixel)| {
            !skip(*x, *y)
                && pixel
                    .0
                    .iter()
                    .zip(expected.get_pixel(*x, *y).0)
                    .any(|(a, e)| a.abs_diff(e) > 8)
        })
        .count()
}

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

#[track_caller]
fn assert_color(image: &RgbaImage, x: u32, y: u32, expected: u32, what: &str) {
    let actual = pixel(image, x, y);
    assert!(
        actual
            .iter()
            .zip(channels(expected))
            .all(|(a, e)| a.abs_diff(e) <= 2),
        "{what} at ({x}, {y}): got #{:02X}{:02X}{:02X}, expected #{expected:06X}",
        actual[0],
        actual[1],
        actual[2],
    );
}

/// Device-pixel bounds `(left, top, right, bottom)` of the pixels in the region that differ from
/// `background` by more than the protocol threshold, with the brightest red channel found.
fn ink(
    image: &RgbaImage,
    region: (u32, u32, u32, u32),
    background: u32,
) -> Option<((u32, u32, u32, u32), u8)> {
    let background = channels(background);
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    let mut brightest = 0;
    for y in region.1..region.3 {
        for x in region.0..region.2 {
            let p = pixel(image, x, y);
            if p.iter().zip(background).any(|(a, e)| a.abs_diff(e) > 8) {
                brightest = brightest.max(p[0]);
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(x), b.max(y)),
                });
            }
        }
    }
    bounds.map(|bounds| (bounds, brightest))
}

/// A8: FID-ANM-002-1 reproduces the described scene within the protocol tolerance: the rectangle
/// is exactly (20, 40, 50, 40) in #3366CC at full opacity and every other pixel is white.
#[test]
fn fid_anm_002_1_width_track_pixels() {
    let mut host = RenderHost::new();
    let image = host.render_logical(240., 120., |_, _| width_scene());
    assert_eq!(image.dimensions(), (480, 240));

    let expected = described(
        240,
        120,
        0xFFFFFF,
        &[Fill {
            x: 20.,
            y: 40.,
            width: 50.,
            height: 40.,
            radius: 4.,
            color: 0x3366CC,
        }],
    );
    assert_within_protocol_tolerance(&image, &expected, |_, _| false);

    // Landmarks across the 50 px wide rectangle (device x = 40..140, y = 80..160).
    assert_color(&image, 90, 120, 0x3366CC, "rectangle center");
    assert_color(&image, 40 + 1, 120, 0x3366CC, "left edge, inside");
    assert_color(&image, 40 - 1, 120, 0xFFFFFF, "left edge, outside");
    assert_color(&image, 139, 120, 0x3366CC, "right edge, inside (width 50)");
    assert_color(&image, 140, 120, 0xFFFFFF, "right edge, outside");
    assert_color(&image, 90, 81, 0x3366CC, "top edge, inside");
    assert_color(&image, 90, 79, 0xFFFFFF, "top edge, outside");
    assert_color(&image, 90, 159, 0x3366CC, "bottom edge, inside");
    assert_color(&image, 90, 160, 0xFFFFFF, "bottom edge, outside");
    assert_color(&image, 40, 80, 0xFFFFFF, "rounded top-left corner");
    assert_color(&image, 139, 159, 0xFFFFFF, "rounded bottom-right corner");
    // A full 100 px width (the track's target) is not reached at 500 ms.
    assert_color(&image, 200, 120, 0xFFFFFF, "beyond the half-way width");

    let ink = ink(&image, (0, 0, 480, 240), 0xFFFFFF).expect("rectangle is painted");
    assert_eq!(ink.0, (40, 80, 139, 159), "painted extent");
}

/// A8: FID-ANM-002-2 reproduces the described scene within the protocol tolerance: the moving
/// card's fill composites to #2C3748, its label is dimmed to half intensity over that fill (not
/// group-faded), and the stationary card is opaque.
#[test]
fn fid_anm_002_2_opacity_cards_pixels() {
    let mut host = RenderHost::new();
    let image = host.render_logical(800., 480., |_, _| opacity_scene());
    assert_eq!(image.dimensions(), (1600, 960));

    let expected = described(
        800,
        480,
        0x181A1F,
        &[
            Fill {
                x: 280.,
                y: 216.,
                width: 240.,
                height: 48.,
                radius: 8.,
                color: 0x2C3748,
            },
            Fill {
                x: 280.,
                y: 300.,
                width: 240.,
                height: 48.,
                radius: 8.,
                color: 0x30343D,
            },
        ],
    );
    // The label text boxes are (296, 230, 240 - 16, 20) and (296, 314, ..): glyph pixels are
    // checked separately below.
    let in_label = |x: u32, y: u32| {
        let (x, y) = (x / SCALE, y / SCALE);
        (296..520).contains(&x) && ((230..250).contains(&y) || (314..334).contains(&y))
    };
    assert_within_protocol_tolerance(&image, &expected, in_label);

    // Fills: half-opacity #405470 over #181A1F, and the opaque sibling.
    assert_color(&image, 1000, 450, 0x2C3748, "moving card fill");
    assert_color(&image, 1000, 660, 0x30343D, "stationary card fill");
    assert_color(&image, 1100, 450, 0x181A1F, "right of the moving card");
    assert_color(&image, 559, 450, 0x181A1F, "left of the moving card");
    assert_color(&image, 560, 450, 0x2C3748, "moving card left edge");
    assert_color(&image, 1039, 450, 0x2C3748, "moving card right edge");
    assert_color(&image, 1000, 431, 0x181A1F, "above the moving card");
    assert_color(&image, 1000, 528, 0x181A1F, "below the moving card");
    assert_color(&image, 1000, 599, 0x181A1F, "above the stationary card");
    assert_color(&image, 560, 696, 0x181A1F, "below the stationary card");
    assert_color(&image, 560, 432, 0x181A1F, "moving card rounded corner");
    assert_color(&image, 560, 600, 0x181A1F, "stationary card rounded corner");

    // Labels: ink stays inside each label's 20 px line box starting at the 16 px / 14 px inset.
    let (moving, moving_peak) =
        ink(&image, (570, 440, 1030, 520), 0x2C3748).expect("moving label is painted");
    let (stationary, stationary_peak) =
        ink(&image, (570, 608, 1030, 688), 0x30343D).expect("stationary label is painted");
    for ((left, top, right, bottom), box_top) in [(moving, 230), (stationary, 314)] {
        assert!(left >= 296 * SCALE, "label starts at x = {left}");
        assert!(
            right < 296 * SCALE + 120 * SCALE,
            "label ends at x = {right}"
        );
        assert!(
            top >= box_top * SCALE && bottom < (box_top + 20) * SCALE,
            "label rows {top}..={bottom} lie in the line box at y = {box_top}"
        );
    }
    // Glyph coverage is multiplied by 0.5 before compositing: the strongest pixel of the moving
    // label is far below the label color (#F0), near (#F0 + #2C) / 2 = #86 and clearly above the
    // fill; the stationary label reaches the label color.
    assert!(
        (0x7A..=0x96).contains(&moving_peak),
        "moving label peak {moving_peak:#04X}"
    );
    assert!(
        stationary_peak >= 0xE8,
        "stationary label peak {stationary_peak:#04X}"
    );
}

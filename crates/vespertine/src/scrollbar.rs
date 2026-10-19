//! Classic scrollbar thumb indicators (CMP-031 R1-R7).
//!
//! A scrollable element or virtual list whose style names a valid scrollbar color
//! ([`crate::Styled::scrollbar_color`]) paints one solid rounded thumb per scrollable axis whose
//! content overflows by more than one pixel. The thumbs are paint-only: they reserve no layout
//! space, take no pointer input and are anchored to the edges of the viewport's padding box
//! instead of moving with the content. They are derived from the container's scroll offset at
//! paint time, so they repaint in the very frame the content scrolls.
//!
//! The geometry is exposed as pure functions so it can be tested without a window:
//! [`classic_thumb_span`] is the shared one-axis vocabulary (R4) and [`classic_thumb_rects`]
//! combines both axes (R5, R6).

use crate::{Bounds, Hsla, Pixels, Point, Window, fill, point, px, size};

/// The default thumb thickness in logical pixels (R6).
pub const CLASSIC_SCROLLBAR_THICKNESS: f32 = 6.0;
/// The shortest thumb in logical pixels (or the track, when that is shorter) (R4).
pub const CLASSIC_MIN_THUMB: f32 = 20.0;
/// The overflow, in logical pixels, a viewport must exceed before a thumb is shown (R1, R3).
pub const CLASSIC_OVERFLOW_THRESHOLD: f32 = 1.0;

/// Whether `color` is a valid scrollbar color: every component is finite and the color is not
/// fully transparent. Anything else disables classic scrollbar painting (R3).
pub fn is_valid_scrollbar_color(color: Hsla) -> bool {
    color.h.is_finite()
        && color.s.is_finite()
        && color.l.is_finite()
        && color.a.is_finite()
        && color.a > 0.0
}

/// The position of a thumb along its track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbSpan {
    /// Distance from the track start.
    pub start: f32,
    /// Thumb length.
    pub length: f32,
}

/// Thumb geometry along one axis (R1, R4).
///
/// `track` is the track length, `max_scroll` the maximum scroll distance and `offset` the
/// scroll offset (zero or negative as the content scrolls up or left; out-of-range offsets
/// clamp). Returns `None` for a content that fits (`max_scroll <= 1`) and for a track that is
/// not positive.
pub fn classic_thumb_span(track: f32, max_scroll: f32, offset: f32) -> Option<ThumbSpan> {
    if track.is_nan() || max_scroll.is_nan() || offset.is_nan() {
        return None;
    }
    if track <= 0.0 || max_scroll <= CLASSIC_OVERFLOW_THRESHOLD {
        return None;
    }
    let length = (track * track / (track + max_scroll)).clamp(CLASSIC_MIN_THUMB.min(track), track);
    let progress = (-offset / max_scroll).clamp(0.0, 1.0);
    Some(ThumbSpan {
        start: progress * (track - length),
        length,
    })
}

/// The classic thumbs of one viewport, in window coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClassicThumbs {
    /// The vertical thumb, at the right edge of the viewport.
    pub vertical: Option<Bounds<Pixels>>,
    /// The horizontal thumb, at the bottom edge of the viewport.
    pub horizontal: Option<Bounds<Pixels>>,
}

fn snap(value: f32) -> f32 {
    value.round()
}

/// The snapped rectangles of both thumbs (R3, R5, R6).
///
/// `viewport` is the padding box of the scrollable element; `max_scroll` and `offset` are the
/// per-axis maximum scroll distance and scroll offset; `thickness` the thumb thickness in logical
/// pixels. When both thumbs show, each track is shortened by the thickness so the corner stays
/// free. Positions and sizes are rounded to whole logical pixels, half away from zero,
/// separately for x, y, width and height, relative to the viewport origin.
pub fn classic_thumb_rects(
    viewport: Bounds<Pixels>,
    max_scroll: Point<Pixels>,
    offset: Point<Pixels>,
    thickness: f32,
    horizontal_enabled: bool,
    vertical_enabled: bool,
) -> ClassicThumbs {
    let width = f32::from(viewport.size.width);
    let height = f32::from(viewport.size.height);
    let overflows = |max: Pixels| f32::from(max) > CLASSIC_OVERFLOW_THRESHOLD;
    let show_vertical = vertical_enabled && overflows(max_scroll.y);
    let show_horizontal = horizontal_enabled && overflows(max_scroll.x);
    let both = show_vertical && show_horizontal;
    let reserved = if both { thickness } else { 0.0 };

    let vertical = show_vertical
        .then(|| {
            classic_thumb_span(
                height - reserved,
                f32::from(max_scroll.y),
                f32::from(offset.y),
            )
        })
        .flatten()
        .map(|span| {
            Bounds::new(
                viewport.origin + point(px(snap(width - thickness)), px(snap(span.start))),
                size(px(snap(thickness)), px(snap(span.length))),
            )
        });
    let horizontal = show_horizontal
        .then(|| {
            classic_thumb_span(
                width - reserved,
                f32::from(max_scroll.x),
                f32::from(offset.x),
            )
        })
        .flatten()
        .map(|span| {
            Bounds::new(
                viewport.origin + point(px(snap(span.start)), px(snap(height - thickness))),
                size(px(snap(span.length)), px(snap(thickness))),
            )
        });
    ClassicThumbs {
        vertical,
        horizontal,
    }
}

/// Paints the classic thumbs: solid rounded rectangles in exactly `color`, with no track fill,
/// border, shadow or animation (R6).
pub fn paint_classic_thumbs(
    thumbs: &ClassicThumbs,
    thickness: f32,
    color: Hsla,
    window: &mut Window,
) {
    let radius = px(thickness / 2.0);
    for bounds in [thumbs.vertical, thumbs.horizontal].into_iter().flatten() {
        window.paint_quad(fill(bounds, color).corner_radii(radius));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a3_geometry_table() {
        let span = classic_thumb_span(100.0, 200.0, 0.0).unwrap();
        assert!(approx(span.length, 33.33) && approx(span.start, 0.0));
        let span = classic_thumb_span(100.0, 200.0, -200.0).unwrap();
        assert!(approx(span.length, 33.33) && approx(span.start, 66.67));
        let span = classic_thumb_span(100.0, 200.0, -100.0).unwrap();
        assert!(approx(span.start, 33.33));
        let span = classic_thumb_span(100.0, 200.0, -450.0).unwrap();
        assert!(approx(span.start, 66.67));
        let span = classic_thumb_span(100.0, 200.0, 30.0).unwrap();
        assert!(approx(span.start, 0.0));
        assert!(approx(
            classic_thumb_span(100.0, 100000.0, 0.0).unwrap().length,
            20.0
        ));
        assert!(approx(
            classic_thumb_span(10.0, 100000.0, 0.0).unwrap().length,
            10.0
        ));
    }

    #[test]
    fn fitting_content_and_empty_tracks_have_no_thumb() {
        assert!(classic_thumb_span(100.0, 1.0, 0.0).is_none());
        assert!(classic_thumb_span(100.0, 0.0, 0.0).is_none());
        assert!(classic_thumb_span(0.0, 100.0, 0.0).is_none());
        assert!(classic_thumb_span(-4.0, 100.0, 0.0).is_none());
        assert!(classic_thumb_span(100.0, 1.5, 0.0).is_some());
    }

    #[test]
    fn invalid_colors_are_rejected() {
        assert!(is_valid_scrollbar_color(Hsla {
            h: 0.1,
            s: 0.5,
            l: 0.5,
            a: 1.0
        }));
        assert!(!is_valid_scrollbar_color(Hsla {
            h: f32::NAN,
            s: 0.5,
            l: 0.5,
            a: 1.0
        }));
        assert!(!is_valid_scrollbar_color(Hsla {
            h: 0.1,
            s: 0.5,
            l: 0.5,
            a: 0.0
        }));
    }

    #[test]
    fn fid_cmp_031_geometry() {
        // FID-CMP-031-1: viewport at (20, 20), 300 x 240, maximum scroll 720 vertically.
        let viewport = Bounds::new(point(px(20.0), px(20.0)), size(px(300.0), px(240.0)));
        let thumbs = classic_thumb_rects(
            viewport,
            point(px(0.0), px(720.0)),
            point(px(0.0), px(-360.0)),
            6.0,
            true,
            true,
        );
        assert_eq!(
            thumbs.vertical,
            Some(Bounds::new(
                point(px(314.0), px(110.0)),
                size(px(6.0), px(60.0))
            ))
        );
        assert_eq!(thumbs.horizontal, None);
        // FID-CMP-031-2: both axes overflow, so each track is shortened by the thickness.
        let thumbs = classic_thumb_rects(
            viewport,
            point(px(300.0), px(720.0)),
            point(px(-150.0), px(-360.0)),
            6.0,
            true,
            true,
        );
        assert_eq!(
            thumbs.vertical,
            Some(Bounds::new(
                point(px(314.0), px(108.0)),
                size(px(6.0), px(57.0))
            ))
        );
        assert_eq!(
            thumbs.horizontal,
            Some(Bounds::new(
                point(px(94.0), px(254.0)),
                size(px(146.0), px(6.0))
            ))
        );
    }

    #[test]
    fn disabled_axes_paint_no_thumb() {
        let viewport = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(240.0)));
        let thumbs = classic_thumb_rects(
            viewport,
            point(px(300.0), px(720.0)),
            point(px(0.0), px(0.0)),
            6.0,
            false,
            true,
        );
        assert!(thumbs.vertical.is_some() && thumbs.horizontal.is_none());
        // With only one visible axis the track is not shortened.
        assert_eq!(thumbs.vertical.unwrap().size.height, px(60.0));
    }
}

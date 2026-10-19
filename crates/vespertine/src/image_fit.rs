//! Fitted image geometry (RND-004): the shared source-crop routine of the image painters and
//! the pure cover-fit, pan and anchor-zoom helpers that compute a fitted content rectangle
//! without touching the decoded image.

use crate::{AtlasTile, Bounds, DevicePixels, Pixels, Point, Size, point, px, size};

/// Smallest fitted width or height used as a divisor when cropping, in logical pixels.
const MIN_FITTED_DIVISOR: f32 = 1.0;

/// Crops `tile` to the part of its image shown by `visible` when the whole image is laid out
/// in `fitted` (RND-004 R9).
///
/// Both rectangles are in logical pixels. The crop origin is the tile origin plus
/// `round(origin ratio * tile size)`, the crop size is `round(size ratio * tile size)` and at
/// least one texel per axis, with rounding to the nearest texel. The fitted width and height
/// divide as at least one logical pixel. Equal rectangles use the whole tile. The result
/// always lies inside `tile`.
pub(crate) fn crop_tile(
    tile: AtlasTile,
    visible: Bounds<Pixels>,
    fitted: Bounds<Pixels>,
) -> AtlasTile {
    if visible == fitted {
        return tile;
    }
    let tile_origin = tile.bounds.origin;
    let tile_size = tile.bounds.size;
    if tile_size.width.0 <= 0 || tile_size.height.0 <= 0 {
        return tile;
    }

    let fitted_width = fitted.size.width.0.max(MIN_FITTED_DIVISOR);
    let fitted_height = fitted.size.height.0.max(MIN_FITTED_DIVISOR);
    let x_offset_ratio = (visible.origin.x.0 - fitted.origin.x.0) / fitted_width;
    let y_offset_ratio = (visible.origin.y.0 - fitted.origin.y.0) / fitted_height;
    let width_ratio = visible.size.width.0 / fitted_width;
    let height_ratio = visible.size.height.0 / fitted_height;

    let (origin_x, width) = crop_axis(
        tile_origin.x.0,
        tile_size.width.0,
        x_offset_ratio,
        width_ratio,
    );
    let (origin_y, height) = crop_axis(
        tile_origin.y.0,
        tile_size.height.0,
        y_offset_ratio,
        height_ratio,
    );
    AtlasTile {
        bounds: Bounds {
            origin: point(DevicePixels(origin_x), DevicePixels(origin_y)),
            size: size(DevicePixels(width), DevicePixels(height)),
        },
        ..tile
    }
}

/// One axis of [`crop_tile`]: the cropped origin and extent in texels.
fn crop_axis(tile_origin: i32, tile_extent: i32, offset_ratio: f32, size_ratio: f32) -> (i32, i32) {
    let tile_end = tile_origin + tile_extent;
    let origin = (tile_origin + (offset_ratio * tile_extent as f32).round() as i32)
        .min(tile_end - 1)
        .max(tile_origin);
    let extent = ((size_ratio * tile_extent as f32).round() as i32)
        .max(1)
        .min(tile_end - origin);
    (origin, extent)
}

fn is_valid(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

/// Cover-fit scale of a source of `source` pixels in a viewport of `viewport` logical
/// pixels at `zoom`, or `None` for invalid geometry.
fn cover_scale(viewport: Size<Pixels>, source: Size<f32>, zoom: f32) -> Option<f32> {
    if !(is_valid(viewport.width.0)
        && is_valid(viewport.height.0)
        && is_valid(source.width)
        && is_valid(source.height)
        && is_valid(zoom))
    {
        return None;
    }
    Some((viewport.width.0 / source.width).max(viewport.height.0 / source.height) * zoom)
}

fn focal_is_valid(focal: Point<f32>) -> bool {
    !focal.x.is_nan() && !focal.y.is_nan()
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Computes the fitted content rectangle of a cover fit (RND-004 R13).
///
/// `source` is the image size in pixels, `zoom` multiplies the cover scale
/// `max(w / sw, h / sh)` and `focal` (each coordinate clamped to `[0, 1]`) selects which part
/// of the overflow is cropped on each axis: the fitted origin is the viewport origin plus
/// `(-overflow.x * focal.x, -overflow.y * focal.y)`. Zoom 1 with focal `(0.5, 0.5)` is the
/// centered object-fit cover rectangle. Zoom below 1 is outside the contract: the formula is
/// applied literally and may leave gaps.
///
/// Returns `None` when the viewport size, source size or zoom is not finite and positive,
/// or when a focal coordinate is NaN; a caller then paints nothing.
pub fn cover_fit(
    viewport: Bounds<Pixels>,
    source: Size<f32>,
    zoom: f32,
    focal: Point<f32>,
) -> Option<Bounds<Pixels>> {
    let scale = cover_scale(viewport.size, source, zoom)?;
    if !focal_is_valid(focal) {
        return None;
    }
    let fitted = size(px(source.width * scale), px(source.height * scale));
    let overflow_x = fitted.width.0 - viewport.size.width.0;
    let overflow_y = fitted.height.0 - viewport.size.height.0;
    Some(Bounds {
        origin: point(
            viewport.origin.x + px(-overflow_x * clamp01(focal.x)),
            viewport.origin.y + px(-overflow_y * clamp01(focal.y)),
        ),
        size: fitted,
    })
}

/// Returns the focal point after dragging the content by `delta` logical pixels (RND-004
/// R14): each coordinate changes by `-delta / overflow` on its axis and is clamped to
/// `[0, 1]`, where overflow is `fitted - viewport`. An axis whose overflow is not positive
/// keeps its focal value.
pub fn pan_focal(
    focal: Point<f32>,
    delta: Point<Pixels>,
    viewport: Size<Pixels>,
    fitted: Size<Pixels>,
) -> Point<f32> {
    let axis = |focal: f32, delta: Pixels, viewport: Pixels, fitted: Pixels| {
        let overflow = fitted.0 - viewport.0;
        if overflow > 0.0 {
            clamp01(focal - delta.0 / overflow)
        } else {
            focal
        }
    };
    point(
        axis(focal.x, delta.x, viewport.width, fitted.width),
        axis(focal.y, delta.y, viewport.height, fitted.height),
    )
}

/// Changes the zoom from `zoom` to `new_zoom` keeping the source point beneath `pointer`
/// (window-space logical pixels) in place whenever clamping permits (RND-004 R15).
///
/// With the current fitted origin `o` and cover scale `k`, the source point is
/// `u = (pointer - o) / k`; the desired origin after the change is `pointer - u * k'` and the
/// new focal coordinate is `-(desired origin - viewport origin) / overflow'`, clamped to
/// `[0, 1]`. An axis whose new overflow is not positive keeps its focal value. Returns the
/// new zoom and focal point, or `None` for invalid geometry or zoom (see [`cover_fit`]).
pub fn anchor_zoom(
    viewport: Bounds<Pixels>,
    source: Size<f32>,
    zoom: f32,
    focal: Point<f32>,
    new_zoom: f32,
    pointer: Point<Pixels>,
) -> Option<(f32, Point<f32>)> {
    let current = cover_fit(viewport, source, zoom, focal)?;
    let scale = cover_scale(viewport.size, source, zoom)?;
    let new_scale = cover_scale(viewport.size, source, new_zoom)?;

    let axis = |focal: f32,
                pointer: f32,
                origin: f32,
                viewport_origin: f32,
                viewport_extent: f32,
                source_extent: f32| {
        let source_point = (pointer - origin) / scale;
        let desired_origin = pointer - source_point * new_scale;
        let new_overflow = source_extent * new_scale - viewport_extent;
        if new_overflow > 0.0 {
            clamp01(-(desired_origin - viewport_origin) / new_overflow)
        } else {
            clamp01(focal)
        }
    };
    let new_focal = point(
        axis(
            focal.x,
            pointer.x.0,
            current.origin.x.0,
            viewport.origin.x.0,
            viewport.size.width.0,
            source.width,
        ),
        axis(
            focal.y,
            pointer.y.0,
            current.origin.y.0,
            viewport.origin.y.0,
            viewport.size.height.0,
            source.height,
        ),
    );
    Some((new_zoom, new_focal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtlasTextureId, AtlasTextureKind, TileId};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn tile(x: i32, y: i32, w: i32, h: i32) -> AtlasTile {
        AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Polychrome,
            },
            tile_id: TileId(0),
            padding: 0,
            bounds: Bounds {
                origin: point(DevicePixels(x), DevicePixels(y)),
                size: size(DevicePixels(w), DevicePixels(h)),
            },
        }
    }

    fn rect(tile: AtlasTile) -> [i32; 4] {
        [
            tile.bounds.origin.x.0,
            tile.bounds.origin.y.0,
            tile.bounds.size.width.0,
            tile.bounds.size.height.0,
        ]
    }

    #[test]
    fn crop_matches_the_specified_example() {
        // RND-004 A9: 512 x 256 source, visible (120, 60) 400 x 280 inside fitted (40, 60)
        // 560 x 280.
        let cropped = crop_tile(
            tile(0, 0, 512, 256),
            bounds(120.0, 60.0, 400.0, 280.0),
            bounds(40.0, 60.0, 560.0, 280.0),
        );
        assert_eq!(rect(cropped), [73, 0, 366, 256]);
        // The same crop inside an atlas tile that does not start at the origin.
        let cropped = crop_tile(
            tile(100, 50, 512, 256),
            bounds(120.0, 60.0, 400.0, 280.0),
            bounds(40.0, 60.0, 560.0, 280.0),
        );
        assert_eq!(rect(cropped), [173, 50, 366, 256]);
    }

    #[test]
    fn equal_rectangles_use_the_whole_tile() {
        let area = bounds(10.0, 20.0, 33.3, 44.4);
        assert_eq!(
            rect(crop_tile(tile(8, 8, 17, 9), area, area)),
            [8, 8, 17, 9]
        );
    }

    #[test]
    fn crop_keeps_at_least_one_texel() {
        let cropped = crop_tile(
            tile(0, 0, 100, 100),
            bounds(10.0, 10.0, 0.001, 0.001),
            bounds(0.0, 0.0, 1000.0, 1000.0),
        );
        assert_eq!(rect(cropped), [1, 1, 1, 1]);
    }

    #[test]
    fn crop_divisors_are_at_least_one_logical_pixel() {
        // Fitted 0.25 x 0.5 divides as 1 x 1: visible 0.5 x 0.5 is half the tile.
        let cropped = crop_tile(
            tile(0, 0, 64, 64),
            bounds(0.0, 0.0, 0.5, 0.5),
            bounds(0.0, 0.0, 0.25, 0.5),
        );
        assert_eq!(rect(cropped), [0, 0, 32, 32]);
    }

    #[test]
    fn crop_stays_inside_the_tile() {
        // A visible rectangle far right and below the fitted one clamps to the last texel.
        let cropped = crop_tile(
            tile(4, 4, 10, 10),
            bounds(500.0, 500.0, 40.0, 40.0),
            bounds(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(rect(cropped), [13, 13, 1, 1]);
        // A visible rectangle left of the fitted one clamps to the tile origin.
        let cropped = crop_tile(
            tile(4, 4, 10, 10),
            bounds(-500.0, -500.0, 40.0, 40.0),
            bounds(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(rect(cropped), [4, 4, 4, 4]);
        // Larger than the fitted rectangle: size clamps to the tile.
        let cropped = crop_tile(
            tile(4, 4, 10, 10),
            bounds(-10.0, -10.0, 300.0, 300.0),
            bounds(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(rect(cropped), [4, 4, 10, 10]);
    }

    fn fitted(viewport: Bounds<Pixels>, source: [f32; 2], zoom: f32, focal: [f32; 2]) -> [f32; 4] {
        let bounds = cover_fit(
            viewport,
            size(source[0], source[1]),
            zoom,
            point(focal[0], focal[1]),
        )
        .expect("valid geometry");
        [
            bounds.origin.x.0,
            bounds.origin.y.0,
            bounds.size.width.0,
            bounds.size.height.0,
        ]
    }

    #[test]
    fn cover_fit_follows_the_focal_point_and_zoom() {
        let viewport = bounds(0.0, 0.0, 640.0, 400.0);
        let source = [800.0, 400.0];
        assert_eq!(
            fitted(viewport, source, 1.0, [0.5, 0.5]),
            [-80.0, 0.0, 800.0, 400.0]
        );
        assert_eq!(
            fitted(viewport, source, 1.0, [0.0, 0.0]),
            [0.0, 0.0, 800.0, 400.0]
        );
        assert_eq!(
            fitted(viewport, source, 1.0, [1.0, 1.0]),
            [-160.0, 0.0, 800.0, 400.0]
        );
        // Focal coordinates clamp to [0, 1].
        assert_eq!(
            fitted(viewport, source, 1.0, [7.0, -3.0]),
            [-160.0, 0.0, 800.0, 400.0]
        );
        assert_eq!(
            fitted(viewport, source, 2.0, [0.5, 0.5]),
            [-480.0, -200.0, 1600.0, 800.0]
        );
        // FID-RND-004-3: the viewport is offset.
        assert_eq!(
            fitted(bounds(80.0, 40.0, 640.0, 400.0), source, 1.0, [0.5, 0.5]),
            [0.0, 40.0, 800.0, 400.0]
        );
    }

    #[test]
    fn cover_fit_rejects_invalid_geometry() {
        let viewport = bounds(0.0, 0.0, 640.0, 400.0);
        let src = size(800.0, 400.0);
        let focal = point(0.5, 0.5);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(cover_fit(viewport, src, bad, focal).is_none(), "zoom {bad}");
            assert!(cover_fit(viewport, size(bad, 400.0), 1.0, focal).is_none());
            assert!(cover_fit(viewport, size(800.0, bad), 1.0, focal).is_none());
            assert!(cover_fit(bounds(0.0, 0.0, bad, 400.0), src, 1.0, focal).is_none());
            assert!(cover_fit(bounds(0.0, 0.0, 640.0, bad), src, 1.0, focal).is_none());
        }
        assert!(cover_fit(viewport, src, 1.0, point(f32::NAN, 0.5)).is_none());
    }

    #[test]
    fn pan_moves_focal_by_negative_delta_over_overflow() {
        let viewport = size(px(640.0), px(400.0));
        let fitted = size(px(800.0), px(400.0));
        let moved = pan_focal(
            point(0.5, 0.3),
            point(px(-16.0), px(50.0)),
            viewport,
            fitted,
        );
        // overflow.x = 160: dragging left by 16 moves the focal by +0.1.
        assert!((moved.x - 0.6).abs() < 1e-6);
        // No vertical overflow: the axis keeps its focal.
        assert_eq!(moved.y, 0.3);
        let clamped = pan_focal(
            point(0.9, 0.0),
            point(px(-160.0), px(0.0)),
            viewport,
            fitted,
        );
        assert_eq!(clamped.x, 1.0);
        let clamped = pan_focal(point(0.1, 0.0), point(px(160.0), px(0.0)), viewport, fitted);
        assert_eq!(clamped.x, 0.0);
    }

    #[test]
    fn anchor_zoom_keeps_the_source_point_under_the_pointer() {
        let viewport = bounds(20.0, 30.0, 640.0, 400.0);
        let source = size(800.0, 600.0);
        let focal = point(0.4, 0.6);
        let pointer = point(px(300.0), px(200.0));
        let (zoom, new_focal) = anchor_zoom(viewport, source, 1.5, focal, 2.5, pointer).unwrap();
        assert_eq!(zoom, 2.5);
        let before = cover_fit(viewport, source, 1.5, focal).unwrap();
        let after = cover_fit(viewport, source, 2.5, new_focal).unwrap();
        let k_before = before.size.width.0 / source.width;
        let k_after = after.size.width.0 / source.width;
        for (pointer, before, after) in [
            (pointer.x.0, before.origin.x.0, after.origin.x.0),
            (pointer.y.0, before.origin.y.0, after.origin.y.0),
        ] {
            let source_before = (pointer - before) / k_before;
            let source_after = (pointer - after) / k_after;
            assert!(
                (source_before - source_after).abs() < 1e-2,
                "{source_before} vs {source_after}"
            );
        }
        assert!((0.0..=1.0).contains(&new_focal.x) && (0.0..=1.0).contains(&new_focal.y));
    }

    #[test]
    fn anchor_zoom_clamps_at_edges_and_keeps_axes_without_overflow() {
        let viewport = bounds(0.0, 0.0, 640.0, 400.0);
        let source = size(800.0, 400.0);
        // Zooming from 2 to 1 (cover scale 2 -> 1) with focal (0.5, 0.25): the fitted origin
        // is (-480, -100). A pointer on the viewport's left edge would need a focal of 1.5;
        // on the right edge, -0.5. Both clamp to [0, 1].
        let focal = point(0.5, 0.25);
        let (zoom, left) =
            anchor_zoom(viewport, source, 2.0, focal, 1.0, point(px(0.0), px(0.0))).unwrap();
        assert_eq!(zoom, 1.0);
        assert_eq!(left.x, 1.0);
        let (_, right) =
            anchor_zoom(viewport, source, 2.0, focal, 1.0, point(px(640.0), px(0.0))).unwrap();
        assert_eq!(right.x, 0.0);
        // Pointer at the viewport center: u = 400, desired origin = 320 - 400 = -80, focal 0.5.
        let (_, middle) = anchor_zoom(
            viewport,
            source,
            2.0,
            focal,
            1.0,
            point(px(320.0), px(200.0)),
        )
        .unwrap();
        assert!((middle.x - 0.5).abs() < 1e-6);
        // At zoom 1 there is no vertical overflow: the vertical focal is unchanged.
        assert_eq!(left.y, 0.25);
        assert_eq!(right.y, 0.25);
        assert!(
            anchor_zoom(
                viewport,
                source,
                1.0,
                point(0.5, 0.5),
                0.0,
                point(px(0.0), px(0.0))
            )
            .is_none()
        );
    }
}

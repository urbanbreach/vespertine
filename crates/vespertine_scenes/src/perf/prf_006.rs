//! PERF-PRF-006: 20 sparklines, 160 mini-donut sectors and a large donut, all rebuilt through the
//! path builder on every frame inside a scrolling block (spec PRF-006, performance workload).
//!
//! Every content coordinate is snapped to 1/256 logical px, so adding the (integral) content
//! position is exact in `f32` and the origin-relative commands of a moved shape are bit-identical,
//! as the spec's "relative geometry never changes" requires.

use crate::common::{INK, SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, FillOptions, FillRule, Hsla, IntoElement, LineCap, LineJoin, ParentElement,
    Path, PathBuilder, PathStyle, Pixels, Point, StrokeOptions, Styled, Window, canvas, div, hsla,
    point, px,
};

const ROWS: usize = 20;
const POINTS: usize = 32;
const CONTENT_HEIGHT: f32 = 640.0;
const SPARK_COLOR: u32 = 0x2563eb;

struct Prf006 {
    /// `v(r, p)`: the value of every sparkline point as of the last applied event.
    values: [[f32; POINTS]; ROWS],
    /// Number of events applied (`n` of the last one).
    events: u64,
    /// Mini-donut sector outlines relative to the donut center.
    mini: Arc<Vec<[(f32, f32); 8]>>,
    /// Large-donut sector outlines relative to the donut center.
    large: Arc<Vec<[(f32, f32); 8]>>,
}

/// Snaps to a multiple of 1/256.
fn snap(value: f32) -> f32 {
    (value * 256.0).round() / 256.0
}

/// The eight points of an annular sector (outer start, outer controls, outer end, inner end,
/// inner controls, inner start) relative to the center; angles in degrees, clockwise.
fn sector(outer: f32, inner: f32, from: f32, to: f32) -> [(f32, f32); 8] {
    let (a0, a1) = (from.to_radians(), to.to_radians());
    let span = a1 - a0;
    let on = |radius: f32, angle: f32| (radius * angle.cos(), radius * angle.sin());
    let tangent = |angle: f32| (-angle.sin(), angle.cos());
    let along =
        |p: (f32, f32), t: (f32, f32), distance: f32| (p.0 + t.0 * distance, p.1 + t.1 * distance);
    let control = |radius: f32| 4.0 / 3.0 * (span / 4.0).tan() * radius;
    let (o0, o1) = (on(outer, a0), on(outer, a1));
    let (i0, i1) = (on(inner, a0), on(inner, a1));
    let (d_out, d_in) = (control(outer), control(inner));
    let points = [
        o0,
        along(o0, tangent(a0), d_out),
        along(o1, tangent(a1), -d_out),
        o1,
        i1,
        along(i1, tangent(a1), -d_in),
        along(i0, tangent(a0), d_in),
        i0,
    ];
    points.map(|(x, y)| (snap(x), snap(y)))
}

impl Prf006 {
    fn new() -> Self {
        let mut values = [[0.0; POINTS]; ROWS];
        for (r, row) in values.iter_mut().enumerate() {
            for (p, value) in row.iter_mut().enumerate() {
                *value = 0.5 + 0.4 * (0.35 * p as f32 + 0.9 * r as f32).sin();
            }
        }
        Self {
            values,
            events: 0,
            mini: Arc::new(
                (0..8)
                    .map(|k| sector(12.0, 7.0, 45.0 * k as f32 + 2.0, 45.0 * k as f32 + 43.0))
                    .collect(),
            ),
            large: Arc::new(
                (0..12)
                    .map(|k| sector(90.0, 56.0, 30.0 * k as f32 + 1.5, 30.0 * k as f32 + 28.5))
                    .collect(),
            ),
        }
    }

    /// Applies every event up to frame `f`.
    fn update(&mut self, f: u64) {
        let target = f / 50;
        while self.events < target {
            self.events += 1;
            let n = self.events;
            let (r, p) = (((7 * n) % 20) as usize, ((11 * n) % 32) as usize);
            self.values[r][p] = 0.15 + 0.7 * ((37 * n) % 100) as f32 / 100.0;
        }
    }
}

/// Scroll offset `o(f)`: a triangle wave of period 320 and amplitude 160.
fn scroll_offset(f: u64) -> f32 {
    let phase = f % 320;
    (if phase <= 160 { phase } else { 320 - phase }) as f32
}

fn spark_color(alpha: f32) -> Hsla {
    let mut color = hex(SPARK_COLOR);
    color.a = alpha;
    color
}

fn sector_path(center: Point<Pixels>, outline: &[(f32, f32); 8]) -> Option<Path<Pixels>> {
    let at = |(x, y): (f32, f32)| center + point(px(x), px(y));
    let mut builder = PathBuilder::fill();
    builder.move_to(at(outline[0]));
    builder.cubic_bezier_to(at(outline[3]), at(outline[1]), at(outline[2]));
    builder.line_to(at(outline[4]));
    builder.cubic_bezier_to(at(outline[7]), at(outline[5]), at(outline[6]));
    builder.close();
    builder.build().ok()
}

fn sparkline_points(values: &[f32; POINTS], row: usize) -> [(f32, f32); POINTS] {
    let mut points = [(0.0, 0.0); POINTS];
    for (p, point) in points.iter_mut().enumerate() {
        let x = 120.0 + p as f32 * 300.0 / 31.0;
        let y = 32.0 * row as f32 + 4.0 + 24.0 * (1.0 - values[p]);
        *point = (snap(x), snap(y));
    }
    points
}

/// Builds every path of the frame through the path builder and hands it to `emit` with its
/// fill color; `content` is the position of the content block.
fn build_shapes(scene: &Prf006, content: Point<Pixels>, emit: &mut impl FnMut(Path<Pixels>, Hsla)) {
    let at = |x: f32, y: f32| content + point(px(x), px(y));

    for (r, row) in scene.values.iter().enumerate() {
        let points = sparkline_points(row, r);

        let mut stroke = PathBuilder::stroke(px(1.5)).with_style(PathStyle::Stroke(
            StrokeOptions::default()
                .with_line_width(1.5)
                .with_line_cap(LineCap::Round)
                .with_line_join(LineJoin::Round)
                .with_miter_limit(4.0),
        ));
        for (i, &(x, y)) in points.iter().enumerate() {
            if i == 0 {
                stroke.move_to(at(x, y));
            } else {
                stroke.line_to(at(x, y));
            }
        }
        if let Ok(path) = stroke.build() {
            emit(path, hex(SPARK_COLOR));
        }

        let base = snap(32.0 * r as f32 + 28.0);
        let mut area = PathBuilder::fill().with_style(PathStyle::Fill(
            FillOptions::default().with_fill_rule(FillRule::NonZero),
        ));
        for (i, &(x, y)) in points.iter().enumerate() {
            if i == 0 {
                area.move_to(at(x, y));
            } else {
                area.line_to(at(x, y));
            }
        }
        area.line_to(at(points[POINTS - 1].0, base));
        area.line_to(at(points[0].0, base));
        area.close();
        if let Ok(path) = area.build() {
            emit(path, spark_color(0x33 as f32 / 255.0));
        }

        let center = at(480.0, 32.0 * r as f32 + 16.0);
        for (k, outline) in scene.mini.iter().enumerate() {
            if let Some(path) = sector_path(center, outline) {
                emit(path, hsla((45 * k) as f32 / 360.0, 0.6, 0.55, 1.0));
            }
        }
    }

    let center = at(720.0, 160.0);
    for (k, outline) in scene.large.iter().enumerate() {
        if let Some(path) = sector_path(center, outline) {
            emit(path, hsla((30 * k) as f32 / 360.0, 0.6, 0.55, 1.0));
        }
    }
}

impl Scene for Prf006 {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.update(f);
        let offset = scroll_offset(f);
        let frame_scene = Prf006 {
            values: self.values,
            events: self.events,
            mini: Arc::clone(&self.mini),
            large: Arc::clone(&self.large),
        };

        let shapes = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                build_shapes(&frame_scene, bounds.origin, &mut |path, color| {
                    window.paint_path(path, color)
                });
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(880.0))
        .h(px(CONTENT_HEIGHT));

        let mut content = div()
            .absolute()
            .left(px(0.0))
            .top(px(-offset))
            .w(px(880.0))
            .h(px(CONTENT_HEIGHT))
            .child(shapes);
        for r in 0..ROWS {
            content = content.child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .top(px(32.0 * r as f32))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .child(format!("Series {r}")),
            );
        }

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(content)
            .into_any_element()
    }
}

register_scene!("PERF-PRF-006", Prf006::new);

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{path_cache_stats, reset_path_cache};

    /// Replays the workload's path building (no window) and checks the spec's expected counts:
    /// 60 distinct shapes on the first frame, then none except the two shapes of the changed row on
    /// every 50th frame.
    #[test]
    fn perf_prf_006_tessellates_each_shape_once_and_only_changed_rows_again() {
        reset_path_cache();
        let mut scene = Prf006::new();
        let mut paths_per_frame = 0;
        for f in 0..720u64 {
            scene.update(f);
            let before = path_cache_stats();
            let mut count = 0;
            build_shapes(
                &scene,
                point(px(0.0), px(-scroll_offset(f))),
                &mut |_, _| count += 1,
            );
            let after = path_cache_stats();
            paths_per_frame = count;
            let tessellated = after.tessellations - before.tessellations;
            let expected = match f {
                0 => 20 * 2 + 8 + 12,
                f if f % 50 == 0 => 2,
                _ => 0,
            };
            assert_eq!(tessellated, expected, "frame {f}");
            assert_eq!(after.hits - before.hits, count - expected, "frame {f}");
        }
        assert_eq!(paths_per_frame, 20 * 2 + 160 + 12);
        let stats = path_cache_stats();
        assert_eq!(stats.recent_shapes + stats.older_shapes, 60 + 14 * 2);
        assert!(stats.retained_bytes > 0);
    }
}

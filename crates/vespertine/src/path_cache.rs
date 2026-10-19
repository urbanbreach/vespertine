//! Translation-invariant tessellation cache (PRF-006).
//!
//! A built path is reduced to its shape: the transformed commands expressed relative to the
//! path's first point. The tessellator is always fed that origin-relative shape, its triangles
//! are retained by shape and by every option that influences them, and the output vertices are
//! translated by the origin. A shape that is merely moved is therefore tessellated once.
//!
//! Storage is thread-local and two-generation: at most [`GENERATION_CAPACITY`] shapes are
//! recent, and the shapes that were recent before the last rotation are older.

use std::{cell::RefCell, sync::Arc};

use anyhow::Error;
use lyon::{
    math::Point as LyonPoint,
    path::{Event, Path as LyonPath},
    tessellation::{
        BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
        StrokeVertex, VertexBuffers,
    },
};
use vespertine_collections::FxHashMap;

use crate::{Path, PathVertex, Pixels, Point, point, px};

/// Shapes held by one generation; the cache retains at most twice this many.
pub(crate) const GENERATION_CAPACITY: usize = 1_024;

/// The style inputs that influence the triangles of a path.
pub(crate) enum TessellationStyle<'a> {
    Fill(&'a FillOptions),
    Stroke {
        options: &'a StrokeOptions,
        dash_array: Option<&'a [Pixels]>,
    },
}

/// One tessellated vertex in the origin-relative coordinate convention.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalVertex {
    xy: [f32; 2],
    st: [f32; 2],
}

/// The triangles of one shape, positions relative to the shape's origin.
#[derive(Debug)]
struct Shape {
    /// Position of the first tessellator vertex (the path's start point), origin relative.
    start: [f32; 2],
    vertices: Box<[LocalVertex]>,
}

impl Shape {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(&*self.vertices)
    }
}

/// Counters of the calling thread's cache.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathCacheStats {
    /// Tessellator runs, for cached and bypassing paths.
    pub tessellations: u64,
    /// Builds served from either generation.
    pub hits: u64,
    /// Shapes in the recent generation.
    pub recent_shapes: usize,
    /// Shapes in the older generation.
    pub older_shapes: usize,
    /// Bytes of the retained triangle data of both generations.
    pub retained_bytes: usize,
}

type Generation = FxHashMap<Box<[u32]>, Arc<Shape>>;

#[derive(Default)]
struct PathCache {
    recent: Generation,
    older: Generation,
    recent_bytes: usize,
    older_bytes: usize,
    tessellations: u64,
    hits: u64,
    /// Key words of the shape being looked up; reused to keep hits allocation free.
    scratch: Vec<u32>,
}

impl PathCache {
    fn lookup(&mut self, key: &[u32]) -> Option<Arc<Shape>> {
        if let Some(shape) = self.recent.get(key) {
            return Some(shape.clone());
        }
        let (key, shape) = self.older.remove_entry(key)?;
        self.older_bytes -= shape.bytes();
        self.insert(key, shape.clone());
        Some(shape)
    }

    /// Inserts into the recent generation, rotating first when it is full.
    fn insert(&mut self, key: Box<[u32]>, shape: Arc<Shape>) {
        if self.recent.len() >= GENERATION_CAPACITY {
            self.older = std::mem::take(&mut self.recent);
            self.older_bytes = std::mem::take(&mut self.recent_bytes);
        }
        self.recent_bytes += shape.bytes();
        self.recent.insert(key, shape);
    }
}

thread_local! {
    static CACHE: RefCell<PathCache> = RefCell::new(PathCache::default());
}

/// Counters of the calling thread's cache.
#[cfg(any(test, feature = "test-support"))]
pub fn path_cache_stats() -> PathCacheStats {
    CACHE.with_borrow(|cache| PathCacheStats {
        tessellations: cache.tessellations,
        hits: cache.hits,
        recent_shapes: cache.recent.len(),
        older_shapes: cache.older.len(),
        retained_bytes: cache.recent_bytes + cache.older_bytes,
    })
}

/// Empties the calling thread's cache and zeroes its counters.
#[cfg(any(test, feature = "test-support"))]
pub fn reset_path_cache() {
    CACHE.with_borrow_mut(|cache| *cache = PathCache::default());
}

fn push_relative(key: &mut Vec<u32>, p: LyonPoint, origin: LyonPoint) {
    key.push((p.x - origin.x).to_bits());
    key.push((p.y - origin.y).to_bits());
}

/// Builds the key of `path` under `style` into `key`; returns the origin (the first point of
/// the path), or `None` when the path has no commands.
fn write_key(path: &LyonPath, style: &TessellationStyle, key: &mut Vec<u32>) -> Option<LyonPoint> {
    key.clear();
    match style {
        TessellationStyle::Fill(options) => {
            key.extend([
                0,
                options.tolerance.to_bits(),
                options.fill_rule as u32,
                options.sweep_orientation as u32,
                options.handle_intersections as u32,
            ]);
        }
        TessellationStyle::Stroke {
            options,
            dash_array,
        } => {
            let variable = options.variable_line_width;
            key.extend([
                1,
                options.start_cap as u32,
                options.end_cap as u32,
                options.line_join as u32,
                options.line_width.to_bits(),
                variable.is_some() as u32,
                variable.unwrap_or_default() as u32,
                options.miter_limit.to_bits(),
                options.tolerance.to_bits(),
            ]);
            match dash_array {
                None => key.push(0),
                Some(dashes) => {
                    key.push(1);
                    key.push(dashes.len() as u32);
                    key.extend(dashes.iter().map(|dash| dash.0.to_bits()));
                }
            }
        }
    }

    let mut origin = None;
    for event in path.iter() {
        let origin = *origin.get_or_insert_with(|| match event {
            Event::Begin { at } => at,
            // A well-formed path always begins with `Begin`.
            Event::Line { from, .. }
            | Event::Quadratic { from, .. }
            | Event::Cubic { from, .. } => from,
            Event::End { last, .. } => last,
        });
        match event {
            Event::Begin { at } => {
                key.push(1);
                push_relative(key, at, origin);
            }
            Event::Line { to, .. } => {
                key.push(2);
                push_relative(key, to, origin);
            }
            Event::Quadratic { ctrl, to, .. } => {
                key.push(3);
                push_relative(key, ctrl, origin);
                push_relative(key, to, origin);
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                key.push(4);
                push_relative(key, ctrl1, origin);
                push_relative(key, ctrl2, origin);
                push_relative(key, to, origin);
            }
            Event::End { close, .. } => key.push(if close { 6 } else { 5 }),
        }
    }
    origin
}

/// The path with every coordinate expressed relative to `origin`.
fn normalized(path: &LyonPath, origin: LyonPoint) -> LyonPath {
    let relative = |p: LyonPoint| lyon::math::point(p.x - origin.x, p.y - origin.y);
    let mut builder = LyonPath::builder();
    for event in path.iter() {
        match event {
            Event::Begin { at } => {
                builder.begin(relative(at));
            }
            Event::Line { to, .. } => {
                builder.line_to(relative(to));
            }
            Event::Quadratic { ctrl, to, .. } => {
                builder.quadratic_bezier_to(relative(ctrl), relative(to));
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                builder.cubic_bezier_to(relative(ctrl1), relative(ctrl2), relative(to));
            }
            Event::End { close, .. } => builder.end(close),
        }
    }
    builder.build()
}

/// Runs the tessellator over an origin-relative path and returns its triangles.
fn tessellate(path: &LyonPath, style: &TessellationStyle) -> Result<Shape, Error> {
    let mut buffers: VertexBuffers<LyonPoint, u16> = VertexBuffers::new();
    match style {
        TessellationStyle::Fill(options) => {
            FillTessellator::new().tessellate_path(
                path,
                options,
                &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position()),
            )?;
        }
        TessellationStyle::Stroke {
            options,
            dash_array,
        } => {
            let dashed;
            let path = match dash_array {
                Some(dash_array) => {
                    dashed = dash(path, dash_array);
                    &dashed
                }
                None => path,
            };
            StrokeTessellator::new().tessellate_path(
                path,
                options,
                &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| vertex.position()),
            )?;
        }
    }

    let Some(first) = buffers.vertices.first() else {
        return Ok(Shape {
            start: [0.0; 2],
            vertices: Box::default(),
        });
    };
    let start = [first.x, first.y];
    let vertices = buffers
        .indices
        .chunks_exact(3)
        .flatten()
        .map(|&index| {
            let vertex = buffers.vertices[index as usize];
            LocalVertex {
                xy: [vertex.x, vertex.y],
                // The tessellator's triangles carry no curve coordinates of their own.
                st: [0.0, 1.0],
            }
        })
        .collect();
    Ok(Shape { start, vertices })
}

/// Splits `path` into the "on" intervals of the dash pattern.
fn dash(path: &LyonPath, dash_array: &[Pixels]) -> LyonPath {
    let measurements = lyon::algorithms::measure::PathMeasurements::from_path(path, 0.01);
    let mut sampler =
        measurements.create_sampler(path, lyon::algorithms::measure::SampleType::Normalized);
    let mut builder = LyonPath::builder();

    let total_length = sampler.length();
    let dash_array_len = dash_array.len();
    let mut pos = 0.;
    let mut dash_index = 0;
    while pos < total_length {
        let dash_length = dash_array[dash_index % dash_array_len].0;
        let next_pos = (pos + dash_length).min(total_length);
        if dash_index % 2 == 0 {
            let start = pos / total_length;
            let end = next_pos / total_length;
            sampler.split_range(start..end, &mut builder);
        }
        pos = next_pos;
        dash_index += 1;
    }
    builder.build()
}

/// Builds the output path of `shape` at `origin`: positions translated, curve coordinates copied.
fn instantiate(shape: &Shape, origin: LyonPoint) -> Path<Pixels> {
    if shape.vertices.is_empty() {
        return Path::new(Point::default());
    }
    let translate = |xy: [f32; 2]| point(px(xy[0] + origin.x), px(xy[1] + origin.y));
    let vertices = shape
        .vertices
        .iter()
        .map(|vertex| PathVertex {
            xy_position: translate(vertex.xy),
            st_position: point(vertex.st[0], vertex.st[1]),
            content_mask: Default::default(),
        })
        .collect();
    Path::from_vertices(translate(shape.start), vertices)
}

/// Tessellates `path` (already transformed) under `style` through the cache.
pub(crate) fn build(path: &LyonPath, style: TessellationStyle) -> Result<Path<Pixels>, Error> {
    let mut key = CACHE.with_borrow_mut(|cache| std::mem::take(&mut cache.scratch));
    let origin = write_key(path, &style, &mut key);

    let Some(origin) = origin else {
        // Nothing to key: tessellate in the same convention without retaining anything.
        CACHE.with_borrow_mut(|cache| {
            cache.scratch = key;
            cache.tessellations += 1;
        });
        return tessellate(path, &style).map(|shape| instantiate(&shape, LyonPoint::zero()));
    };

    let cached = CACHE.with_borrow_mut(|cache| {
        let shape = cache.lookup(&key);
        cache.hits += shape.is_some() as u64;
        shape
    });
    let result = match cached {
        Some(shape) => Ok(instantiate(&shape, origin)),
        None => {
            CACHE.with_borrow_mut(|cache| cache.tessellations += 1);
            tessellate(&normalized(path, origin), &style).map(|shape| {
                let shape = Arc::new(shape);
                let built = instantiate(&shape, origin);
                let owned_key: Box<[u32]> = key.as_slice().into();
                CACHE.with_borrow_mut(|cache| cache.insert(owned_key, shape));
                built
            })
        }
    };
    CACHE.with_borrow_mut(|cache| cache.scratch = key);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillRule, LineCap, LineJoin, PathBuilder, PathStyle};

    type Bits = Vec<[u32; 4]>;

    /// Positions, curve coordinates and bounds as exact bit patterns.
    fn bits(path: &Path<Pixels>) -> (Bits, [u32; 4]) {
        let vertices = path
            .vertices
            .iter()
            .map(|v| {
                [
                    v.xy_position.x.0.to_bits(),
                    v.xy_position.y.0.to_bits(),
                    v.st_position.x.to_bits(),
                    v.st_position.y.to_bits(),
                ]
            })
            .collect();
        let b = &path.bounds;
        (
            vertices,
            [
                b.origin.x.0.to_bits(),
                b.origin.y.0.to_bits(),
                b.size.width.0.to_bits(),
                b.size.height.0.to_bits(),
            ],
        )
    }

    fn at(origin: (f32, f32), x: f32, y: f32) -> Point<Pixels> {
        point(px(origin.0 + x), px(origin.1 + y))
    }

    /// An S-shaped curve: dyadic coordinates keep the origin-relative commands bit-identical
    /// at the origins used by the tests.
    fn curve(width: f32, origin: (f32, f32)) -> PathBuilder {
        let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(
            StrokeOptions::default()
                .with_line_width(width)
                .with_line_cap(LineCap::Round)
                .with_line_join(LineJoin::Round),
        ));
        builder.move_to(at(origin, 0.0, 0.0));
        builder.curve_to(at(origin, 20.0, 10.0), at(origin, 10.0, -10.0));
        builder.cubic_bezier_to(
            at(origin, 40.0, 0.0),
            at(origin, 25.0, 20.0),
            at(origin, 35.0, -20.0),
        );
        builder.line_to(at(origin, 50.0, 12.5));
        builder
    }

    fn blob(origin: (f32, f32)) -> PathBuilder {
        let mut builder = PathBuilder::fill();
        builder.move_to(at(origin, 0.0, 0.0));
        builder.line_to(at(origin, 30.0, 5.0));
        builder.curve_to(at(origin, 40.0, 30.0), at(origin, 45.0, 10.0));
        builder.cubic_bezier_to(
            at(origin, 5.0, 40.0),
            at(origin, 30.0, 45.0),
            at(origin, 10.0, 50.0),
        );
        builder.close();
        builder
    }

    const ORIGINS: [(f32, f32); 5] = [
        (12.0, 7.0),
        (-37.25, -4.5),
        (0.5, 0.25),
        (65536.0, -131072.0),
        (3.0, 1024.5),
    ];

    fn fresh(builder: PathBuilder) -> Path<Pixels> {
        reset_path_cache();
        builder.build().expect("builds")
    }

    #[test]
    fn translated_shapes_tessellate_once_and_match_fresh_builds() {
        for shape in [
            (|origin| curve(2.5, origin)) as fn((f32, f32)) -> PathBuilder,
            blob,
        ] {
            let expected: Vec<_> = ORIGINS.iter().map(|&o| bits(&fresh(shape(o)))).collect();

            reset_path_cache();
            for (origin, expected) in ORIGINS.iter().zip(&expected) {
                let built = shape(*origin).build().expect("builds");
                assert_eq!(&bits(&built), expected, "origin {origin:?}");
            }
            let stats = path_cache_stats();
            assert_eq!(stats.tessellations, 1);
            assert_eq!(stats.hits, ORIGINS.len() as u64 - 1);
            assert_eq!(stats.recent_shapes, 1);
            assert!(stats.retained_bytes > 0);
        }
    }

    #[test]
    fn hits_keep_curve_coordinates_and_bound_the_translated_triangles() {
        reset_path_cache();
        let first = curve(3.0, (0.0, 0.0)).build().unwrap();
        let moved = curve(3.0, (100.0, 50.0)).build().unwrap();
        assert_eq!(path_cache_stats().hits, 1);
        assert_eq!(first.vertices.len(), moved.vertices.len());
        assert!(!moved.vertices.is_empty());
        for (a, b) in first.vertices.iter().zip(&moved.vertices) {
            assert_eq!(a.st_position, b.st_position);
            assert_eq!(a.xy_position.x.0 + 100.0, b.xy_position.x.0);
            assert_eq!(a.xy_position.y.0 + 50.0, b.xy_position.y.0);
        }
        let (min_x, max_x) = moved
            .vertices
            .iter()
            .map(|v| v.xy_position.x.0)
            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        assert_eq!(
            moved.bounds.origin.x.0,
            min_x.min(moved.vertices[0].xy_position.x.0)
        );
        assert_eq!(moved.bounds.origin.x.0 + moved.bounds.size.width.0, max_x);
    }

    /// Builds `variant` after warming the cache with `base`; it must miss and equal a fresh build.
    fn assert_variant_misses(name: &str, base: PathBuilder, variant: impl Fn() -> PathBuilder) {
        reset_path_cache();
        base.build().expect("base builds");
        let before = path_cache_stats();
        let built = variant().build().expect("variant builds");
        let after = path_cache_stats();
        assert_eq!(
            after.tessellations,
            before.tessellations + 1,
            "{name}: miss"
        );
        assert_eq!(after.hits, before.hits, "{name}: no hit");
        assert_eq!(
            bits(&built),
            bits(&fresh(variant())),
            "{name}: matches fresh"
        );
    }

    fn stroke_with(options: impl Fn(StrokeOptions) -> StrokeOptions) -> PathBuilder {
        let mut builder = curve(2.0, (5.0, 5.0));
        let PathStyle::Stroke(base) = &builder.style else {
            unreachable!()
        };
        builder.style = PathStyle::Stroke(options(*base));
        builder
    }

    fn fill_with(options: impl Fn(FillOptions) -> FillOptions) -> PathBuilder {
        let mut builder = blob((5.0, 5.0));
        builder.style = PathStyle::Fill(options(FillOptions::default()));
        builder
    }

    #[test]
    fn every_triangle_input_changes_the_key() {
        let base = || stroke_with(|o| o);
        assert_variant_misses("width", base(), || stroke_with(|o| o.with_line_width(3.0)));
        assert_variant_misses("start cap", base(), || {
            stroke_with(|o| o.with_start_cap(LineCap::Square))
        });
        assert_variant_misses("end cap", base(), || {
            stroke_with(|o| o.with_end_cap(LineCap::Butt))
        });
        assert_variant_misses("join", base(), || {
            stroke_with(|o| o.with_line_join(LineJoin::Bevel))
        });
        assert_variant_misses("miter limit", base(), || {
            stroke_with(|o| o.with_miter_limit(2.0))
        });
        assert_variant_misses("stroke tolerance", base(), || {
            stroke_with(|o| o.with_tolerance(0.5))
        });
        assert_variant_misses("dash", base(), || {
            stroke_with(|o| o).dash_array(&[px(4.0), px(2.0)])
        });
        assert_variant_misses(
            "dash length",
            stroke_with(|o| o).dash_array(&[px(4.0), px(2.0)]),
            || stroke_with(|o| o).dash_array(&[px(4.0), px(2.5)]),
        );
        assert_variant_misses(
            "dash gap",
            stroke_with(|o| o).dash_array(&[px(4.0), px(2.0)]),
            || stroke_with(|o| o).dash_array(&[px(4.0), px(3.0)]),
        );

        let base = || fill_with(|o| o);
        assert_variant_misses("fill rule", base(), || {
            fill_with(|o| o.with_fill_rule(FillRule::NonZero))
        });
        assert_variant_misses("fill tolerance", base(), || {
            fill_with(|o| o.with_tolerance(0.5))
        });
        assert_variant_misses("fill vs stroke", base(), || {
            let mut builder = blob((5.0, 5.0));
            builder.style = PathStyle::Stroke(StrokeOptions::default());
            builder
        });

        let moved_control = |dy: f32| {
            let mut builder = PathBuilder::fill();
            builder.move_to(at((5.0, 5.0), 0.0, 0.0));
            builder.curve_to(at((5.0, 5.0), 20.0, 0.0), at((5.0, 5.0), 10.0, dy));
            builder.close();
            builder
        };
        assert_variant_misses("control point", moved_control(-10.0), || {
            moved_control(-10.5)
        });

        let transformed = |scale: f32, rotation: f32| {
            let mut builder = blob((5.0, 5.0));
            builder.scale(scale);
            builder.rotate(rotation);
            builder
        };
        assert_variant_misses("scale", transformed(1.0, 10.0), || transformed(1.25, 10.0));
        assert_variant_misses("rotation", transformed(1.0, 10.0), || {
            transformed(1.0, 11.0)
        });
    }

    #[test]
    fn identical_dash_patterns_share_a_shape() {
        reset_path_cache();
        stroke_with(|o| o).dash_array(&[px(4.0)]).build().unwrap();
        stroke_with(|o| o)
            .dash_array(&[px(4.0), px(4.0)])
            .build()
            .unwrap();
        assert_eq!(path_cache_stats().tessellations, 1);
    }

    #[test]
    fn transforms_apply_to_the_commands_before_the_shape_is_derived() {
        reset_path_cache();
        let mut scaled = PathBuilder::fill();
        scaled.move_to(point(px(1.0), px(2.0)));
        scaled.line_to(point(px(5.0), px(2.0)));
        scaled.line_to(point(px(3.0), px(6.0)));
        scaled.close();
        scaled.scale(2.0);
        scaled.translate(point(px(8.0), px(-4.0)));
        let scaled = scaled.build().unwrap();

        let mut direct = PathBuilder::fill();
        direct.move_to(point(px(10.0), px(0.0)));
        direct.line_to(point(px(18.0), px(0.0)));
        direct.line_to(point(px(14.0), px(8.0)));
        direct.close();
        let direct = direct.build().unwrap();

        let stats = path_cache_stats();
        assert_eq!((stats.tessellations, stats.hits), (1, 1));
        assert_eq!(bits(&scaled), bits(&direct));
    }

    fn triangle_shape(i: usize) -> PathBuilder {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(px(0.0), px(0.0)));
        builder.line_to(point(px(10.0), px(0.0)));
        builder.line_to(point(px(0.0), px(1.0 + i as f32)));
        builder.close();
        builder
    }

    fn misses(builder: PathBuilder) -> bool {
        let before = path_cache_stats().tessellations;
        builder.build().unwrap();
        path_cache_stats().tessellations > before
    }

    #[test]
    fn two_generations_rotate_promote_and_bound_retention() {
        reset_path_cache();
        for i in 0..GENERATION_CAPACITY {
            triangle_shape(i).build().unwrap();
        }
        let full = path_cache_stats();
        assert_eq!(
            (full.recent_shapes, full.older_shapes),
            (GENERATION_CAPACITY, 0)
        );

        // The next insertion rotates: the 1,024 shapes become older, the new shape is recent.
        triangle_shape(GENERATION_CAPACITY).build().unwrap();
        let rotated = path_cache_stats();
        assert_eq!(
            (rotated.recent_shapes, rotated.older_shapes),
            (1, GENERATION_CAPACITY)
        );

        // An older hit promotes without tessellating and leaves the older generation.
        assert!(!misses(triangle_shape(5)));
        let promoted = path_cache_stats();
        assert_eq!(
            (promoted.recent_shapes, promoted.older_shapes),
            (2, GENERATION_CAPACITY - 1)
        );

        // Fill the recent generation, then force a second rotation: shape 5 (promoted) and the
        // shape of the first rotation survive, never-used older shapes are dropped.
        for i in 0..GENERATION_CAPACITY - 2 {
            triangle_shape(2_000 + i).build().unwrap();
        }
        assert_eq!(path_cache_stats().recent_shapes, GENERATION_CAPACITY);
        triangle_shape(5_000).build().unwrap();
        let second = path_cache_stats();
        assert_eq!(
            (second.recent_shapes, second.older_shapes),
            (1, GENERATION_CAPACITY)
        );
        assert!(!misses(triangle_shape(5)), "shape 5 survived the rotation");
        assert!(
            !misses(triangle_shape(GENERATION_CAPACITY)),
            "the shape of the first rotation survived"
        );
        assert!(
            misses(triangle_shape(7)),
            "unused for two rotations: evicted"
        );
        assert!(!misses(triangle_shape(7)), "and cached again afterwards");

        // Retention never exceeds two generations however many shapes are inserted.
        let mut most = 0;
        for i in 0..5 * GENERATION_CAPACITY {
            triangle_shape(10_000 + i).build().unwrap();
            let stats = path_cache_stats();
            let total = stats.recent_shapes + stats.older_shapes;
            assert!(total <= 2 * GENERATION_CAPACITY);
            most = most.max(total);
        }
        assert_eq!(most, 2 * GENERATION_CAPACITY);
    }

    #[test]
    fn promotion_into_a_full_recent_generation_rotates() {
        reset_path_cache();
        for i in 0..GENERATION_CAPACITY {
            triangle_shape(i).build().unwrap();
        }
        triangle_shape(GENERATION_CAPACITY).build().unwrap(); // rotation: older = 0..1023
        for i in 0..GENERATION_CAPACITY - 1 {
            triangle_shape(3_000 + i).build().unwrap(); // recent is now full
        }
        let full = path_cache_stats();
        assert_eq!(
            (full.recent_shapes, full.older_shapes),
            (GENERATION_CAPACITY, GENERATION_CAPACITY)
        );
        // Promoting shape 9 needs room: rotate, dropping the old older generation.
        assert!(!misses(triangle_shape(9)));
        let promoted = path_cache_stats();
        assert_eq!(
            (promoted.recent_shapes, promoted.older_shapes),
            (1, GENERATION_CAPACITY)
        );
    }

    #[test]
    fn empty_and_degenerate_paths_match_the_uncached_result_and_are_not_retained() {
        reset_path_cache();
        let empty = PathBuilder::fill().build().unwrap();
        assert!(empty.vertices.is_empty());
        assert_eq!(bits(&empty), bits(&Path::new(Point::default())));

        let mut lone_move = PathBuilder::stroke(px(2.0));
        lone_move.move_to(point(px(3.0), px(4.0)));
        let lone_move = lone_move.build().unwrap();
        assert!(lone_move.vertices.is_empty());
        assert_eq!(bits(&lone_move), bits(&Path::new(Point::default())));

        let degenerate = || {
            let mut builder = PathBuilder::fill();
            builder.move_to(point(px(3.0), px(4.0)));
            builder.line_to(point(px(30.0), px(40.0)));
            builder.close();
            builder
        };
        let first = degenerate().build().unwrap();
        let second = degenerate().build().unwrap();
        assert!(first.vertices.is_empty());
        assert_eq!(bits(&first), bits(&Path::new(Point::default())));
        assert_eq!(bits(&first), bits(&second));
    }

    /// A stroke whose triangles do not fit 16-bit indices: the tessellator reports an error.
    fn too_big(prefix_only: bool) -> PathBuilder {
        let mut builder = PathBuilder::stroke(px(2.0)).with_style(PathStyle::Stroke(
            StrokeOptions::default()
                .with_line_width(2.0)
                .with_line_join(LineJoin::Round)
                .with_line_cap(LineCap::Round),
        ));
        builder.move_to(point(px(0.0), px(0.0)));
        let segments = if prefix_only { 3 } else { 40_000 };
        for i in 1..=segments {
            let x = (i % 100) as f32 * 4.0;
            builder.line_to(point(px(x), px(i as f32 * 0.5)));
        }
        builder
    }

    #[test]
    fn failures_report_the_error_and_are_not_cached() {
        reset_path_cache();
        let first = too_big(false).build().expect_err("too many vertices");
        let stats = path_cache_stats();
        assert_eq!(
            (
                stats.recent_shapes,
                stats.older_shapes,
                stats.retained_bytes
            ),
            (0, 0, 0)
        );
        let second = too_big(false).build().expect_err("fails again");
        assert_eq!(first.to_string(), second.to_string());
        let stats = path_cache_stats();
        assert_eq!((stats.tessellations, stats.hits), (2, 0));

        // A valid path sharing the failed path's first commands is unaffected.
        let valid = too_big(true).build().unwrap();
        assert_eq!(bits(&valid), bits(&fresh(too_big(true))));
        assert_eq!(path_cache_stats().recent_shapes, 1);
    }

    #[test]
    fn threads_do_not_share_cached_shapes() {
        reset_path_cache();
        curve(2.5, (1.0, 1.0)).build().unwrap();
        let in_thread = std::thread::spawn(|| {
            let built = curve(2.5, (64.0, 32.0)).build().unwrap();
            let stats = path_cache_stats();
            let expected = bits(&fresh(curve(2.5, (64.0, 32.0))));
            (bits(&built) == expected, stats)
        })
        .join()
        .unwrap();
        let (identical, stats) = in_thread;
        assert!(identical);
        assert_eq!(
            (stats.tessellations, stats.hits),
            (1, 0),
            "the thread started cold"
        );
        let main = path_cache_stats();
        assert_eq!(
            (main.tessellations, main.hits, main.recent_shapes),
            (1, 0, 1)
        );
        // The calling thread's own shape is still served from its cache.
        curve(2.5, (9.0, 9.0)).build().unwrap();
        assert_eq!(path_cache_stats().hits, 1);
    }
}

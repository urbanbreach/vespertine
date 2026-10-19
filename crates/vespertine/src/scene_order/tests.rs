use super::*;
use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, ContentMask, Hsla, MonochromeSprite, Primitive,
    Quad, Scene, TileId, TransformationMatrix, bounds_tree::BoundsTree, point, size,
};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::time::{Duration, Instant};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

/// Orders of a stream according to the upstream bounds tree (the oracle).
fn tree_orders(stream: &[Bounds<ScaledPixels>]) -> Vec<u32> {
    let mut tree = BoundsTree::<ScaledPixels>::default();
    stream.iter().map(|bounds| tree.insert(*bounds)).collect()
}

/// Orders according to the literal definition: 1 + the maximum order of every earlier
/// overlapping entry, using the upstream strict-inequality predicate.
fn brute_force_orders(stream: &[Bounds<ScaledPixels>]) -> Vec<u32> {
    let mut orders = Vec::with_capacity(stream.len());
    for (i, bounds) in stream.iter().enumerate() {
        let best = (0..i)
            .filter(|&j| bounds.intersects(&stream[j]))
            .map(|j| orders[j])
            .max()
            .unwrap_or(0);
        orders.push(best + 1);
    }
    orders
}

fn run_frame(index: &mut OrderIndex, stream: &[Bounds<ScaledPixels>]) -> Vec<u32> {
    index.begin_frame();
    stream.iter().map(|bounds| index.insert(*bounds)).collect()
}

fn raw_bounds(rng: &mut StdRng, max_scale_exponent: i32) -> Bounds<ScaledPixels> {
    match rng.random_range(0..10) {
        // Ordinary window-sized content.
        0..=3 => bounds(
            rng.random_range(-20.0..400.0),
            rng.random_range(-20.0..300.0),
            rng.random_range(1.0..160.0),
            rng.random_range(1.0..120.0),
        ),
        // Aligned to cell boundaries so edges touch exactly.
        4 => bounds(
            64.0 * rng.random_range(0..6) as f32,
            64.0 * rng.random_range(0..5) as f32,
            64.0 * rng.random_range(0..3) as f32,
            64.0 * rng.random_range(0..3) as f32,
        ),
        // Negative coordinates.
        5 => bounds(
            rng.random_range(-9000.0..-1.0),
            rng.random_range(-9000.0..50.0),
            rng.random_range(1.0..9500.0),
            rng.random_range(1.0..9500.0),
        ),
        // Very large bounds.
        6 => {
            let scale = 10f32.powi(rng.random_range(5..max_scale_exponent));
            bounds(
                rng.random_range(-1.0..1.0) * scale,
                rng.random_range(-1.0..1.0) * scale,
                rng.random_range(0.0..1.0) * scale,
                rng.random_range(0.0..1.0) * scale,
            )
        }
        // Degenerate (zero width and/or zero height).
        7 | 8 => {
            let x = rng.random_range(-10.0..300.0f32);
            let y = rng.random_range(-10.0..300.0f32);
            let x = if rng.random_bool(0.3) {
                (x / 64.0).round() * 64.0
            } else {
                x
            };
            match rng.random_range(0..3) {
                0 => bounds(x, y, 0.0, rng.random_range(0.0..200.0)),
                1 => bounds(x, y, rng.random_range(0.0..200.0), 0.0),
                _ => bounds(x, y, 0.0, 0.0),
            }
        }
        // Full-window cover.
        _ => bounds(0.0, 0.0, 1760.0, 960.0),
    }
}

/// Bounds on a quarter-pixel lattice with magnitudes up to a few million pixels. Every sum the
/// upstream bounds tree computes on them is exact in `f32`, which makes the tree a valid oracle
/// (its union bounds lose precision for unrelated magnitudes beyond that).
fn random_bounds(rng: &mut StdRng) -> Bounds<ScaledPixels> {
    let bounds = raw_bounds(rng, 7);
    let quantize = |value: f32| (value * 4.0).round() / 4.0;
    self::bounds(
        quantize(bounds.origin.x.0),
        quantize(bounds.origin.y.0),
        quantize(bounds.size.width.0),
        quantize(bounds.size.height.0),
    )
}

/// Like [`random_bounds`] but with unquantized values up to 1e29, for comparisons against the
/// literal overlap definition.
fn wild_bounds(rng: &mut StdRng) -> Bounds<ScaledPixels> {
    raw_bounds(rng, 30)
}

/// Derives the next frame's stream from the previous one: removals, replaced bounds, shifted
/// bounds, and insertions.
fn mutate(rng: &mut StdRng, stream: &[Bounds<ScaledPixels>]) -> Vec<Bounds<ScaledPixels>> {
    let mut next = stream.to_vec();
    for _ in 0..rng.random_range(0..4) {
        if !next.is_empty() {
            next.remove(rng.random_range(0..next.len()));
        }
    }
    for _ in 0..rng.random_range(0..4) {
        if !next.is_empty() {
            let at = rng.random_range(0..next.len());
            next[at] = if rng.random_bool(0.5) {
                random_bounds(rng)
            } else {
                let mut moved = next[at];
                moved.origin.x.0 += rng.random_range(-30.0..30.0);
                moved
            };
        }
    }
    for _ in 0..rng.random_range(0..4) {
        let at = rng.random_range(0..=next.len());
        next.insert(at, random_bounds(rng));
    }
    next
}

#[test]
fn orders_match_bounds_tree_oracle_over_seeded_frames() {
    let mut replayed = 0;
    let mut recomputed = 0;
    for seed in 0..400u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let count = rng.random_range(1..260);
        let mut stream: Vec<_> = (0..count).map(|_| random_bounds(&mut rng)).collect();
        let mut index = OrderIndex::default();
        let mut other = OrderIndex::default();
        for frame in 0..6 {
            let expected = tree_orders(&stream);
            let actual = if frame % 2 == 0 {
                run_frame(&mut index, &stream)
            } else {
                // Alternate with a second index that adopts the first one's stream, like two
                // scenes swapping roles between frames.
                other.begin_frame();
                other.adopt_baseline(&index);
                stream.iter().map(|b| other.insert(*b)).collect()
            };
            let brute = brute_force_orders(&stream);
            if actual != expected {
                let at = (0..actual.len())
                    .find(|&i| actual[i] != expected[i])
                    .unwrap();
                panic!(
                    "seed {seed} frame {frame} at {at} of {}: actual {} tree {} brute {} bounds {:?}",
                    actual.len(),
                    actual[at],
                    expected[at],
                    brute[at],
                    stream[at]
                );
            }
            let stats = if frame % 2 == 0 {
                index.stats()
            } else {
                other.stats()
            };
            replayed += stats.replayed_entries;
            recomputed += stats.recomputed_entries;
            if frame % 2 == 1 {
                std::mem::swap(&mut index, &mut other);
            }
            stream = mutate(&mut rng, &stream);
        }
    }
    assert!(
        replayed > 0 && recomputed > 0,
        "both paths must be exercised"
    );
}

#[test]
fn grid_orders_match_oracle_with_budget_forced_to_grid() {
    // A scene whose first frame is searched through the spatial grid (no baseline and more
    // entries than the replay budget allows scanning).
    for seed in 0..40u64 {
        let mut rng = StdRng::seed_from_u64(1000 + seed);
        let stream: Vec<_> = (0..900).map(|_| random_bounds(&mut rng)).collect();
        let mut index = OrderIndex::default();
        let actual = run_frame(&mut index, &stream);
        assert_eq!(actual, tree_orders(&stream), "seed {seed}");
        let wild: Vec<_> = (0..900).map(|_| wild_bounds(&mut rng)).collect();
        assert_eq!(
            run_frame(&mut index, &wild),
            brute_force_orders(&wild),
            "seed {seed} wild"
        );
        assert_eq!(index.stats().grid_builds, 1);
        assert!(index.stats().spatial_queries > 0);
    }
}

#[test]
fn malformed_bounds_follow_the_overlap_predicate() {
    let mut rng = StdRng::seed_from_u64(7);
    for round in 0..200 {
        let mut stream: Vec<_> = (0..rng.random_range(2..80))
            .map(|_| wild_bounds(&mut rng))
            .collect();
        for _ in 0..rng.random_range(1..4) {
            let at = rng.random_range(0..=stream.len());
            stream.insert(
                at,
                match rng.random_range(0..4) {
                    0 => bounds(f32::NAN, 0.0, 10.0, 10.0),
                    1 => bounds(0.0, 0.0, f32::INFINITY, 50.0),
                    2 => bounds(-f32::INFINITY, 10.0, f32::INFINITY, 40.0),
                    _ => bounds(30.0, 30.0, -20.0, 40.0),
                },
            );
        }
        let mut index = OrderIndex::default();
        let first = run_frame(&mut index, &stream);
        assert_eq!(
            first,
            brute_force_orders(&stream),
            "round {round} first frame"
        );
        let mut shifted = stream.clone();
        shifted.insert(
            rng.random_range(0..=shifted.len()),
            bounds(5.0, 5.0, 60.0, 60.0),
        );
        let second = run_frame(&mut index, &shifted);
        assert_eq!(
            second,
            brute_force_orders(&shifted),
            "round {round} second frame"
        );
        let third = run_frame(&mut index, &shifted);
        assert_eq!(
            third,
            brute_force_orders(&shifted),
            "round {round} third frame"
        );
    }
}

#[test]
fn exceeding_the_replay_budget_builds_the_grid_without_changing_orders() {
    let mut rng = StdRng::seed_from_u64(99);
    let base: Vec<_> = (0..700)
        .map(|_| {
            bounds(
                rng.random_range(0.0..800.0),
                rng.random_range(0.0..500.0),
                rng.random_range(10.0..120.0),
                rng.random_range(10.0..90.0),
            )
        })
        .collect();
    let mut index = OrderIndex::default();
    assert_eq!(run_frame(&mut index, &base), tree_orders(&base));

    // One entry inserted at the front shifts every stream position, so nothing can be replayed
    // and each entry is searched linearly until the budget forces the grid.
    let mut shifted = base.clone();
    shifted.insert(0, bounds(0.0, 0.0, 900.0, 600.0));
    let actual = run_frame(&mut index, &shifted);
    assert_eq!(actual, tree_orders(&shifted));
    let stats = index.stats();
    assert_eq!(stats.grid_builds, 1);
    assert!(stats.replay_comparisons <= REPLAY_COMPARISON_BUDGET);
    assert!(
        stats.replay_comparisons > REPLAY_COMPARISON_BUDGET - shifted.len() as u32,
        "the budget was spent before the grid was built: {stats:?}"
    );
    assert!(stats.spatial_queries > 0);
    assert_eq!(stats.replayed_entries, 0);

    // A frame identical to the previous one after the grid-built frame replays everything.
    let actual = run_frame(&mut index, &shifted);
    assert_eq!(actual, tree_orders(&shifted));
    assert_eq!(index.stats().replayed_entries, shifted.len() as u32);
}

#[test]
fn replay_check_against_many_changed_regions_respects_the_budget() {
    // 200 small rects spaced along y = 0, followed by 1000 more along y = 5. In the next frame
    // the first 200 move (400 changed regions); the 1000 unchanged ones touch the same bit-grid
    // cells without overlapping any changed region, so each must be compared against the whole
    // changed list until the budget is spent and the grid takes over.
    let frame = |moved: bool| -> Vec<Bounds<ScaledPixels>> {
        (0..1200)
            .map(|i| {
                if i < 200 {
                    bounds(
                        3.0 * i as f32 + if moved { 1.5 } else { 0.0 },
                        0.0,
                        1.0,
                        1.0,
                    )
                } else {
                    bounds(3.0 * (i - 200) as f32, 5.0, 1.0, 1.0)
                }
            })
            .collect()
    };
    let mut index = OrderIndex::default();
    run_frame(&mut index, &frame(false));
    let moved = frame(true);
    let actual = run_frame(&mut index, &moved);
    assert_eq!(actual, tree_orders(&moved));
    let stats = index.stats();
    assert_eq!(stats.grid_builds, 1, "{stats:?}");
    assert!(
        stats.replay_comparisons <= REPLAY_COMPARISON_BUDGET,
        "{stats:?}"
    );
    assert!(
        stats.replay_comparisons > REPLAY_COMPARISON_BUDGET - 400,
        "{stats:?}"
    );
    assert!(
        stats.replayed_entries > 0 && stats.spatial_queries > 0,
        "{stats:?}"
    );
    assert_eq!(stats.entries, 1200);
}

#[test]
fn unchanged_frame_replays_every_order_without_grid_or_queries() {
    let mut rng = StdRng::seed_from_u64(5);
    let stream: Vec<_> = (0..500).map(|_| random_bounds(&mut rng)).collect();
    let mut index = OrderIndex::default();
    let first = run_frame(&mut index, &stream);
    assert_eq!(first, tree_orders(&stream));

    let second = run_frame(&mut index, &stream);
    assert_eq!(second, first);
    assert_eq!(
        index.stats(),
        SceneOrderStats {
            entries: 500,
            replayed_entries: 500,
            ..SceneOrderStats::default()
        }
    );
    assert!(index.changed_regions().is_empty());
}

#[test]
fn changing_one_primitive_marks_old_and_new_bounds_and_reevaluates_overlaps() {
    let a = bounds(0.0, 0.0, 100.0, 100.0);
    let b = bounds(200.0, 0.0, 100.0, 100.0);
    let c = bounds(0.0, 0.0, 50.0, 50.0); // overlaps the old a
    let d = bounds(200.0, 0.0, 50.0, 50.0); // overlaps b only
    let e = bounds(500.0, 500.0, 10.0, 10.0); // overlaps nothing
    let f = bounds(60.0, 60.0, 20.0, 20.0); // overlaps the old a, not c
    let h = bounds(310.0, 310.0, 10.0, 10.0); // overlaps the new a only
    let before = vec![a, b, c, d, e, f, h];

    let mut index = OrderIndex::default();
    let first = run_frame(&mut index, &before);
    assert_eq!(first, vec![1, 1, 2, 2, 1, 2, 1]);

    let moved_a = bounds(300.0, 300.0, 100.0, 100.0);
    let after = vec![moved_a, b, c, d, e, f, h];
    let second = run_frame(&mut index, &after);
    assert_eq!(second, tree_orders(&after));
    assert_eq!(second, vec![1, 1, 1, 2, 1, 1, 2]);

    let stats = index.stats();
    // a (changed), c, f (overlap the old a) and h (overlaps the new a) are recomputed;
    // b, d and e do not intersect any changed region and are replayed.
    assert_eq!(stats.recomputed_entries, 4);
    assert_eq!(stats.replayed_entries, 3);
    assert_eq!(stats.entries, 7);
    // Both the old and the new bounds of `a` were recorded first, in that order.
    let regions = index.changed_regions();
    assert_eq!(regions[0], a);
    assert_eq!(regions[1], moved_a);
    // c, f and h changed order and are recorded as well (old and new region each).
    assert_eq!(regions.len(), 8);
}

#[test]
fn unmarkable_changed_regions_fall_back_to_exact_checks() {
    // The changed region has an infinite edge and cannot be marked in the bit grid; every later
    // entry overlapping it must still be re-evaluated.
    let x = bounds(0.0, 0.0, 100.0, 100.0);
    let y = bounds(10.0, 10.0, 10.0, 10.0);
    let z = bounds(500.0, 500.0, 10.0, 10.0);
    let mut index = OrderIndex::default();
    assert_eq!(run_frame(&mut index, &[x, y, z]), vec![1, 2, 1]);

    let wide = bounds(0.0, 0.0, f32::INFINITY, 100.0);
    let stream = [wide, y, z];
    let actual = run_frame(&mut index, &stream);
    assert_eq!(actual, brute_force_orders(&stream));
    assert_eq!(actual, vec![1, 2, 1]);
    // y overlaps the changed (infinite) region and is recomputed, z lies outside of it.
    assert_eq!(index.stats().recomputed_entries, 2);
    assert_eq!(index.stats().replayed_entries, 1);
}

#[test]
fn first_primitive_and_isolated_primitives_receive_order_one() {
    let mut index = OrderIndex::default();
    let stream = [
        bounds(0.0, 0.0, 10.0, 10.0),
        // Touching edges do not overlap.
        bounds(10.0, 0.0, 10.0, 10.0),
        bounds(0.0, 10.0, 10.0, 10.0),
        // A degenerate bound strictly inside another's extent overlaps it.
        bounds(5.0, 2.0, 0.0, 4.0),
        // A degenerate bound on the edge does not.
        bounds(10.0, 2.0, 0.0, 4.0),
        // Two degenerate bounds never overlap each other.
        bounds(5.0, 2.0, 0.0, 4.0),
    ];
    assert_eq!(run_frame(&mut index, &stream), vec![1, 1, 1, 2, 1, 2]);
    assert_eq!(tree_orders(&stream), vec![1, 1, 1, 2, 1, 2]);
}

// ---------------------------------------------------------------------------------------------
// PERF-PRF-002 workload at the scene level.
// ---------------------------------------------------------------------------------------------

const SCALE: f32 = 2.0;

fn hsl_to_hsla(hue: f32) -> Hsla {
    crate::hsla(hue / 360.0, 0.5, 0.9, 1.0)
}

fn content_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: bounds(0.0, 0.0, 1760.0, 960.0),
    }
}

fn cell_quad(x: f32, y: f32, fill: Hsla) -> Quad {
    Quad {
        bounds: bounds(x * SCALE, y * SCALE, 22.0 * SCALE, 19.0 * SCALE),
        content_mask: content_mask(),
        background: fill.into(),
        ..Default::default()
    }
}

fn label_sprite(x: f32, y: f32, digit: u32) -> MonochromeSprite {
    MonochromeSprite {
        order: 0,
        blur: 0.0,
        bounds: bounds(
            (x + 2.0) * SCALE,
            (y + 3.0) * SCALE,
            8.0 * SCALE,
            13.0 * SCALE,
        ),
        content_mask: content_mask(),
        color: Hsla::default(),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Monochrome,
            },
            tile_id: TileId(digit),
            padding: 0,
            bounds: Default::default(),
        },
        transformation: TransformationMatrix::unit(),
        scope: Default::default(),
    }
}

/// Emits the primitives of frame `f` of the PERF-PRF-002 workload in draw order.
fn perf_frame(f: usize, emit: &mut impl FnMut(Primitive)) {
    let phase = if f < 330 {
        1
    } else if f < 630 {
        2
    } else {
        3
    };
    let overlay_row = (f.wrapping_sub(630)) % 25;
    let overlay = |emit: &mut dyn FnMut(Primitive)| {
        emit(Primitive::Quad(Quad {
            bounds: bounds(0.0, 0.0, 1760.0, 960.0),
            content_mask: content_mask(),
            background: crate::hsla(0.0, 0.0, 0.0, 6.0 / 255.0).into(),
            ..Default::default()
        }))
    };
    let moved = if phase >= 2 && f >= 330 {
        Some((f - 330) % 1000)
    } else {
        None
    };
    for i in 0..1000usize {
        let (column, row) = (i % 40, i / 40);
        if phase == 3 && column == 0 {
            if row == overlay_row {
                overlay(emit);
            }
            if row == 12 {
                overlay(emit);
            }
        }
        let (x, y) = (22.0 * column as f32, 19.0 * row as f32);
        let hue = if phase == 1 && f >= 30 && i == f % 1000 {
            ((7 * i + 5 * f) % 360) as f32
        } else {
            ((7 * i) % 360) as f32
        };
        let x = if moved == Some(i) { x + 11.0 } else { x };
        emit(Primitive::Quad(cell_quad(x, y, hsl_to_hsla(hue))));
        emit(Primitive::MonochromeSprite(label_sprite(
            x,
            y,
            (i % 10) as u32,
        )));
    }
}

/// The upstream scene stage: bounds tree ordering and a stable comparison sort per frame.
#[derive(Default)]
struct LegacyScene {
    tree: BoundsTree<ScaledPixels>,
    quads: Vec<Quad>,
    sprites: Vec<MonochromeSprite>,
}

impl LegacyScene {
    fn clear(&mut self) {
        self.tree.clear();
        self.quads.clear();
        self.sprites.clear();
    }

    fn insert(&mut self, primitive: Primitive) {
        let clipped = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);
        if clipped.is_empty() {
            return;
        }
        let order = self.tree.insert(clipped);
        match primitive {
            Primitive::Quad(mut quad) => {
                quad.order = order;
                self.quads.push(quad);
            }
            Primitive::MonochromeSprite(mut sprite) => {
                sprite.order = order;
                self.sprites.push(sprite);
            }
            _ => unreachable!(),
        }
    }

    fn finish(&mut self) {
        self.quads.sort_by_key(|quad| quad.order);
        self.sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
    }
}

fn same_orders(scene: &Scene, legacy: &LegacyScene) -> bool {
    scene.quads.len() == legacy.quads.len()
        && scene
            .quads
            .iter()
            .zip(&legacy.quads)
            .all(|(a, b)| a.order == b.order)
        && scene.monochrome_sprites.len() == legacy.sprites.len()
        && scene
            .monochrome_sprites
            .iter()
            .zip(&legacy.sprites)
            .all(|(a, b)| a.order == b.order)
}

#[test]
fn perf_workload_orders_match_legacy_and_phase_one_replays_linearly() {
    let mut scenes = [Scene::default(), Scene::default()];
    let mut legacy = LegacyScene::default();
    let mut last_stats = SceneOrderStats::default();
    let mut phase_two_grid_builds = 0;
    let frames: Vec<usize> = (0..40).chain(322..346).chain(622..650).collect();
    for &f in &frames {
        let (previous, current) = if f % 2 == 0 {
            let (a, b) = scenes.split_at_mut(1);
            (&b[0], &mut a[0])
        } else {
            let (a, b) = scenes.split_at_mut(1);
            (&a[0], &mut b[0])
        };
        current.clear();
        current.adopt_order_baseline(previous);
        legacy.clear();
        perf_frame(f, &mut |primitive| {
            legacy.insert(primitive.clone());
            current.insert_primitive(primitive);
        });
        current.finish();
        legacy.finish();
        assert!(same_orders(current, &legacy), "frame {f}");
        last_stats = current.order_stats();
        let consecutive = f > 0 && frames.contains(&(f - 1));
        if consecutive && (31..330).contains(&f) {
            // Phase 1: geometry never changes, so orders are replayed linearly.
            assert_eq!(last_stats.replayed_entries, last_stats.entries, "frame {f}");
            assert_eq!(last_stats.grid_builds, 0);
            assert_eq!(last_stats.spatial_queries, 0);
        }
        if consecutive && (331..630).contains(&f) {
            phase_two_grid_builds += last_stats.grid_builds;
        }
    }
    assert_eq!(
        phase_two_grid_builds, 0,
        "one moved cell never needs the grid"
    );
    assert!(last_stats.entries >= 2000);
}

/// Timing of the scene-finish stage (ordering during insertion plus sorting) for the
/// PERF-PRF-002 phases against the upstream path. Run with
/// `cargo test -p vespertine --release scene_order::tests::perf_prf_002 -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark; prints timings"]
fn perf_prf_002_scene_finish_timing() {
    let mut scenes = [Scene::default(), Scene::default()];
    let mut legacy = LegacyScene::default();
    let phases = [
        ("phase 1", 30..330usize),
        ("phase 2", 330..630),
        ("phase 3", 630..930),
    ];
    let mut total_new = [Duration::ZERO; 3];
    let mut total_old = [Duration::ZERO; 3];
    let mut frames = [0u32; 3];
    let mut allocation_marks = [0u64; 4];
    let mut stats_sum = [SceneOrderStats::default(); 3];
    // Pre-generate primitives so only the scene stage is timed.
    let streams: Vec<Vec<Primitive>> = (0..930)
        .map(|f| {
            let mut primitives = Vec::new();
            perf_frame(f, &mut |p| primitives.push(p));
            primitives
        })
        .collect();
    for (f, stream) in streams.iter().enumerate() {
        let (a, b) = scenes.split_at_mut(1);
        let (previous, current) = if f % 2 == 0 {
            (&b[0], &mut a[0])
        } else {
            (&a[0], &mut b[0])
        };
        let started = Instant::now();
        current.clear();
        current.adopt_order_baseline(previous);
        for primitive in stream {
            current.insert_primitive(primitive.clone());
        }
        current.finish();
        let new_time = started.elapsed();

        let started = Instant::now();
        legacy.clear();
        for primitive in stream {
            legacy.insert(primitive.clone());
        }
        legacy.finish();
        let old_time = started.elapsed();

        if let Some(phase) = phases.iter().position(|(_, range)| range.contains(&f)) {
            total_new[phase] += new_time;
            total_old[phase] += old_time;
            frames[phase] += 1;
            let stats = current.order_stats();
            stats_sum[phase].replayed_entries += stats.replayed_entries;
            stats_sum[phase].recomputed_entries += stats.recomputed_entries;
            stats_sum[phase].spatial_queries += stats.spatial_queries;
            stats_sum[phase].grid_builds += stats.grid_builds;
            stats_sum[phase].replay_comparisons += stats.replay_comparisons;
        }
        if let Some(mark) = [29usize, 329, 629, 929].iter().position(|&last| last == f) {
            allocation_marks[mark] =
                scenes[0].sort_scratch_allocations() + scenes[1].sort_scratch_allocations();
        }
    }
    for (phase, (name, _)) in phases.iter().enumerate() {
        let n = frames[phase];
        let new = total_new[phase] / n;
        let old = total_old[phase] / n;
        println!(
            "{name}: new {new:?}/frame, upstream {old:?}/frame, speedup {:.2}x; \
             per frame replayed {} recomputed {} queries {} grid builds {} comparisons {}",
            old.as_secs_f64() / new.as_secs_f64(),
            stats_sum[phase].replayed_entries / n,
            stats_sum[phase].recomputed_entries / n,
            stats_sum[phase].spatial_queries / n,
            stats_sum[phase].grid_builds as f32 / n as f32,
            stats_sum[phase].replay_comparisons / n,
        );
    }
    println!(
        "sort scratch growth events (cumulative over both scenes) at the end of warm-up and phases 1-3: {allocation_marks:?}"
    );
}

#[test]
fn signed_zero_edges_follow_the_total_order_of_scaled_pixels() {
    // Upstream compares pixel values with the IEEE total order (-0.0 < 0.0), so a bounds
    // starting at -0.0 overlaps one that ends at 0.0.
    let ends_at_zero = bounds(-100.0, 0.0, 100.0, 10.0);
    let starts_at_negative_zero = bounds(-0.0, 0.0, 50.0, 50.0);
    let starts_at_zero = bounds(0.0, 0.0, 50.0, 50.0);
    let spans_zero = bounds(-1.0, 0.0, 2.0, 5.0);
    let degenerate_at_negative_zero = bounds(-0.0, 1.0, 0.0, 5.0);
    let stream = [
        ends_at_zero,
        starts_at_negative_zero,
        starts_at_zero,
        spans_zero,
        degenerate_at_negative_zero,
    ];
    let expected = tree_orders(&stream);
    assert_eq!(expected, brute_force_orders(&stream));
    assert_eq!(expected[1], 2, "-0.0 start overlaps a bounds ending at 0.0");
    assert_eq!(
        expected[2], 3,
        "only the overlap with the second bounds counts"
    );
    let mut index = OrderIndex::default();
    assert_eq!(run_frame(&mut index, &stream), expected);
    // The same stream through the spatial grid.
    index.begin_frame();
    index.build_grid();
    let through_grid: Vec<u32> = stream.iter().map(|b| index.insert(*b)).collect();
    assert_eq!(through_grid, expected);
    // Replay compares bounds bit for bit, so -0.0 and 0.0 are different bounds.
    let mut changed = stream;
    changed[1] = starts_at_zero;
    let actual = run_frame(&mut index, &changed);
    assert_eq!(actual, tree_orders(&changed));
}

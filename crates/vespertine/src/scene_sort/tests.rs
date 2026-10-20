use super::*;
use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, Hsla, PaintOperation,
    Primitive, Scene, TileId, TransformationMatrix, point, size,
};
use rand::{Rng, SeedableRng, rngs::StdRng};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn wide_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: bounds(-1.0e6, -1.0e6, 2.0e6, 2.0e6),
    }
}

fn quad(order: u32, tag: f32) -> Quad {
    Quad {
        order,
        bounds: bounds(tag, 0.0, 1.0, 1.0),
        content_mask: wide_mask(),
        ..Default::default()
    }
}

fn texture(index: u32) -> AtlasTextureId {
    AtlasTextureId {
        index,
        kind: AtlasTextureKind::Monochrome,
    }
}

/// A sprite whose `blur` field carries its original index.
fn sprite(order: u32, texture_index: u32, tile: u32, original_index: u32) -> MonochromeSprite {
    MonochromeSprite {
        order,
        blur: original_index as f32,
        bounds: bounds(0.0, 0.0, 1.0, 1.0),
        content_mask: wide_mask(),
        color: Hsla::default(),
        tile: AtlasTile {
            texture_id: texture(texture_index),
            tile_id: TileId(tile),
            padding: 0,
            bounds: Default::default(),
        },
        transformation: TransformationMatrix::unit(),
        scope: Default::default(),
    }
}

fn sort_quads(quads: &mut Vec<Quad>, scratch: &mut SortScratch) {
    sort_records(quads, &mut scratch.gather.quads, &mut scratch.index);
}

fn sort_sprites(sprites: &mut Vec<MonochromeSprite>, scratch: &mut SortScratch) {
    sort_records(
        sprites,
        &mut scratch.gather.monochrome_sprites,
        &mut scratch.index,
    );
}

fn expected_quads(quads: &[Quad]) -> Vec<(u32, f32)> {
    let mut expected: Vec<_> = quads
        .iter()
        .map(|q| (q.order, q.bounds.origin.x.0))
        .collect();
    expected.sort_by_key(|(order, _)| *order);
    expected
}

fn expected_sprites(sprites: &[MonochromeSprite]) -> Vec<u32> {
    let mut expected: Vec<_> = sprites.iter().collect();
    expected.sort_by_key(|s| {
        (
            s.order,
            s.tile.texture_id.index,
            s.tile.tile_id.0,
            s.blur as u32,
        )
    });
    expected.iter().map(|s| s.blur as u32).collect()
}

#[test]
fn dense_and_sparse_orders_sort_like_a_stable_comparison_sort() {
    let mut scratch = SortScratch::default();
    for seed in 0..200u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let count = rng.random_range(2..120usize);
        // Dense: max order <= 4 * count (counting placement). Sparse: above (compact keys).
        let max_order = if seed % 2 == 0 {
            rng.random_range(1..=4 * count as u32)
        } else {
            rng.random_range(4 * count as u32 + 1..100_000)
        };
        let mut quads: Vec<_> = (0..count)
            .map(|i| quad(rng.random_range(1..=max_order), i as f32))
            .collect();
        let mut sprites: Vec<_> = (0..count)
            .map(|i| {
                sprite(
                    rng.random_range(1..=max_order),
                    rng.random_range(0..3),
                    rng.random_range(0..4),
                    i as u32,
                )
            })
            .collect();
        let expected_q = expected_quads(&quads);
        let expected_s = expected_sprites(&sprites);
        let actual_max = quads.iter().map(|q| q.order).max().unwrap() as usize;
        sort_quads(&mut quads, &mut scratch);
        sort_sprites(&mut sprites, &mut scratch);
        let actual_q: Vec<_> = quads
            .iter()
            .map(|q| (q.order, q.bounds.origin.x.0))
            .collect();
        assert_eq!(
            actual_q, expected_q,
            "seed {seed} (max order {actual_max}, count {count})"
        );
        let actual_s: Vec<_> = sprites.iter().map(|s| s.blur as u32).collect();
        assert_eq!(actual_s, expected_s, "seed {seed}");
    }
}

#[test]
fn sparse_and_dense_paths_are_both_taken() {
    let mut scratch = SortScratch::default();
    // Dense: counting placement allocates counts, never keys.
    let mut dense = vec![quad(3, 0.0), quad(1, 1.0), quad(2, 2.0), quad(1, 3.0)];
    sort_quads(&mut dense, &mut scratch);
    assert!(scratch.index.counts.capacity() > 0);
    assert_eq!(scratch.index.keys.capacity(), 0);
    // Sparse: max order 500 > 4 * 3 uses compact keys.
    let mut sparse = vec![quad(500, 0.0), quad(1, 1.0), quad(7, 2.0)];
    sort_quads(&mut sparse, &mut scratch);
    assert!(scratch.index.keys.capacity() > 0);
    assert_eq!(
        sparse.iter().map(|q| q.order).collect::<Vec<_>>(),
        vec![1, 7, 500]
    );
}

#[test]
fn sprites_sort_by_order_texture_tile_and_original_index() {
    let mut scratch = SortScratch::default();
    let mut sprites = vec![
        sprite(2, 1, 5, 0),
        sprite(1, 1, 9, 1),
        sprite(1, 0, 9, 2),
        sprite(1, 0, 3, 3),
        sprite(1, 0, 3, 4), // equal key as index 3: stays after it
        sprite(2, 0, 1, 5),
        sprite(1, 1, 9, 6), // equal key as index 1: stays after it
    ];
    sort_sprites(&mut sprites, &mut scratch);
    let order: Vec<u32> = sprites.iter().map(|s| s.blur as u32).collect();
    // (order, texture, tile, index): (1,0,3,3) (1,0,3,4) (1,0,9,2) (1,1,9,1) (1,1,9,6)
    // (2,0,1,5) (2,1,5,0)
    assert_eq!(order, vec![3, 4, 2, 1, 6, 5, 0]);

    // The same keys in a sparse order space sort identically.
    let mut sparse = vec![
        sprite(900, 1, 5, 0),
        sprite(1, 1, 9, 1),
        sprite(1, 0, 9, 2),
        sprite(1, 0, 3, 3),
        sprite(1, 0, 3, 4),
        sprite(900, 0, 1, 5),
        sprite(1, 1, 9, 6),
    ];
    sort_sprites(&mut sparse, &mut scratch);
    assert!(scratch.index.sprite_keys.capacity() > 0);
    assert_eq!(
        sparse.iter().map(|s| s.blur as u32).collect::<Vec<_>>(),
        vec![3, 4, 2, 1, 6, 5, 0]
    );
}

#[test]
fn single_texture_sprite_arrays_sort_like_a_stable_comparison_sort() {
    let mut scratch = SortScratch::default();
    // Short and long arrays (comparison and radix paths), dense and sparse orders, tile
    // identities spanning one, three and four bytes.
    for seed in 0..12u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let count = if seed % 2 == 0 {
            rng.random_range(2..RADIX_MIN_LEN)
        } else {
            rng.random_range(RADIX_MIN_LEN..3000usize)
        };
        let max_order = [3u32, 40, 100_000][seed as usize % 3];
        let tile_span = [16u32, 70_000, u32::MAX][(seed as usize / 2) % 3];
        let mut sprites: Vec<_> = (0..count)
            .map(|i| {
                sprite(
                    rng.random_range(1..=max_order),
                    7,
                    rng.random_range(0..tile_span),
                    i as u32,
                )
            })
            .collect();
        let expected = expected_sprites(&sprites);
        sort_sprites(&mut sprites, &mut scratch);
        let actual: Vec<_> = sprites.iter().map(|s| s.blur as u32).collect();
        assert_eq!(actual, expected, "seed {seed} count {count}");
    }
}

#[test]
fn already_sorted_input_is_not_moved() {
    let mut scratch = SortScratch::default();
    let mut quads: Vec<_> = (0..64).map(|i| quad(1 + i / 4, i as f32)).collect();
    let address = quads.as_ptr();
    let capacity = quads.capacity();
    sort_quads(&mut quads, &mut scratch);
    assert_eq!(quads.as_ptr(), address);
    assert_eq!(quads.capacity(), capacity);
    assert!(
        quads
            .iter()
            .enumerate()
            .all(|(i, q)| q.bounds.origin.x.0 == i as f32)
    );
    assert_eq!(scratch.index.allocations(), 0);
    assert_eq!(scratch.gather.quads.capacity(), 0);

    let mut sprites: Vec<_> = (0..40).map(|i| sprite(1 + i / 8, 0, i % 8, i)).collect();
    let address = sprites.as_ptr();
    sort_sprites(&mut sprites, &mut scratch);
    assert_eq!(sprites.as_ptr(), address);
    assert_eq!(scratch.index.allocations(), 0);
    assert_eq!(scratch.gather.monochrome_sprites.capacity(), 0);
}

fn unsorted_scene(chain: usize, isolated: usize) -> Scene {
    let mut scene = Scene::default();
    // A chain of overlapping quads (orders 1..=chain), then isolated ones (order 1) that sort
    // in front of most of the chain.
    for i in 0..chain {
        scene.insert_primitive(Quad {
            bounds: bounds(i as f32, i as f32, 100.0, 100.0),
            content_mask: wide_mask(),
            ..Default::default()
        });
        scene.insert_primitive(sprite(0, 0, (i % 7) as u32, 0).clone_at(10.0 + i as f32));
    }
    for i in 0..isolated {
        scene.insert_primitive(Quad {
            bounds: bounds(1000.0 + 200.0 * i as f32, 0.0, 10.0, 10.0),
            content_mask: wide_mask(),
            ..Default::default()
        });
    }
    scene
}

trait CloneAt {
    fn clone_at(&self, x: f32) -> MonochromeSprite;
}

impl CloneAt for MonochromeSprite {
    fn clone_at(&self, x: f32) -> MonochromeSprite {
        MonochromeSprite {
            bounds: bounds(x, 5.0, 3.0, 3.0),
            ..*self
        }
    }
}

#[test]
fn warmed_repeated_scene_allocates_no_new_sort_scratch() {
    let mut scene = Scene::default();
    let mut after_warmup = 0;
    for frame in 0..12 {
        let fresh = unsorted_scene(150, 20);
        scene.clear();
        for operation in fresh.paint_operations {
            if let PaintOperation::Primitive(primitive) = operation {
                scene.insert_primitive(primitive);
            }
        }
        scene.finish();
        assert!(scene.quads.windows(2).all(|w| w[0].order <= w[1].order));
        if frame == 3 {
            after_warmup = scene.sort_scratch_allocations();
            assert!(
                after_warmup > 0,
                "the first frames must have grown the scratch"
            );
        }
    }
    assert_eq!(scene.sort_scratch_allocations(), after_warmup);

    // Fewer primitives than the previously reached capacity: still no growth.
    let smaller = unsorted_scene(100, 10);
    scene.clear();
    for operation in smaller.paint_operations {
        if let PaintOperation::Primitive(primitive) = operation {
            scene.insert_primitive(primitive);
        }
    }
    scene.finish();
    assert_eq!(scene.sort_scratch_allocations(), after_warmup);
}

#[test]
fn scene_sort_equals_the_unoptimized_stable_sort() {
    // Sparse orders: 10 quads on top of a chain of 200 shadows.
    let mut scene = Scene::default();
    for i in 0..200 {
        scene.insert_primitive(crate::Shadow {
            order: 0,
            blur_radius: ScaledPixels(1.0),
            bounds: bounds(i as f32 * 0.5, 0.0, 50.0, 50.0),
            corner_radii: Default::default(),
            content_mask: wide_mask(),
            color: Hsla::default(),
            element_bounds: Default::default(),
            element_corner_radii: Default::default(),
            inset: 0,
            curved_clip: 0,
        });
    }
    for i in 0..10 {
        scene.insert_primitive(Quad {
            bounds: bounds(i as f32, 0.0, 50.0, 50.0),
            content_mask: wide_mask(),
            ..Default::default()
        });
    }
    // Isolated low-order quads inserted last force a reorder.
    for i in 0..5 {
        scene.insert_primitive(Quad {
            bounds: bounds(5000.0 + 100.0 * i as f32, 0.0, 10.0, 10.0),
            content_mask: wide_mask(),
            ..Default::default()
        });
    }
    let mut expected_quads: Vec<_> = scene
        .quads
        .iter()
        .map(|q| (q.order, q.bounds.origin.x.0))
        .collect();
    expected_quads.sort_by_key(|(order, _)| *order);
    let max_quad_order = expected_quads.iter().map(|(o, _)| *o).max().unwrap() as usize;
    assert!(
        max_quad_order > 4 * scene.quads.len(),
        "orders must be sparse"
    );
    scene.finish();
    let actual: Vec<_> = scene
        .quads
        .iter()
        .map(|q| (q.order, q.bounds.origin.x.0))
        .collect();
    assert_eq!(actual, expected_quads);
    assert!(scene.shadows.windows(2).all(|w| w[0].order <= w[1].order));
}

#[test]
fn layer_scopes_stay_balanced_and_primitives_keep_their_scope() {
    let mut scene = Scene::default();
    // Background content, then a layer holding several quads and sprites, then more content
    // drawn over the layer, then a nested layer.
    scene.insert_primitive(quad_at(0.0, 0.0, 400.0, 400.0));
    scene.push_layer(bounds(10.0, 10.0, 300.0, 300.0));
    let layer_order = scene.quads.last().unwrap().order + 1;
    for i in 0..5 {
        scene.insert_primitive(quad_at(20.0 + 40.0 * i as f32, 20.0, 30.0, 30.0));
        scene.insert_primitive(sprite(0, 0, 4 - i as u32, 0).clone_at(20.0 + 40.0 * i as f32));
    }
    scene.push_layer(bounds(50.0, 50.0, 100.0, 100.0));
    scene.insert_primitive(quad_at(60.0, 60.0, 20.0, 20.0));
    scene.pop_layer();
    scene.insert_primitive(quad_at(30.0, 30.0, 10.0, 10.0));
    scene.pop_layer();
    scene.insert_primitive(quad_at(15.0, 15.0, 200.0, 200.0));
    scene.insert_primitive(quad_at(1000.0, 1000.0, 5.0, 5.0));

    let operations_before: Vec<String> = scene.paint_operations.iter().map(describe).collect();
    scene.finish();
    let operations_after: Vec<String> = scene.paint_operations.iter().map(describe).collect();
    assert_eq!(
        operations_before, operations_after,
        "the operation log is never reordered"
    );

    let mut depth = 0i32;
    for operation in &scene.paint_operations {
        match operation {
            PaintOperation::StartLayer(_) => depth += 1,
            PaintOperation::EndLayer => {
                depth -= 1;
                assert!(depth >= 0);
            }
            PaintOperation::Primitive(_)
            | PaintOperation::BackdropBlur(_)
            | PaintOperation::PushCurvedClip(_)
            | PaintOperation::PopCurvedClip => {}
        }
    }
    assert_eq!(depth, 0);

    // Primitives inserted inside the outer layer carry the layer's order (and the nested layer
    // its own), wherever the sort moved them.
    let in_layer = scene
        .quads
        .iter()
        .filter(|q| q.order == layer_order)
        .count();
    assert_eq!(
        in_layer,
        5 + 1,
        "five quads and the one after the nested layer"
    );
    assert_eq!(
        scene
            .monochrome_sprites
            .iter()
            .filter(|s| s.order == layer_order)
            .count(),
        5
    );
    assert!(scene.quads.windows(2).all(|w| w[0].order <= w[1].order));
    // The background quad precedes the layer, the quad drawn over it follows it.
    assert!(scene.quads.first().unwrap().order < layer_order);
    assert!(scene.quads.iter().any(|q| q.order > layer_order));
    // Sprites inside the layer share an order and group by tile within it.
    let tiles: Vec<u32> = scene
        .monochrome_sprites
        .iter()
        .map(|s| s.tile.tile_id.0)
        .collect();
    assert_eq!(tiles, vec![0, 1, 2, 3, 4]);
}

fn quad_at(x: f32, y: f32, width: f32, height: f32) -> Quad {
    Quad {
        bounds: bounds(x, y, width, height),
        content_mask: wide_mask(),
        ..Default::default()
    }
}

fn describe(operation: &PaintOperation) -> String {
    match operation {
        PaintOperation::StartLayer(b) => format!("start {b:?}"),
        PaintOperation::EndLayer => "end".into(),
        PaintOperation::Primitive(Primitive::Quad(q)) => format!("quad {} {:?}", q.order, q.bounds),
        PaintOperation::Primitive(Primitive::MonochromeSprite(s)) => {
            format!("sprite {} {:?}", s.order, s.bounds)
        }
        PaintOperation::Primitive(_)
        | PaintOperation::BackdropBlur(_)
        | PaintOperation::PushCurvedClip(_)
        | PaintOperation::PopCurvedClip => "other".into(),
    }
}

//! CMP-026 A6 (mouse table), A7 (time inputs), A8 (blend formulas), render size and uniform packing.

use vespertine::{point, px, size};
use vespertine_components::shader::blend::{
    BlendMode, blend, bright_pass, clamp_bloom, edge_fade, luma,
};
use vespertine_components::shader::inputs::{
    TimeInputs, TimeState, UniformInputs, mouse_table, render_size,
};

#[test]
fn a6_mouse_table() {
    let (scale, height) = (2.0, 600.0);
    // Not hovered: (0, 0, -1, -1), whether or not a press is recorded.
    assert_eq!(
        mouse_table(None, None, height, scale),
        [0.0, 0.0, -1.0, -1.0]
    );
    assert_eq!(
        mouse_table(None, Some(point(px(5.), px(5.))), height, scale),
        [0.0, 0.0, -1.0, -1.0]
    );
    // Hovered: (x, y, -1, -1) with y measured from the bottom.
    let hovered = mouse_table(Some(point(px(10.), px(20.))), None, height, scale);
    assert_eq!(hovered, [20.0, 560.0, -1.0, -1.0]);
    // Pressed: the press origin fills components three and four.
    let pressed = mouse_table(
        Some(point(px(10.), px(20.))),
        Some(point(px(3.), px(4.))),
        height,
        scale,
    );
    assert_eq!(pressed, [20.0, 560.0, 6.0, 592.0]);
    // The top row maps to the largest y, the bottom row to 0.
    assert_eq!(
        mouse_table(Some(point(px(0.), px(0.))), None, height, scale)[1],
        600.0
    );
    assert_eq!(
        mouse_table(Some(point(px(0.), px(300.))), None, height, scale)[1],
        0.0
    );
}

#[test]
fn a7_time_inputs() {
    // An animated surface: the first frame has delta 0 and counter 0.
    let mut time = TimeState::default();
    let first = time.begin_frame(1.5, true);
    assert_eq!(
        first,
        TimeInputs {
            time: 1.5,
            delta: 0.0,
            frame: 0
        }
    );
    let second = time.begin_frame(1.75, true);
    assert_eq!((second.time, second.delta, second.frame), (1.75, 0.25, 1));
    let third = time.begin_frame(1.70, true);
    assert_eq!(third.delta, 0.0, "the delta is clamped to at least 0");
    assert_eq!(third.frame, 2);
    assert_eq!(time.frames_started(), 3);

    // A non-animated surface keeps the time of its first frame; the counter still advances.
    let mut still = TimeState::default();
    let a = still.begin_frame(2.0, false);
    let b = still.begin_frame(9.0, false);
    let c = still.begin_frame(30.0, false);
    assert_eq!((a.time, b.time, c.time), (2.0, 2.0, 2.0));
    assert_eq!((a.delta, b.delta, c.delta), (0.0, 0.0, 0.0));
    assert_eq!((a.frame, b.frame, c.frame), (0, 1, 2));
}

fn close(a: [f32; 4], b: [f32; 4]) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} != {b:?}");
    }
}

#[test]
fn a8_blend_formulas() {
    let no_glow = [0.0; 3];
    // Light with black is transparent.
    assert_eq!(
        blend(BlendMode::Light, [0.0; 3], 1.0, no_glow, 0.0, 1.0),
        [0.0; 4]
    );
    // Light with (0.5, 0.25, 0): straight (1, 0.5, 0), alpha 0.5 in the interior.
    close(
        blend(BlendMode::Light, [0.5, 0.25, 0.0], 1.0, no_glow, 0.0, 1.0),
        [1.0, 0.5, 0.0, 0.5],
    );
    // Values above 1 survive to the bloom stage and clamp afterwards.
    close(
        blend(BlendMode::Light, [2.0, 0.5, 0.0], 1.0, no_glow, 0.0, 1.0),
        [1.0, 0.5, 0.0, 1.0],
    );
    // The fade multiplies alpha only.
    close(
        blend(BlendMode::Light, [0.5, 0.25, 0.0], 1.0, no_glow, 0.0, 0.5),
        [1.0, 0.5, 0.0, 0.25],
    );
    // Glow adds to the color before the alpha is taken.
    close(
        blend(
            BlendMode::Light,
            [0.25, 0.0, 0.0],
            1.0,
            [1.0, 0.5, 0.0],
            0.5,
            1.0,
        ),
        [1.0, 0.333333, 0.0, 0.75],
    );
    // Opaque: alpha 1 everywhere, no fade, rgb clamped.
    close(
        blend(BlendMode::Opaque, [0.0; 3], 0.0, no_glow, 0.0, 0.0),
        [0.0, 0.0, 0.0, 1.0],
    );
    close(
        blend(BlendMode::Opaque, [3.0, -1.0, 0.5], 0.2, no_glow, 2.0, 0.0),
        [1.0, 0.0, 0.5, 1.0],
    );
    // Alpha: a = clamp(alpha + luma(glow x bloom)); straight color from the weighted sum.
    close(
        blend(BlendMode::Alpha, [0.5, 0.5, 0.5], 0.5, no_glow, 0.0, 1.0),
        [0.5, 0.5, 0.5, 0.5],
    );
    assert_eq!(
        blend(BlendMode::Alpha, [1.0; 3], 0.0, no_glow, 0.0, 1.0),
        [0.0; 4]
    );
    let a = blend(
        BlendMode::Alpha,
        [1.0, 0.0, 0.0],
        0.5,
        [1.0, 1.0, 1.0],
        1.0,
        1.0,
    );
    let expected_alpha = (0.5 + luma([1.0, 1.0, 1.0])).clamp(0.0, 1.0);
    assert!((a[3] - expected_alpha).abs() < 1e-6);
    assert_eq!(BlendMode::default(), BlendMode::Light);
}

#[test]
fn a8_edge_fade_is_zero_on_the_boundary_and_one_beyond_fifteen_percent() {
    let surface = (200.0, 100.0);
    // The fade width is 15% of the shorter side: 15 px.
    for boundary in [
        (0.0, 50.0),
        (200.0, 50.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 0.0),
    ] {
        assert_eq!(edge_fade(boundary, surface), 0.0, "{boundary:?}");
    }
    let one = |p: (f32, f32)| assert!((edge_fade(p, surface) - 1.0).abs() < 1e-5, "{p:?}");
    one((15.0, 50.0));
    one((100.0, 15.0));
    one((100.0, 50.0));
    one((40.0, 30.0));
    let half = edge_fade((7.5, 50.0), surface);
    assert!(
        (half - 0.5).abs() < 1e-6,
        "the quintic is 0.5 at half width: {half}"
    );
    assert!(edge_fade((3.0, 50.0), surface) < edge_fade((9.0, 50.0), surface));
    // A tiny surface has a width of at least one pixel.
    assert_eq!(edge_fade((1.0, 1.0), (4.0, 4.0)), 1.0);
}

#[test]
fn bloom_strength_is_clamped_and_the_bright_pass_has_a_knee() {
    assert_eq!(clamp_bloom(-1.0), 0.0);
    assert_eq!(clamp_bloom(10.0), 3.0);
    assert_eq!(clamp_bloom(f32::NAN), 0.0);
    assert_eq!(clamp_bloom(1.5), 1.5);
    assert_eq!(bright_pass([0.3, 0.3, 0.3]), [0.0; 3], "below the knee");
    let over = bright_pass([2.0, 2.0, 2.0]);
    assert!(
        (over[0] - 2.0).abs() < 1e-6,
        "above the knee passes unchanged"
    );
}

#[test]
fn render_size_rounds_and_clamps() {
    assert_eq!(render_size(size(px(100.4), px(50.5)), 1.0), (100, 51));
    assert_eq!(render_size(size(px(0.), px(-5.)), 2.0), (1, 1));
    assert_eq!(
        render_size(size(px(100_000.), px(8191.6)), 1.0),
        (8192, 8192)
    );
    assert_eq!(render_size(size(px(f32::NAN), px(10.)), 1.0).0, 1);
}

#[test]
fn the_uniform_block_packs_inputs_four_per_slot() {
    let inputs = UniformInputs {
        size: (640, 480),
        ratio: 2.0,
        time: TimeInputs {
            time: 3.0,
            delta: 0.5,
            frame: 7,
        },
        mouse: [1.0, 2.0, -1.0, -1.0],
        camera: [0.1, 0.2, 1.5],
        inputs: (0..6).map(|i| i as f32 + 0.5).collect(),
        channel_sizes: [(640.0, 480.0), (640.0, 480.0), (1.0, 1.0), (1.0, 1.0)],
    };
    let block = inputs.to_block();
    assert_eq!(block.resolution, [640.0, 480.0, 2.0, 0.0]);
    assert_eq!(block.time[..2], [3.0, 0.5]);
    assert_eq!(block.frame[0], 7);
    assert_eq!(block.camera, [0.1, 0.2, 1.5, 0.0]);
    assert_eq!(block.inputs[0], [0.5, 1.5, 2.5, 3.5]);
    assert_eq!(block.inputs[1], [4.5, 5.5, 0.0, 0.0]);
    assert_eq!(block.channel_resolution[2], [1.0, 1.0, 1.0, 0.0]);
    assert_eq!(
        std::mem::size_of::<vespertine_components::shader::inputs::UniformBlock>(),
        208
    );
}

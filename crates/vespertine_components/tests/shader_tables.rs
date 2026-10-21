//! CMP-026 A6 (mouse table), A7 (time inputs), bloom clamping, render size and uniform packing.

use vespertine::{point, px, size};
use vespertine_components::shader::blend::{BlendMode, clamp_bloom};
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

#[test]
fn bloom_strength_is_clamped_and_the_default_blend_is_light() {
    assert_eq!(clamp_bloom(-1.0), 0.0);
    assert_eq!(clamp_bloom(10.0), 3.0);
    assert_eq!(clamp_bloom(f32::NAN), 0.0);
    assert_eq!(clamp_bloom(1.5), 1.5);
    assert_eq!(BlendMode::default(), BlendMode::Light);
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

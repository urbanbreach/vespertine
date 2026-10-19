//! CMP-026 on the GPU: A2 (compile errors leave feedback untouched), A4 (feedback buffers),
//! A8 (compositing), bloom, inputs, textures. These tests use the available wgpu adapter and
//! skip (as the specification allows: no software execution of user programs is required) only
//! when the machine has none.

use std::sync::Arc;
use vespertine_components::shader::blend::BlendMode;
use vespertine_components::shader::gpu::{GpuState, ShaderFrame, TextureImage, TextureTable, gpu};

macro_rules! require_gpu {
    () => {
        if let Err(reason) = gpu() {
            eprintln!("skipping: no GPU adapter: {reason}");
            return;
        }
    };
}

fn frame(main: &str) -> ShaderFrame {
    ShaderFrame {
        main: main.into(),
        buffers: Vec::new(),
        input_names: Vec::new(),
        input_values: Vec::new(),
        texture_names: Vec::new(),
        textures: TextureTable::new(),
        blend: BlendMode::Opaque,
        bloom: 0.0,
        animated: false,
        size: (64, 32),
        ratio: 1.0,
        mouse: [0.0, 0.0, -1.0, -1.0],
        camera: [0.0, 0.0, 1.0],
        time: 0.0,
    }
}

/// `[b, g, r, a]` of a pixel of a BGRA frame.
fn at(bgra: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [bgra[i], bgra[i + 1], bgra[i + 2], bgra[i + 3]]
}

fn near(actual: u8, expected: f32) {
    assert!(
        (actual as f32 - expected).abs() <= 1.5,
        "{actual} is not near {expected}"
    );
}

fn state() -> GpuState {
    GpuState::new().expect("gpu state")
}

#[test]
fn a_gradient_program_renders_upright_with_bottom_left_fragment_coordinates() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(fragCoord.x / iResolution.x, fragCoord.y / iResolution.y, 0.0, 1.0);
        }",
    );
    f.size = (64, 32);
    let image = state.render(&f).unwrap();
    assert_eq!(image.len(), 64 * 32 * 4);
    // Pixel centers are at half-integers: x = 32 gives 32.5 / 64.
    let [b, g, r, a] = at(&image, 64, 32, 16);
    near(r, 255.0 * 32.5 / 64.0);
    assert_eq!(b, 0);
    assert_eq!(a, 255);
    // The image is upright: the top row has the largest fragment y.
    let top = at(&image, 64, 10, 0)[1];
    let bottom = at(&image, 64, 10, 31)[1];
    assert!(top > 240 && bottom < 15, "top {top}, bottom {bottom}");
    let _ = g;
}

#[test]
fn a4_a_value_written_on_frame_n_is_read_back_as_the_previous_frame_on_frame_n_plus_1() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(texture(iChannel0, fragCoord / iResolution.xy).r, 0.0, 0.0, 1.0);
        }",
    );
    f.buffers = vec![
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            float previous = texture(iChannel0, fragCoord / iResolution.xy).r;
            fragColor = vec4(previous + 0.25, 0.0, 0.0, 1.0);
        }"
        .into(),
    ];
    // The image pass sees the buffer's output of the current frame (an earlier pass); the buffer
    // sees its own output of the previous frame (feedback).
    for (frame_number, expected) in [(1, 0.25), (2, 0.5), (3, 0.75)] {
        let image = state.render(&f).unwrap();
        near(at(&image, 64, 20, 10)[2], expected * 255.0);
        let previous = state.read_previous(0).unwrap();
        assert!(
            (previous[0][0] - expected).abs() < 1e-3,
            "frame {frame_number}: {previous:?}"
        );
    }
    assert_eq!(state.frames(), 3);
    // Changing the size resets the history.
    f.size = (48, 24);
    let image = state.render(&f).unwrap();
    near(at(&image, 48, 20, 10)[2], 0.25 * 255.0);
    assert_eq!(state.feedback_size(), Some((48, 24)));
    // Changing the number of buffers resets it too.
    f.buffers.push(f.buffers[0].clone());
    let image = state.render(&f).unwrap();
    assert!(at(&image, 48, 20, 10)[2] <= 130, "history was lost");
}

#[test]
fn a2_a_compile_error_leaves_the_feedback_state_and_frame_counter_untouched() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(texture(iChannel0, fragCoord / iResolution.xy).r, 0.0, 0.0, 1.0);
        }",
    );
    f.buffers = vec![
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(texture(iChannel0, fragCoord / iResolution.xy).r + 0.25, 0.0, 0.0, 1.0);
        }"
        .into(),
    ];
    state.render(&f).unwrap();
    state.render(&f).unwrap();
    assert_eq!(state.frames(), 2);
    let before = state.read_previous(0).unwrap();

    let mut broken = f.clone();
    broken.size = (16, 16); // would have recreated the targets had compilation not failed first
    broken.main = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(1.0);
    fragColor = nothing_here;
}"
    .into();
    let error = state.render(&broken).unwrap_err();
    assert!(error.contains("line 3"), "{error}");
    assert_eq!(state.frames(), 2);
    assert_eq!(
        state.feedback_size(),
        Some((64, 32)),
        "the feedback targets were not touched"
    );
    assert_eq!(state.read_previous(0).unwrap(), before);

    // A broken buffer program is reported with its own label and also touches nothing.
    let mut broken = f.clone();
    broken.buffers[0] = "void mainImage(out vec4 c, in vec2 p) { c = vec4(oops); }".into();
    let error = state.render(&broken).unwrap_err();
    assert!(error.starts_with("buffer 0: line 1"), "{error}");
    assert_eq!(state.read_previous(0).unwrap(), before);

    // The next good frame continues the history.
    let image = state.render(&f).unwrap();
    near(at(&image, 64, 20, 10)[2], 0.75 * 255.0);
}

#[test]
fn a8_compositing_on_the_gpu_follows_the_blend_formulas() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(0.5, 0.25, 0.0, 1.0); }",
    );
    f.size = (100, 100);
    // Light: straight (1, 0.5, 0), alpha 0.5 inside; the edge fade is 0 on the boundary.
    f.blend = BlendMode::Light;
    let image = state.render(&f).unwrap();
    let [b, g, r, a] = at(&image, 100, 50, 50);
    assert_eq!((b, r), (0, 255));
    near(g, 127.5);
    near(a, 127.5);
    assert_eq!(at(&image, 100, 0, 0)[3], 0, "the corner is faded out");
    assert!(
        at(&image, 100, 2, 50)[3] < at(&image, 100, 8, 50)[3],
        "the fade ramps up"
    );
    assert!(at(&image, 100, 50, 50)[3] >= at(&image, 100, 16, 50)[3]);
    // Light with black is transparent.
    let mut black = f.clone();
    black.main = "void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(0.0, 0.0, 0.0, 1.0); }".into();
    let image = state.render(&black).unwrap();
    assert_eq!(at(&image, 100, 50, 50), [0, 0, 0, 0]);
    // Opaque: alpha 1 everywhere, edges included, values clamped.
    let mut opaque = f.clone();
    opaque.blend = BlendMode::Opaque;
    opaque.main = "void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(3.0, 0.25, -1.0, 0.1); }".into();
    let image = state.render(&opaque).unwrap();
    for (x, y) in [(0, 0), (99, 99), (50, 50), (0, 50)] {
        let [b, g, r, a] = at(&image, 100, x, y);
        assert_eq!((b, r, a), (0, 255, 255), "({x}, {y})");
        near(g, 63.75);
    }
    // Alpha: the shader's alpha is the alpha, with the straight color.
    let mut alpha = f.clone();
    alpha.blend = BlendMode::Alpha;
    alpha.main = "void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(0.5, 0.5, 0.5, 0.5); }".into();
    let image = state.render(&alpha).unwrap();
    let [b, _, _, a] = at(&image, 100, 50, 50);
    near(b, 127.5);
    near(a, 127.5);
}

#[test]
fn bloom_spreads_values_above_one_into_the_surroundings() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            vec2 d = fragCoord - iResolution.xy * 0.5;
            fragColor = vec4(vec3(3.0) * step(length(d), 4.0), 1.0);
        }",
    );
    f.size = (96, 96);
    f.blend = BlendMode::Light;
    let plain = state.render(&f).unwrap();
    assert_eq!(
        at(&plain, 96, 48 + 12, 48)[3],
        0,
        "no light outside the disc without bloom"
    );
    assert!(
        at(&plain, 96, 48, 48)[3] > 200,
        "values above 1 clamp inside the disc"
    );
    f.bloom = 1.0;
    let bloomed = state.render(&f).unwrap();
    assert!(
        at(&bloomed, 96, 48 + 12, 48)[3] > 0,
        "the glow reaches outside the disc"
    );
    assert!(
        at(&bloomed, 96, 48 + 6, 48)[3] >= at(&bloomed, 96, 48 + 12, 48)[3],
        "and fades with distance"
    );
    // Strength is clamped to [0, 3].
    f.bloom = 100.0;
    assert!(state.render(&f).is_ok());
}

#[test]
fn uniforms_reach_the_program_mouse_time_camera_and_named_inputs() {
    require_gpu!();
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(iMouse.x / iResolution.x, iMouse.y / iResolution.y, gain, 1.0);
        }",
    );
    f.input_names = vec!["gain".into()];
    f.input_values = vec![0.5];
    f.mouse = [16.0, 8.0, -1.0, -1.0];
    let image = state.render(&f).unwrap();
    let [b, g, r, _] = at(&image, 64, 5, 5);
    near(r, 255.0 * 16.0 / 64.0);
    near(g, 255.0 * 8.0 / 32.0);
    near(b, 127.5);

    // Time inputs: the first frame has delta 0 and counter 0; the counter increments per frame.
    let mut t = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(iTime / 10.0, float(iFrame) / 10.0, iTimeDelta, 1.0) + vec4(iCamera.x * 0.0);
        }",
    );
    t.animated = true;
    t.time = 2.0;
    let mut state = self::state();
    let first = state.render(&t).unwrap();
    let [db, dg, dr, _] = at(&first, 64, 5, 5);
    near(dr, 255.0 * 0.2);
    near(dg, 0.0);
    near(db, 0.0);
    t.time = 3.0;
    let second = state.render(&t).unwrap();
    let [db, dg, dr, _] = at(&second, 64, 5, 5);
    near(dr, 255.0 * 0.3);
    near(dg, 255.0 * 0.1);
    near(db, 255.0 * 1.0);
}

#[test]
fn textures_resolve_by_name_and_a_missing_name_samples_as_transparent_black() {
    require_gpu!();
    let table = TextureTable::new();
    table.insert(
        "checker",
        TextureImage::from_rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
        .unwrap(),
    );
    let mut state = state();
    let mut f = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            vec2 uv = fragCoord / iResolution.xy;
            vec4 a = textureLod(iTexture0, uv, 0.0);
            vec4 b = textureLod(iTexture1, uv, 0.0);
            fragColor = vec4(a.rgb, 1.0) + vec4(0.0, 0.0, b.a, 0.0);
        }",
    );
    f.textures = table;
    f.texture_names = vec!["checker".into(), "missing".into()];
    let image = state.render(&f).unwrap();
    // The texture's first row is its top: the left half of the top row is red.
    // Sampled at texel centers (u = 0.25 and 0.75, v = 0.75 for the top row).
    let top_left = at(&image, 64, 16, 8);
    assert!(top_left[2] > 200 && top_left[1] < 60, "{top_left:?}");
    let top_right = at(&image, 64, 48, 8);
    assert!(top_right[1] > 200 && top_right[2] < 60, "{top_right:?}");
    assert!(
        top_left[0] < 30,
        "the missing texture is transparent black: {top_left:?}"
    );
    // The noise texture is not constant.
    let mut n = frame(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
            fragColor = vec4(texture(iNoise, fragCoord / 256.0).rgb, 1.0);
        }",
    );
    n.textures = Arc::new(TextureTable::default());
    let image = state.render(&n).unwrap();
    let distinct: std::collections::HashSet<[u8; 4]> =
        (0..64).map(|x| at(&image, 64, x, 5)).collect();
    assert!(
        distinct.len() > 32,
        "{} distinct noise pixels",
        distinct.len()
    );
}

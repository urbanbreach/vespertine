//! Compositing of shader output (CMP-026 R8, R9): the blend formulas, the edge fade and the
//! bloom constants. The GPU composite pass implements the same formulas; they are written here
//! in plain Rust so they are tested without a GPU.

/// Luma weights.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// Alpha at or below which a pixel is fully transparent.
pub const ALPHA_EPSILON: f32 = 0.0001;
/// Bloom strength range.
pub const BLOOM_RANGE: (f32, f32) = (0.0, 3.0);
/// Smoothstep knee of the bright pass.
pub const BRIGHT_KNEE: (f32, f32) = (0.6, 1.2);
/// Blur iterations.
pub const BLOOM_ITERATIONS: u32 = 3;
/// Center weight and tap weights of the separable blur (four taps each side).
pub const BLOOM_WEIGHTS: [f32; 5] = [0.227, 0.194, 0.121, 0.054, 0.016];
/// Tap spacing in texels.
pub const BLOOM_TAP_SPACING: f32 = 1.6;
/// Width of the edge fade as a fraction of the shorter side.
pub const FADE_FRACTION: f32 = 0.15;

/// How shader output sits on the content behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    /// Output rgb clamped to [0, 1], alpha 1, no edge fade.
    Opaque,
    /// The output is emitted light: alpha is the brightest channel (the default, because
    /// programs commonly write alpha 1 over black).
    #[default]
    Light,
    /// The shader's alpha (plus the glow's luma) is the alpha.
    Alpha,
}

/// The bloom strength clamped to [0, 3]; non-finite values disable bloom.
pub fn clamp_bloom(strength: f32) -> f32 {
    if strength.is_finite() {
        strength.clamp(BLOOM_RANGE.0, BLOOM_RANGE.1)
    } else {
        0.0
    }
}

/// Luma of a color.
pub fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// The quintic smoothstep `s^3 (s (6 s - 15) + 10)` of `d / width` clamped to [0, 1].
pub fn ease(distance: f32, width: f32) -> f32 {
    let s = (distance / width).clamp(0.0, 1.0);
    (s * s * s * (s * (6.0 * s - 15.0) + 10.0)).clamp(0.0, 1.0)
}

/// The edge fade at fragment position `p` (device pixels) of a surface of `size` device pixels:
/// `w = max(0.15 x min(width, height), 1)`; per axis `d = min(p, size - p)`; the product of the
/// eased distances. 0 on the boundary, 1 at distances of at least `w` from every edge.
pub fn edge_fade(p: (f32, f32), size: (f32, f32)) -> f32 {
    let width = (FADE_FRACTION * size.0.min(size.1)).max(1.0);
    let axis = |p: f32, size: f32| ease(p.min(size - p), width);
    axis(p.0, size.0) * axis(p.1, size.1)
}

/// The smoothstep of the bright pass.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The bright pass of the bloom: `c x smoothstep(0.6, 1.2, luma(c))`.
pub fn bright_pass(color: [f32; 3]) -> [f32; 3] {
    let over = smoothstep(BRIGHT_KNEE.0, BRIGHT_KNEE.1, luma(color));
    [color[0] * over, color[1] * over, color[2] * over]
}

/// Composes one pixel: `rgb` and `shader_alpha` are the shader output (16-bit float, so values
/// above 1 survive), `glow` the blurred bloom, `bloom` its strength and `fade` the edge fade.
/// Returns straight-alpha RGBA.
pub fn blend(
    mode: BlendMode,
    rgb: [f32; 3],
    shader_alpha: f32,
    glow: [f32; 3],
    bloom: f32,
    fade: f32,
) -> [f32; 4] {
    let bloom = clamp_bloom(bloom);
    let lit = |c: usize| glow[c] * bloom;
    match mode {
        BlendMode::Opaque => [
            rgb[0].clamp(0.0, 1.0),
            rgb[1].clamp(0.0, 1.0),
            rgb[2].clamp(0.0, 1.0),
            1.0,
        ],
        BlendMode::Light => {
            let c = [
                (rgb[0] + lit(0)).clamp(0.0, 1.0),
                (rgb[1] + lit(1)).clamp(0.0, 1.0),
                (rgb[2] + lit(2)).clamp(0.0, 1.0),
            ];
            let a = c[0].max(c[1]).max(c[2]);
            if a <= ALPHA_EPSILON {
                [0.0; 4]
            } else {
                [c[0] / a, c[1] / a, c[2] / a, a * fade]
            }
        }
        BlendMode::Alpha => {
            let a = (shader_alpha + luma([lit(0), lit(1), lit(2)])).clamp(0.0, 1.0);
            if a <= ALPHA_EPSILON {
                [0.0; 4]
            } else {
                let straight = |c: usize| {
                    ((rgb[c].clamp(0.0, 1.0) * shader_alpha + lit(c)) / a).clamp(0.0, 1.0)
                };
                [straight(0), straight(1), straight(2), a * fade]
            }
        }
    }
}

//! Compositing modes of shader output (CMP-026 R8, R9) and the bloom limits. The blend formulas,
//! the edge fade and the bright pass live in the WGSL of the GPU composite pass
//! ([`super::gpu`]); the GPU-output tests cover them.

/// Bloom strength range.
pub const BLOOM_RANGE: (f32, f32) = (0.0, 3.0);
/// Blur iterations.
pub const BLOOM_ITERATIONS: u32 = 3;

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

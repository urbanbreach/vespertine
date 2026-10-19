//! Pure (graphics-API free) math of the ordered backdrop blur (RND-003) and its
//! region-bounded, reusable-storage processing (PRF-009): filter parameters, snapshot envelope,
//! Gaussian kernel table, tap pairing, snapshot-set reuse and pass scissors.

/// Linux budget of effects per frame, counted in processing order (RND-003 R12).
pub const MAX_EFFECTS_PER_FRAME: usize = 32;
/// Snapshot allocation quantum in device pixels per dimension (PRF-009 R1).
pub const SNAPSHOT_QUANTUM: u32 = 256;
/// Largest radius whose weights are served from the precomputed table (PRF-009 R4).
pub const TABLE_MAX_RADIUS: u32 = 128;
/// Number of scalar slots of the kernel table as passed to the shaders (129 entries, padded).
pub const TABLE_SLOTS: usize = 132;
/// Consecutive effect-free rendered frames after which blur storage is released (PRF-009 R3).
pub const EFFECT_FREE_FRAME_LIMIT: u32 = 30;

/// An axis-aligned rectangle of pixels, `min` inclusive and `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl PixelRect {
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn area(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }
}

/// A float rectangle in device pixels, `min` and `max` corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl FloatRect {
    pub fn new(origin: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            x0: origin[0],
            y0: origin[1],
            x1: origin[0] + size[0],
            y1: origin[1] + size[1],
        }
    }

    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }
}

/// Parameters of the separable filter derived from the requested device sigma (RND-003 R7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    /// `max(requested sigma, 1)`.
    pub sigma: f32,
    /// Downsampling factor `d = clamp(floor(s / 8), 1, 4)`.
    pub downsample: u32,
    /// Effective sigma in intermediate texels `t = max(s / d, 0.5)`.
    pub effective_sigma: f32,
    /// Tap radius `r = ceil(3 t)`, never capped.
    pub radius: u32,
}

impl FilterParams {
    pub fn new(device_sigma: f32) -> Self {
        let sigma = f64::from(device_sigma).max(1.0);
        let downsample = ((sigma / 8.0).floor() as u32).clamp(1, 4);
        let effective = (sigma / f64::from(downsample)).max(0.5);
        // Guard against representation error just above an integer (e.g. 3 * 8.000000001).
        let radius = (3.0 * effective - 1e-9).ceil() as u32;
        Self {
            sigma: sigma as f32,
            downsample,
            effective_sigma: effective as f32,
            radius,
        }
    }

    /// Padding of the snapshot envelope on every side: `ceil(3 s) + 2`.
    pub fn envelope_padding(&self) -> u32 {
        (3.0 * f64::from(self.sigma) - 1e-9).ceil() as u32 + 2
    }
}

/// The snapshot envelope: the visible region `V` expanded by the padding, floored/ceiled and
/// clamped to the drawable. `None` when empty (RND-003 R6).
pub fn snapshot_envelope(
    visible: &FloatRect,
    padding: u32,
    drawable: (u32, u32),
) -> Option<PixelRect> {
    if visible.is_empty() {
        return None;
    }
    let p = padding as f32;
    let clamp = |value: f32, max: u32| value.clamp(0.0, max as f32) as u32;
    let rect = PixelRect {
        x0: clamp((visible.x0 - p).floor(), drawable.0),
        y0: clamp((visible.y0 - p).floor(), drawable.1),
        x1: clamp((visible.x1 + p).ceil(), drawable.0),
        y1: clamp((visible.y1 + p).ceil(), drawable.1),
    };
    (!rect.is_empty()).then_some(rect)
}

/// A symmetric Gaussian kernel for one effect, computed once (PRF-009 R4).
#[derive(Clone, Debug, PartialEq)]
pub struct Kernel {
    pub effective_sigma: f32,
    pub radius: u32,
    /// Weights for offsets `0..=128` normalized so `w(0) + 2 * sum w(o) = 1`, zero beyond the
    /// radius. All zero when the radius exceeds [`TABLE_MAX_RADIUS`].
    pub table: [f32; TABLE_SLOTS],
    /// Reciprocal of `sum_{j=-r..=r} exp(-j^2 / (2 t^2))`; used by the full-support fallback.
    pub inverse_norm: f32,
}

impl Kernel {
    pub fn new(effective_sigma: f32, radius: u32) -> Self {
        let t = f64::from(effective_sigma);
        let raw = |offset: u32| (-(f64::from(offset).powi(2)) / (2.0 * t * t)).exp();
        let mut norm = 1.0;
        for offset in 1..=radius {
            norm += 2.0 * raw(offset);
        }
        let mut table = [0.0_f32; TABLE_SLOTS];
        if radius <= TABLE_MAX_RADIUS {
            for offset in 0..=radius {
                table[offset as usize] = (raw(offset) / norm) as f32;
            }
        }
        Self {
            effective_sigma,
            radius,
            table,
            inverse_norm: (1.0 / norm) as f32,
        }
    }

    pub fn for_filter(filter: &FilterParams) -> Self {
        Self::new(filter.effective_sigma, filter.radius)
    }

    pub fn uses_table(&self) -> bool {
        self.radius <= TABLE_MAX_RADIUS
    }

    #[cfg(test)]
    /// The normalized weight for offset `o` (absolute value), evaluated directly rather than
    /// through the table. Used as the reference convolution and by the full-support fallback.
    pub fn direct_weight(&self, offset: u32) -> f64 {
        let t = f64::from(self.effective_sigma);
        (-(f64::from(offset).powi(2)) / (2.0 * t * t)).exp() * f64::from(self.inverse_norm)
    }
}

#[cfg(test)]
/// One bilinear tap of a paired kernel: a distance (in texels, relative to the center) at which
/// to sample on each side and the combined weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairedTap {
    pub distance: f32,
    pub weight: f32,
}

/// Pairs adjacent positive offsets `(k, k + 1)` for odd `k` into single bilinear fetches:
/// combined weight `w(k) + w(k + 1)` at distance `k + w(k + 1) / (w(k) + w(k + 1))`. A missing
/// final partner has weight zero. Only valid at source stride 1 (PRF-009 R5).
#[cfg(test)]
pub fn paired_taps(table: &[f32; TABLE_SLOTS], radius: u32) -> Vec<PairedTap> {
    let mut taps = Vec::with_capacity(radius as usize / 2 + 1);
    let mut k = 1;
    while k <= radius {
        let w1 = table[k as usize];
        let w2 = table[(k + 1) as usize];
        let weight = w1 + w2;
        let distance = k as f32 + if weight > 0.0 { w2 / weight } else { 0.0 };
        taps.push(PairedTap { distance, weight });
        k += 2;
    }
    taps
}

/// Scissors of the two filter passes in filter-target texels (PRF-009 R6, R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassScissors {
    pub horizontal: PixelRect,
    pub vertical: PixelRect,
}

fn axis_scissor(visible: (f32, f32), origin: f32, snapshot: u32, target: u32) -> (u32, u32) {
    let z = target as f32 / snapshot as f32;
    let start = ((visible.0 - origin) * z).floor() as i64 - 1;
    let end = ((visible.1 - origin) * z).ceil() as i64 + 1;
    let start = start.clamp(0, i64::from(target));
    let end = end.clamp(0, i64::from(target)).max(start);
    (start as u32, end as u32)
}

/// Computes both pass scissors. `visible` is the effect's visible rectangle in drawable device
/// coordinates, `snapshot_origin` the copy origin, `snapshot` and `target` the snapshot and
/// filter-target dimensions, `effective_sigma` the sigma `t` in filter texels.
pub fn pass_scissors(
    visible: &FloatRect,
    snapshot_origin: (u32, u32),
    snapshot: (u32, u32),
    target: (u32, u32),
    effective_sigma: f32,
) -> PassScissors {
    let (vx0, vx1) = axis_scissor(
        (visible.x0, visible.x1),
        snapshot_origin.0 as f32,
        snapshot.0,
        target.0,
    );
    let (vy0, vy1) = axis_scissor(
        (visible.y0, visible.y1),
        snapshot_origin.1 as f32,
        snapshot.1,
        target.1,
    );
    let vertical = PixelRect {
        x0: vx0,
        y0: vy0,
        x1: vx1,
        y1: vy1,
    };
    let halo_y = (3.0 * f64::from(effective_sigma) - 1e-9).ceil() as u32 + 1;
    let horizontal = PixelRect {
        x0: vx0.saturating_sub(1),
        y0: vy0.saturating_sub(halo_y),
        x1: (vx1 + 1).min(target.0),
        y1: (vy1 + halo_y).min(target.1),
    };
    PassScissors {
        horizontal,
        vertical,
    }
}

/// Shape of a retained blur set: snapshot dimensions and downsampling factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetShape {
    pub downsample: u32,
    pub width: u32,
    pub height: u32,
}

impl SetShape {
    /// Filter-target dimensions: `ceil(snapshot / downsample)`, at least 1.
    pub fn filter_size(&self) -> (u32, u32) {
        (
            self.width.div_ceil(self.downsample).max(1),
            self.height.div_ceil(self.downsample).max(1),
        )
    }
}

/// Whether a retained set can serve a request (PRF-009 R1 a-c).
pub fn set_is_reusable(
    set: &SetShape,
    downsample: u32,
    required: (u32, u32),
    drawable: (u32, u32),
) -> bool {
    set.downsample == downsample
        && set.width >= required.0
        && set.height >= required.1
        && set.width <= drawable.0
        && set.height <= drawable.1
}

/// Snapshot dimensions of a new set: `ceil(required / 256) * 256` per axis, clamped to the
/// drawable and at least 1.
pub fn new_set_shape(downsample: u32, required: (u32, u32), drawable: (u32, u32)) -> SetShape {
    let quantize = |value: u32, max: u32| {
        (value.div_ceil(SNAPSHOT_QUANTUM) * SNAPSHOT_QUANTUM)
            .min(max)
            .max(1)
    };
    SetShape {
        downsample,
        width: quantize(required.0, drawable.0),
        height: quantize(required.1, drawable.1),
    }
}

/// The copy origin of the snapshot: no farther right/down than `drawable - snapshot` on each
/// axis while still covering the envelope (PRF-009 R2, RND-003 R13).
pub fn snapshot_origin(
    envelope: &PixelRect,
    snapshot: (u32, u32),
    drawable: (u32, u32),
) -> (u32, u32) {
    (
        envelope.x0.min(drawable.0.saturating_sub(snapshot.0)),
        envelope.y0.min(drawable.1.saturating_sub(snapshot.1)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn filter_parameters_match_spec_examples() {
        let f = FilterParams::new(11.0);
        assert_eq!((f.downsample, f.effective_sigma, f.radius), (1, 11.0, 33));
        assert_eq!(FilterParams::new(15.9).downsample, 1);
        let f = FilterParams::new(16.0);
        assert_eq!((f.downsample, f.effective_sigma, f.radius), (2, 8.0, 24));
        let f = FilterParams::new(24.0);
        assert_eq!((f.downsample, f.effective_sigma, f.radius), (3, 8.0, 24));
        let f = FilterParams::new(32.0);
        assert_eq!((f.downsample, f.effective_sigma, f.radius), (4, 8.0, 24));
        assert_eq!(FilterParams::new(500.0).downsample, 4);
        let f = FilterParams::new(1.4);
        assert_eq!((f.downsample, f.effective_sigma, f.radius), (1, 1.4, 5));
        let f = FilterParams::new(0.3);
        assert_eq!(f.sigma, 1.0);
        assert_eq!(f.radius, 3);
        // The radius is never capped.
        assert_eq!(FilterParams::new(400.0).radius, 300);
    }

    #[test]
    fn envelope_padding_for_sigma_16_is_fifty() {
        assert_eq!(FilterParams::new(16.0).envelope_padding(), 50);
    }

    #[test]
    fn envelope_is_expanded_floored_ceiled_and_clamped() {
        let visible = FloatRect::new([100.4, 50.6], [10.0, 10.0]);
        let envelope = snapshot_envelope(&visible, 50, (1000, 1000)).unwrap();
        assert_eq!(
            envelope,
            PixelRect {
                x0: 50,
                y0: 0,
                x1: 161,
                y1: 111
            }
        );
        let touching = FloatRect::new([0.0, 0.0], [1000.0, 1000.0]);
        let envelope = snapshot_envelope(&touching, 10, (1000, 1000)).unwrap();
        assert_eq!((envelope.x0, envelope.x1, envelope.y1), (0, 1000, 1000));
        let outside = FloatRect::new([2000.0, 0.0], [10.0, 10.0]);
        assert_eq!(snapshot_envelope(&outside, 4, (1000, 1000)), None);
        let empty = FloatRect::new([5.0, 5.0], [0.0, 3.0]);
        assert_eq!(snapshot_envelope(&empty, 4, (1000, 1000)), None);
    }

    #[test]
    fn weights_are_normalized_and_zero_beyond_radius() {
        for sigma in [0.5_f32, 0.75, 1.0, 3.2, 8.0, 11.0, 16.0, 32.0, 42.5] {
            let radius = (3.0 * f64::from(sigma) - 1e-9).ceil() as u32;
            let kernel = Kernel::new(sigma, radius);
            let sum: f64 = f64::from(kernel.table[0])
                + 2.0
                    * (1..=radius as usize)
                        .map(|o| f64::from(kernel.table[o]))
                        .sum::<f64>();
            assert!(approx(sum, 1.0, 2e-6), "sigma {sigma}: sum {sum}");
            for offset in (radius as usize + 1)..TABLE_SLOTS {
                assert_eq!(kernel.table[offset], 0.0);
            }
            for offset in 0..=radius {
                assert!(
                    approx(
                        f64::from(kernel.table[offset as usize]),
                        kernel.direct_weight(offset),
                        2e-6
                    ),
                    "sigma {sigma} offset {offset}"
                );
            }
        }
    }

    #[test]
    fn oversized_radius_skips_the_table() {
        let kernel = Kernel::for_filter(&FilterParams::new(400.0));
        assert_eq!(kernel.effective_sigma, 100.0);
        assert_eq!(kernel.radius, 300);
        assert!(!kernel.uses_table());
        assert!(kernel.table.iter().all(|w| *w == 0.0));
        let sum: f64 = (0..=300u32)
            .map(|o| kernel.direct_weight(o) * if o == 0 { 1.0 } else { 2.0 })
            .sum();
        assert!(approx(sum, 1.0, 1e-6));
    }

    /// Reference clamp-to-edge convolution of `signal` with the direct weights.
    fn convolve_direct(signal: &[f64], kernel: &Kernel, index: usize) -> f64 {
        let last = signal.len() as i64 - 1;
        let mut acc = 0.0;
        for j in -(kernel.radius as i64)..=(kernel.radius as i64) {
            let at = (index as i64 + j).clamp(0, last) as usize;
            acc += signal[at] * kernel.direct_weight(j.unsigned_abs() as u32);
        }
        acc
    }

    /// Convolution with paired taps using linear interpolation and clamp-to-edge addressing.
    fn convolve_paired(signal: &[f64], kernel: &Kernel, index: usize) -> f64 {
        let last = signal.len() as i64 - 1;
        let sample = |position: f64| {
            // Texel centers sit at integer positions here.
            let base = position.floor();
            let frac = position - base;
            let a = signal[(base as i64).clamp(0, last) as usize];
            let b = signal[(base as i64 + 1).clamp(0, last) as usize];
            a * (1.0 - frac) + b * frac
        };
        let mut acc = signal[index] * f64::from(kernel.table[0]);
        for tap in paired_taps(&kernel.table, kernel.radius) {
            let d = f64::from(tap.distance);
            acc += f64::from(tap.weight) * (sample(index as f64 + d) + sample(index as f64 - d));
        }
        acc
    }

    #[test]
    fn paired_and_unpaired_agree_including_edges() {
        let signal: Vec<f64> = (0..97)
            .map(|i| ((i * 37 + 11) % 101) as f64 / 100.0)
            .collect();
        for sigma in [0.5_f32, 0.75, 1.0, 3.2, 8.0, 11.0, 16.0, 32.0, 42.5] {
            let radius = (3.0 * f64::from(sigma) - 1e-9).ceil() as u32;
            let kernel = Kernel::new(sigma, radius);
            for index in [0, 1, 2, 48, 94, 95, 96] {
                let direct = convolve_direct(&signal, &kernel, index);
                let paired = convolve_paired(&signal, &kernel, index);
                assert!(
                    approx(direct, paired, 2e-6),
                    "sigma {sigma} index {index}: {direct} vs {paired}"
                );
            }
        }
    }

    #[test]
    fn odd_radius_final_partner_has_zero_weight() {
        let kernel = Kernel::new(1.4, 5);
        let taps = paired_taps(&kernel.table, 5);
        assert_eq!(taps.len(), 3);
        let last = taps[2];
        assert_eq!(last.weight, kernel.table[5]);
        assert_eq!(last.distance, 5.0);
    }

    #[test]
    fn new_sets_are_quantized_and_clamped() {
        let shape = new_set_shape(2, (300, 10), (1800, 1000));
        assert_eq!((shape.width, shape.height), (512, 256));
        let shape = new_set_shape(1, (1700, 990), (1800, 1000));
        assert_eq!((shape.width, shape.height), (1792, 1000));
        let shape = new_set_shape(1, (1795, 995), (1800, 1000));
        assert_eq!((shape.width, shape.height), (1800, 1000));
        let shape = new_set_shape(1, (1, 1), (100, 100));
        assert_eq!((shape.width, shape.height), (100, 100));
        assert_eq!(shape.filter_size(), (100, 100));
        let shape = SetShape {
            downsample: 4,
            width: 1279,
            height: 799,
        };
        assert_eq!(shape.filter_size(), (320, 200));
    }

    #[test]
    fn reuse_requires_factor_size_and_drawable_fit() {
        let set = SetShape {
            downsample: 2,
            width: 512,
            height: 256,
        };
        assert!(set_is_reusable(&set, 2, (500, 200), (1800, 1000)));
        assert!(!set_is_reusable(&set, 1, (500, 200), (1800, 1000)));
        assert!(!set_is_reusable(&set, 2, (513, 200), (1800, 1000)));
        assert!(!set_is_reusable(&set, 2, (500, 257), (1800, 1000)));
        // Drawable shrank below the retained snapshot.
        assert!(!set_is_reusable(&set, 2, (500, 200), (511, 1000)));
    }

    #[test]
    fn copy_origin_is_shifted_inward() {
        let envelope = PixelRect {
            x0: 1700,
            y0: 10,
            x1: 1790,
            y1: 60,
        };
        let origin = snapshot_origin(&envelope, (512, 256), (1800, 1000));
        assert_eq!(origin, (1288, 10));
        assert!(origin.0 + 512 >= envelope.x1);
    }

    #[test]
    fn scissors_cover_composite_neighbors_and_vertical_taps() {
        for (snapshot, d, t) in [
            ((1279_u32, 799_u32), 4_u32, 8.0_f32),
            ((17, 13), 1, 3.2),
            ((300, 200), 3, 8.0),
        ] {
            let target = (snapshot.0.div_ceil(d).max(1), snapshot.1.div_ceil(d).max(1));
            let origin = (3u32, 2u32);
            let visible = FloatRect::new(
                [origin.0 as f32 + 4.3, origin.1 as f32 + 3.7],
                [snapshot.0 as f32 / 3.0, snapshot.1 as f32 / 4.0],
            );
            let s = pass_scissors(&visible, origin, snapshot, target, t);
            let z = (
                target.0 as f32 / snapshot.0 as f32,
                target.1 as f32 / snapshot.1 as f32,
            );
            let radius = (3.0 * f64::from(t) - 1e-9).ceil() as i64;
            // Every composite pixel center in the visible rectangle reads two bilinear neighbors
            // per axis of the vertical output.
            let mut y = visible.y0.floor() + 0.5;
            while y < visible.y1 {
                if y >= visible.y0 {
                    let sy = (y - origin.1 as f32) * z.1 - 0.5;
                    for ty in [sy.floor() as i64, sy.floor() as i64 + 1] {
                        let ty = ty.clamp(0, i64::from(target.1) - 1);
                        assert!(ty >= i64::from(s.vertical.y0) && ty < i64::from(s.vertical.y1));
                        for k in -radius..=radius {
                            let ry = (ty + k).clamp(0, i64::from(target.1) - 1);
                            assert!(
                                ry >= i64::from(s.horizontal.y0) && ry < i64::from(s.horizontal.y1),
                                "row {ry} outside {:?}",
                                s.horizontal
                            );
                        }
                    }
                }
                y += 1.0;
            }
            let mut x = visible.x0.floor() + 0.5;
            while x < visible.x1 {
                if x >= visible.x0 {
                    let sx = (x - origin.0 as f32) * z.0 - 0.5;
                    for tx in [sx.floor() as i64, sx.floor() as i64 + 1] {
                        let tx = tx.clamp(0, i64::from(target.0) - 1);
                        assert!(tx >= i64::from(s.vertical.x0) && tx < i64::from(s.vertical.x1));
                        assert!(
                            tx >= i64::from(s.horizontal.x0) && tx < i64::from(s.horizontal.x1)
                        );
                    }
                }
                x += 1.0;
            }
        }
    }

    #[test]
    fn scissors_are_translation_invariant() {
        let a = pass_scissors(
            &FloatRect::new([50.25, 40.5], [100.0, 20.0]),
            (10, 10),
            (300, 200),
            (150, 100),
            8.0,
        );
        let b = pass_scissors(
            &FloatRect::new([80.25, 70.5], [100.0, 20.0]),
            (40, 40),
            (300, 200),
            (150, 100),
            8.0,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn narrow_regions_shade_fewer_texels_than_the_full_target() {
        let s = pass_scissors(
            &FloatRect::new([200.0, 500.0], [700.0, 40.0]),
            (100, 400),
            (900, 300),
            (450, 150),
            8.0,
        );
        let full = 450u64 * 150;
        assert!(s.vertical.area() < full / 2);
        assert!(s.horizontal.area() < full);
        assert!(s.horizontal.area() >= s.vertical.area());
    }

    #[test]
    fn zero_area_scissor_has_end_at_least_start() {
        let s = pass_scissors(
            &FloatRect::new([5000.0, 0.0], [10.0, 10.0]),
            (0, 0),
            (100, 100),
            (50, 50),
            8.0,
        );
        assert!(s.vertical.is_empty());
        assert!(s.vertical.x1 >= s.vertical.x0);
    }
}

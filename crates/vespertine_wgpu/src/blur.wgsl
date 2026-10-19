// Ordered backdrop blur (RND-003, PRF-009): separable Gaussian filter passes and the
// replacing composite. Standalone module; every pass reads one 2D texture through one
// dynamically-offset parameter block, so every effect keeps its own parameters.

struct BlurParams {
    // Source-uv distance of one tap offset (horizontal or vertical).
    step: vec2<f32>,
    // Dimensions of the render target of a filter pass, in texels.
    out_size: vec2<f32>,
    radius: u32,
    // 1 when adjacent taps may be paired into bilinear fetches (source stride of one texel).
    paired: u32,
    // Effective sigma in filter texels (used by the full-support fallback only).
    effective_sigma: f32,
    // Reciprocal of the sum of all unnormalized weights (fallback only).
    inverse_norm: f32,
    // Effect rectangle (x, y, width, height) in device pixels.
    bounds: vec4<f32>,
    // Content mask (x, y, width, height) in device pixels.
    content_mask: vec4<f32>,
    // Corner radii: top-left, top-right, bottom-right, bottom-left.
    radii: vec4<f32>,
    snapshot_origin: vec2<f32>,
    snapshot_size: vec2<f32>,
    viewport: vec2<f32>,
    pad: vec2<f32>,
    // Pixels covered by the composite draw (x0, y0, x1, y1).
    draw_rect: vec4<f32>,
    // Normalized weights for offsets 0..=128 (132 packed scalar slots plus padding).
    table: array<vec4<f32>, 33>,
}

@group(0) @binding(0) var<uniform> params: BlurParams;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

fn sample_source(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(source, source_sampler, uv, 0.0);
}

fn table_weight(offset: u32) -> f32 {
    return params.table[offset >> 2u][offset & 3u];
}

struct FilterVarying {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_filter(@builtin(vertex_index) vertex_id: u32) -> FilterVarying {
    // One triangle covering the whole target.
    let x = select(-1.0, 3.0, vertex_id == 1u);
    let y = select(-1.0, 3.0, vertex_id == 2u);
    var out = FilterVarying();
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_filter(input: FilterVarying) -> @location(0) vec4<f32> {
    let uv = input.position.xy / params.out_size;
    let radius = params.radius;

    if (radius > 128u) {
        // Full support: exponential weights per offset, normalized by the precomputed sum.
        let t = params.effective_sigma;
        var acc = sample_source(uv) * params.inverse_norm;
        for (var k = 1u; k <= radius; k += 1u) {
            let kf = f32(k);
            let w = exp(-(kf * kf) / (2.0 * t * t)) * params.inverse_norm;
            acc += w * (sample_source(uv + params.step * kf) + sample_source(uv - params.step * kf));
        }
        return acc;
    }

    var acc = sample_source(uv) * table_weight(0u);
    if (params.paired != 0u) {
        for (var k = 1u; k <= radius; k += 2u) {
            let w1 = table_weight(k);
            let w2 = table_weight(k + 1u);
            let w = w1 + w2;
            var distance = f32(k);
            if (w > 0.0) {
                distance += w2 / w;
            }
            let offset = params.step * distance;
            acc += w * (sample_source(uv + offset) + sample_source(uv - offset));
        }
    } else {
        for (var k = 1u; k <= radius; k += 1u) {
            let w = table_weight(k);
            let offset = params.step * f32(k);
            acc += w * (sample_source(uv + offset) + sample_source(uv - offset));
        }
    }
    return acc;
}

struct CompositeVarying {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_composite(@builtin(vertex_index) vertex_id: u32) -> CompositeVarying {
    let unit = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let position = mix(params.draw_rect.xy, params.draw_rect.zw, unit);
    let ndc = position / params.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    var out = CompositeVarying();
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    return out;
}

fn pick_radius(center_to_point: vec2<f32>) -> f32 {
    if (center_to_point.x < 0.0) {
        if (center_to_point.y < 0.0) {
            return params.radii.x;
        }
        return params.radii.w;
    }
    if (center_to_point.y < 0.0) {
        return params.radii.y;
    }
    return params.radii.z;
}

@fragment
fn fs_composite(input: CompositeVarying) -> @location(0) vec4<f32> {
    let p = input.position.xy;
    let mask = params.content_mask;
    if (p.x - mask.x < 0.0 || mask.x + mask.z - p.x < 0.0
        || p.y - mask.y < 0.0 || mask.y + mask.w - p.y < 0.0) {
        discard;
    }

    // Hard cutoff at signed distance zero: no antialiasing ramp.
    let half_size = params.bounds.zw / 2.0;
    let center = params.bounds.xy + half_size;
    let center_to_point = p - center;
    let q = pick_radius(center_to_point);
    let e = abs(center_to_point) - half_size + vec2<f32>(q);
    let distance = length(max(e, vec2<f32>(0.0))) + min(max(e.x, e.y), 0.0) - q;
    if (distance > 0.0) {
        discard;
    }

    let uv = (p - params.snapshot_origin) / params.snapshot_size;
    return sample_source(uv);
}

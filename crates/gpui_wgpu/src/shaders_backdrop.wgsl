// Backdrop blur, ported from the DirectX renderer's `shaders.hlsl`. Every pass
// draws one quad from `target_rect` (device pixels) while reading the source
// texture at `source_rect`, clamped to `clamp_rect` so taps never leave the
// region that was written.

struct BackdropParams {
    target_rect: vec4<f32>,
    source_rect: vec4<f32>,
    clamp_rect: vec4<f32>,
    bounds: vec4<f32>,
    content_mask: vec4<f32>,
    corner_radii: vec4<f32>,
    viewport_size: vec2<f32>,
    source_size: vec2<f32>,
    direction: vec2<f32>,
    sigma: f32,
    saturation: f32,
    opacity: f32,
    block_size: f32,
    premultiplied_alpha: u32,
    pad: u32,
}

@group(0) @binding(0) var<uniform> params: BackdropParams;
@group(1) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

struct BackdropVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) source_position: vec2<f32>,
}

@vertex
fn vs_backdrop(@builtin(vertex_index) vertex_id: u32) -> BackdropVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let target_point = params.target_rect.xy + unit_vertex * params.target_rect.zw;
    var out: BackdropVarying;
    out.position = vec4<f32>(
        target_point / params.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0),
        0.0,
        1.0,
    );
    out.source_position = params.source_rect.xy + unit_vertex * params.source_rect.zw;
    return out;
}

fn sample_backdrop(source_pixel: vec2<f32>) -> vec4<f32> {
    let clamped = clamp(
        source_pixel,
        params.clamp_rect.xy + 0.5,
        params.clamp_rect.xy + params.clamp_rect.zw - 0.5,
    );
    return textureSampleLevel(source_texture, source_sampler, clamped / params.source_size, 0.0);
}

// Averages the block of full-size pixels behind one small pixel with a 4x4
// grid of bilinear taps (each tap already averages 2x2 pixels).
@fragment
fn fs_backdrop_downsample(input: BackdropVarying) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var row = 0; row < 4; row++) {
        for (var column = 0; column < 4; column++) {
            let offset = (vec2<f32>(f32(column), f32(row)) + 0.5) / 4.0 - 0.5;
            sum += sample_backdrop(input.source_position + offset * params.block_size);
        }
    }
    return sum / 16.0;
}

@fragment
fn fs_backdrop_blur(input: BackdropVarying) -> @location(0) vec4<f32> {
    let reach = min(i32(ceil(params.sigma * 3.0)), 32);
    let denominator = 2.0 * params.sigma * params.sigma;
    var sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var tap = -reach; tap <= reach; tap++) {
        let weight = exp(-f32(tap * tap) / denominator);
        sum += sample_backdrop(input.source_position + params.direction * f32(tap)) * weight;
        weight_sum += weight;
    }
    return sum / weight_sum;
}

fn backdrop_corner_radius(center_to_point: vec2<f32>) -> f32 {
    if (center_to_point.x < 0.0) {
        return select(params.corner_radii.w, params.corner_radii.x, center_to_point.y < 0.0);
    }
    return select(params.corner_radii.z, params.corner_radii.y, center_to_point.y < 0.0);
}

fn backdrop_sdf(point: vec2<f32>) -> f32 {
    let half_size = params.bounds.zw / 2.0;
    let center_to_point = point - (params.bounds.xy + half_size);
    let corner_radius = backdrop_corner_radius(center_to_point);
    let corner_center_to_point = abs(center_to_point) - half_size + corner_radius;
    if (corner_radius == 0.0) {
        return max(corner_center_to_point.x, corner_center_to_point.y);
    }
    let signed_distance_to_inset_quad =
        length(max(vec2<f32>(0.0), corner_center_to_point)) +
        min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));
    return signed_distance_to_inset_quad - corner_radius;
}

// CSS `saturate()`: the filter-effects luminance matrix, clamped like Chromium.
fn saturate_backdrop(color: vec3<f32>, amount: f32) -> vec3<f32> {
    return clamp(
        vec3<f32>(
            dot(vec3<f32>(0.213 + 0.787 * amount, 0.715 - 0.715 * amount, 0.072 - 0.072 * amount), color),
            dot(vec3<f32>(0.213 - 0.213 * amount, 0.715 + 0.285 * amount, 0.072 - 0.072 * amount), color),
            dot(vec3<f32>(0.213 - 0.213 * amount, 0.715 - 0.715 * amount, 0.072 + 0.928 * amount), color),
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

@fragment
fn fs_backdrop_composite(input: BackdropVarying) -> @location(0) vec4<f32> {
    let position = input.position.xy;
    let mask_end = params.content_mask.xy + params.content_mask.zw;
    if (any(position < params.content_mask.xy) || any(position > mask_end)) {
        discard;
    }
    let coverage = clamp(0.5 - backdrop_sdf(position), 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    let color = sample_backdrop(input.source_position);
    let alpha = coverage * params.opacity;
    let multiplier = select(1.0, alpha, params.premultiplied_alpha != 0u);
    return vec4<f32>(saturate_backdrop(color.rgb, params.saturation) * multiplier, alpha);
}

// Copies the offscreen frame to the window surface unchanged.
@fragment
fn fs_backdrop_blit(input: BackdropVarying) -> @location(0) vec4<f32> {
    return textureLoad(source_texture, vec2<i32>(input.position.xy), 0);
}

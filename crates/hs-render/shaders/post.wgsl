// HoloScope — post chain: bloom down/up (dual-filter-ish) + composite/tonemap.
// Single bind-group layout: {0: PostU, 1: src_tex, 2: sampler, 3: b_tex}

struct PostU {
    bloom: f32,
    exposure: f32,
    time: f32,
    scanlines: f32,
    vignette: f32,
    _pad0: f32,
    res: vec2<f32>,
    edl_on: f32,
    edl_strength: f32,
    edl_radius: f32,
    bg_mode: f32,
};

@group(0) @binding(0) var<uniform> U: PostU;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;
@group(0) @binding(3) var b_tex: texture_2d<f32>;
@group(0) @binding(4) var depth_tex: texture_depth_2d;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: VOut;
    o.pos = vec4<f32>(p[vi], 0.0, 1.0);
    // texture-space uv (v measured from top), valid for any render target
    o.uv = vec2<f32>((p[vi].x + 1.0) * 0.5, (1.0 - p[vi].y) * 0.5);
    return o;
}

// ---- bloom downsample: 5-tap ----
@fragment
fn fs_down(@location(0) uv_in: vec2<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src_tex));
    let t = 1.0 / dims;
    let uv = clamp(uv_in, vec2<f32>(0.0), vec2<f32>(1.0));
    var c = textureSample(src_tex, src_smp, uv) * 0.5;
    c = c + textureSample(src_tex, src_smp, uv + vec2<f32>(-t.x, -t.y)) * 0.125;
    c = c + textureSample(src_tex, src_smp, uv + vec2<f32>(t.x, -t.y)) * 0.125;
    c = c + textureSample(src_tex, src_smp, uv + vec2<f32>(-t.x, t.y)) * 0.125;
    c = c + textureSample(src_tex, src_smp, uv + vec2<f32>(t.x, t.y)) * 0.125;
    return c;
}

// ---- bloom upsample: tent(prev) + current down mip ----
@fragment
fn fs_up(@location(0) uv_in: vec2<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src_tex));
    let t = 1.0 / dims;
    let uv = clamp(uv_in, vec2<f32>(0.0), vec2<f32>(1.0));
    var a = textureSample(src_tex, src_smp, uv + vec2<f32>(-t.x, -t.y)) * 0.25;
    a = a + textureSample(src_tex, src_smp, uv + vec2<f32>(t.x, -t.y)) * 0.25;
    a = a + textureSample(src_tex, src_smp, uv + vec2<f32>(-t.x, t.y)) * 0.25;
    a = a + textureSample(src_tex, src_smp, uv + vec2<f32>(t.x, t.y)) * 0.25;
    let b = textureSample(b_tex, src_smp, uv);
    return a * 0.65 + b * 0.55;
}

// ---- composite: hdr + bloom → tonemap → vignette/scanlines → srgb view ----
fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

// eye-dome lighting: 8-neighbour log-depth contrast (plan §8.4)
fn edl_shade(uv: vec2<f32>) -> f32 {
    let px = vec2<i32>(uv * U.res);
    let r = i32(max(U.edl_radius, 0.5));
    let zc = textureLoad(depth_tex, px, 0);
    if (zc >= 1.0) {
        return 1.0;
    }
    let ec = -log2(max(zc, 1e-7));
    var resp = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            if (dx == 0 && dy == 0) {
                continue;
            }
            let zn = textureLoad(depth_tex, px + vec2<i32>(dx, dy) * r, 0);
            let en = -log2(max(zn, 1e-7));
            resp = resp + max(0.0, ec - en);
        }
    }
    return exp(-resp * U.edl_strength);
}

@fragment
fn fs_composite(@location(0) uv_in: vec2<f32>) -> @location(0) vec4<f32> {
    let uv = clamp(uv_in, vec2<f32>(0.0), vec2<f32>(1.0));
    var c = textureSample(src_tex, src_smp, uv).rgb;
    c = c + textureSample(b_tex, src_smp, uv).rgb * U.bloom;
    c = c * U.exposure;
    if (U.edl_on > 0.5) {
        c = c * edl_shade(uv);
    }
    // ACES filmic fit
    let x = c;
    c = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    let d = uv - 0.5;
    let vig = 1.0 - dot(d, d) * 1.1 * U.vignette;
    let sl = 1.0 - U.scanlines * 0.05 * (0.5 + 0.5 * sin(uv.y * U.res.y * 3.14159265));
    // faint temporal noise (CRT shimmer)
    let n = fract(sin(dot(uv * U.res + vec2<f32>(U.time * 61.7, U.time * 47.3), vec2<f32>(12.9898, 78.233))) * 43758.5453);
    c = c * vig * sl + (n - 0.5) * 0.012;
    // background modes where nothing was drawn (plan §10.2)
    let zc = textureLoad(depth_tex, vec2<i32>(uv * U.res), 0);
    if (zc >= 1.0) {
        var bg = vec3<f32>(0.0);
        if (U.bg_mode > 1.5 && U.bg_mode < 2.5) {
            // starfield
            let g = hash21(floor(uv * U.res / 2.0));
            let tw = 0.6 + 0.4 * sin(U.time * 2.0 + g * 40.0);
            bg = vec3<f32>(step(0.9975, g)) * tw * 0.8;
        } else if (U.bg_mode > 2.5) {
            // gradient dusk
            bg = mix(vec3<f32>(0.004, 0.010, 0.020), vec3<f32>(0.010, 0.030, 0.055), pow(1.0 - uv.y, 1.5));
        }
        c = c + bg;
    }
    return vec4<f32>(max(c, vec3<f32>(0.0)), 1.0);
}

// HoloScope — point sprite pass (plan §8.2/§8.3).
// Instanced quads, additive in Holo mode. No sorting needed.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    focal_px: f32,
    time: f32,
    point_size_px: f32,
    min_px: f32,
    max_px: f32,
    pixel_to_clip: f32,
    intensity_mul: f32,
    emissive_mul: f32,
    z_exag: f32,
    sweep_pos: f32,
    sweep_on: f32,
    hardness: f32,
};

struct NodeU {
    origin_view: vec4<f32>,
    quant_scale: f32,
    fade_near: f32,
    fade_far: f32,
    size_mul: f32,
    tier_fade: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<storage, read> NODES: array<NodeU>;
@group(0) @binding(2) var<storage, read> LUT: array<vec4<f32>>;

const QUAD: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, 1.0),
);

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) col: vec3<f32>,
    @location(2) alpha: f32,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) ipos: vec4<i32>,
    @location(1) iattr: vec4<u32>,
    @location(2) node_id: u32,
) -> VOut {
    let corner = QUAD[vi];
    let n = NODES[node_id];
    let lp = vec3<f32>(ipos.xyz) * n.quant_scale;
    var world = n.origin_view.xyz + lp;
    world.z = world.z * G.z_exag;
    let view = G.view * vec4<f32>(world, 1.0);
    var clip = G.view_proj * vec4<f32>(world, 1.0);
    let dist = max(-view.z, 0.05);
    let px = clamp(G.point_size_px * n.size_mul * (G.focal_px / dist), G.min_px, G.max_px);
    clip = vec4<f32>(clip.xy + corner * px * G.pixel_to_clip * clip.w, clip.z, clip.w);
    var o: VOut;
    o.clip = clip;
    o.uv = corner;
    var c = LUT[iattr.x].rgb;
    if (iattr.z & 1u) != 0u {
        c = c * G.emissive_mul;
    }
    // scan-sweep: animated vertical plane boosts emissive as it passes (§7.4)
    let sw = exp(-pow((world.x - G.sweep_pos) / 45.0, 2.0)) * G.sweep_on;
    c = c * (1.0 + sw * 2.2);
    o.col = c;
    let inten = f32(iattr.w) / 255.0;
    // near-distance fade: additive blending saturates inside ~10 m of the
    // camera (street-level LiDAR densities); fade in from 1.5→12 m
    let close_fade = smoothstep(1.5, 12.0, dist);
    o.alpha = n.tier_fade * inten * G.intensity_mul * (1.0 + sw * 0.8)
        * smoothstep(n.fade_far, n.fade_near, dist) * close_fade;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    let r2 = dot(i.uv, i.uv);
    if (r2 > 1.0) {
        discard;
    }
    let soft = exp(-4.0 * r2) + 0.15 * (1.0 - r2); // core + faint halo
    let hard = 1.0 - smoothstep(0.35, 1.0, r2);     // crisp LiDAR disc
    let w = mix(soft, hard, G.hardness);
    let a = w * i.alpha;
    return vec4<f32>(i.col * a, a);
}

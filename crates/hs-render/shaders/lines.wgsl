// HoloScope — additive line / world-quad pass (cage, outline, beams, diamonds).

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
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(2) var<storage, read> LUT: array<vec4<f32>>;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) col: vec4<f32>,
};

@vertex
fn vs(
    @location(0) pos: vec3<f32>,
    @location(1) col: vec4<f32>,
    @location(2) slot: u32,
) -> VOut {
    var world = pos;
    world.z = world.z * G.z_exag;
    var o: VOut;
    o.clip = G.view_proj * vec4<f32>(world, 1.0);
    o.col = vec4<f32>(LUT[slot].rgb * col.rgb, col.a) * G.intensity_mul;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.col.rgb * i.col.a, i.col.a);
}

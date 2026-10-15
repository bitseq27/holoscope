//! GPU/CPU record layouts (plan §6.1).

use bytemuck::{Pod, Zeroable};

/// 12-byte authoritative point record (disk + synthesis side).
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuPoint {
    pub pos: [i16; 3],
    pub color: u8,
    pub layer: u8,
    pub flags: u8,
    pub intensity: u8,
}

pub const FLAG_EMISSIVE: u8 = 1 << 0;
pub const FLAG_FADE_GROUP: u8 = 1 << 1;

/// 16-byte GPU instance record: Sint16x4 + Uint8x4 + Uint32 vertex attributes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ImplVertex {
    pub pos: [i16; 4],
    pub rgba: [u8; 4], // color, layer, flags, intensity
    pub node_id: u32,
}

/// Per-node uniform, mirrors `NodeU` in points.wgsl (48 B, 16-aligned).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct NodeU {
    pub origin_view: [f32; 4],
    pub quant_scale: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub size_mul: f32,
    pub tier_fade: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

/// Additive line/quad vertex (32 B): col = multiplier+alpha, slot = LUT index
/// (slot 80 = white, so baked furniture colours pass through unchanged).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct LineVert {
    pub pos: [f32; 3],
    pub col: [f32; 4],
    pub slot: u32,
}

pub type QuadVert = LineVert;

/// Per-frame globals, mirrors `Globals` in shaders (176 B).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub focal_px: f32,
    pub time: f32,
    pub point_size_px: f32,
    pub min_px: f32,
    pub max_px: f32,
    pub pixel_to_clip: f32,
    pub intensity_mul: f32,
    pub emissive_mul: f32,
    pub z_exag: f32,
    pub sweep_pos: f32,
    pub sweep_on: f32,
    pub hardness: f32,
}

/// Post-process uniforms (32 B).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct PostU {
    pub bloom: f32,
    pub exposure: f32,
    pub time: f32,
    pub scanlines: f32,
    pub vignette: f32,
    pub _pad0: f32,
    pub res: [f32; 2],
    pub edl_on: f32,
    pub edl_strength: f32,
    pub edl_radius: f32,
    pub bg_mode: f32,
}

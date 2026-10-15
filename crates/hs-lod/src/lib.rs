//! hs-lod — chunked density-mip LOD (plan §7).
//!
//! Model: the region is tiled into 256 m columns; each (column, layer) holds
//! nested density tiers (chunks). Chunk `t` of a level set `{i % 2^t == 0}` is
//! stored exclusively (`level t \ level t+1`), so rendering level L = drawing
//! chunks L..=TMAX and cross-fades never double density (plan §7.1).
//!
//! A `LodNode` = (column, layer, chunk) → AABB + slab range in one persistent
//! GPU instance pool (plan §4.3 "one big persistent pool with sub-allocated
//! slabs"). Pool layout is ordered coarse-chunk-first so the progressive
//! upload scheduler (≤48 MB/frame) reveals the region coarse→fine (§7.4) and
//! "node uploaded" is a contiguous byte check.

use glam::{Mat4, Vec3, Vec4};
use hs_forge::record::ImplVertex;
use hs_forge::{layer, Region};

pub const COL_SIZE: f32 = 256.0;
pub const TMAX: usize = 5; // chunks 0..=5 → coarsest = 32× base spacing

#[derive(Clone, Copy, Debug)]
pub struct LodNode {
    pub col: [i32; 2],
    pub layer: u8,
    pub chunk: u8,
    pub spacing: f32,
    pub origin: [f32; 4],
    pub quant_scale: f32,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub inst_start: u32,
    pub inst_count: u32,
    pub size_mul: f32,
    pub fade_near: f32,
    pub fade_far: f32,
}

pub struct LodRegion {
    pub nodes: Vec<LodNode>,
    pub instances: Vec<ImplVertex>,
    pub col_grid: [usize; 2],
    pub col_size: f32,
    pub region_min: [f32; 2],
    /// node index lookup: ((colx*gcy)+coly)*16*TMAX + layer*TMAX + chunk
    pub index: Vec<i32>,
    pub base_spacing: [f32; layer::COUNT],
    pub pool_bytes: u64,
}

impl LodRegion {
    pub fn node_at(&self, colx: i32, coly: i32, layer_id: u8, chunk: usize) -> Option<u32> {
        if colx < 0 || coly < 0 || chunk >= TMAX {
            return None;
        }
        let (gx, gy) = (colx as usize, coly as usize);
        if gx >= self.col_grid[0] || gy >= self.col_grid[1] {
            return None;
        }
        let i = ((gx * self.col_grid[1]) + gy) * layer::COUNT * TMAX + layer_id as usize * TMAX + chunk;
        match self.index.get(i).copied() {
            Some(v) if v >= 0 => Some(v as u32),
            _ => None,
        }
    }
}

struct Raw {
    p: [f32; 3],
    color: u8,
    flags: u8,
    intensity: u8,
}

/// Build columns + nested chunks + the GPU instance pool from a forged region.
pub fn build_lod(region: &Region, col_size: f32) -> LodRegion {
    let half = region.size_m * 0.5;
    let region_min = [-half, -half];
    let gcols = (region.size_m / col_size).ceil() as usize;
    let area = region.size_m * region.size_m;

    // per-layer finest spacing: prefer the generator's true spacing hint
    // (area/count is meaningless for linear features — BUG-M6.5-5)
    let mut base_spacing = [4.0f32; layer::COUNT];
    for node in &region.nodes {
        let existing = base_spacing[node.layer as usize];
        base_spacing[node.layer as usize] = existing.min(node.spacing_hint);
    }
    let _ = area;

    // gather world-space points per (column, layer)
    let mut buckets: std::collections::HashMap<(i32, i32, u8), Vec<Raw>> =
        std::collections::HashMap::new();
    for node in &region.nodes {
        let q = node.quant_scale;
        let (ox, oy, oz) = (node.origin[0], node.origin[1], node.origin[2]);
        for p in &node.points {
            let hs_forge::GpuPoint { pos, color, flags, intensity, .. } = *p;
            let x = pos[0] as f32 * q + ox;
            let y = pos[1] as f32 * q + oy;
            let z = pos[2] as f32 * q + oz;
            // furniture (walls/skirt) sits a few m outside the slab: clamp in
            let cx = (((x - region_min[0]) / col_size).floor() as i32).clamp(0, gcols as i32 - 1);
            let cy = (((y - region_min[1]) / col_size).floor() as i32).clamp(0, gcols as i32 - 1);
            buckets
                .entry((cx, cy, node.layer))
                .or_default()
                .push(Raw { p: [x, y, z], color, flags, intensity });
        }
    }

    // nodes sorted coarse-chunk-first (upload priority), then layer/col
    struct Pending {
        node: LodNode,
        verts: Vec<ImplVertex>,
    }
    let mut pend: Vec<Pending> = Vec::new();
    let mut index = vec![-1i32; (gcols * gcols) * layer::COUNT * TMAX];

    for chunk in (0..TMAX).rev() {
        let mut keys: Vec<(i32, i32, u8)> = buckets.keys().copied().collect();
        keys.sort();
        for (cx, cy, lay) in keys {
            let pts = &buckets[&(cx, cy, lay)];
            // exclusive chunk: i % 2^chunk == 0 && i % 2^(chunk+1) != 0
            let stride = 1usize << chunk;
            let sel: Vec<&Raw> = pts
                .iter()
                .enumerate()
                .filter(|(i, _)| i % stride == 0 && (chunk == TMAX - 1 || i % (stride * 2) != 0))
                .map(|(_, r)| r)
                .collect();
            if sel.is_empty() {
                continue;
            }
            let mut zmin = f32::MAX;
            let mut zmax = f32::MIN;
            for r in &sel {
                zmin = zmin.min(r.p[2]);
                zmax = zmax.max(r.p[2]);
            }
            let ox = region_min[0] + (cx as f32 + 0.5) * col_size;
            let oy = region_min[1] + (cy as f32 + 0.5) * col_size;
            let oz = (zmin + zmax) * 0.5;
            let extent = (col_size.max(zmax - zmin + 10.0)) * 1.1;
            let q = extent / 65534.0;
            let inv = 1.0 / q;
            let ni = pend.len() as u32;
            let verts: Vec<ImplVertex> = sel
                .iter()
                .map(|r| ImplVertex {
                    pos: [
                        ((r.p[0] - ox) * inv).round().clamp(-32768.0, 32767.0) as i16,
                        ((r.p[1] - oy) * inv).round().clamp(-32768.0, 32767.0) as i16,
                        ((r.p[2] - oz) * inv).round().clamp(-32768.0, 32767.0) as i16,
                        0,
                    ],
                    rgba: [r.color, lay, r.flags, r.intensity],
                    node_id: ni,
                })
                .collect();
            let size_mul = region
                .nodes
                .iter()
                .find(|n| n.layer == lay)
                .map(|n| n.size_mul)
                .unwrap_or(1.0);
            let node = LodNode {
                col: [cx, cy],
                layer: lay,
                chunk: chunk as u8,
                spacing: region
                    .nodes
                    .iter()
                    .find(|n| n.layer == lay)
                    .map(|n| n.spacing_hint)
                    .unwrap_or(base_spacing[lay as usize])
                    * (1u32 << chunk) as f32,
                origin: [ox, oy, oz, 0.0],
                quant_scale: q,
                aabb_min: [
                    region_min[0] + cx as f32 * col_size,
                    region_min[1] + cy as f32 * col_size,
                    zmin,
                ],
                aabb_max: [
                    region_min[0] + (cx + 1) as f32 * col_size,
                    region_min[1] + (cy + 1) as f32 * col_size,
                    zmax,
                ],
                inst_start: 0,
                inst_count: verts.len() as u32,
                size_mul,
                fade_near: 20_000.0,
                fade_far: 60_000.0,
            };
            pend.push(Pending { node, verts });
            let gi = ((cx as usize * gcols) + cy as usize) * layer::COUNT * TMAX
                + lay as usize * TMAX
                + chunk;
            if let Some(slot) = index.get_mut(gi) {
                *slot = ni as i32;
            }
        }
    }

    // lay out the pool in pend order (coarse first)
    let mut instances: Vec<ImplVertex> = Vec::new();
    let mut nodes: Vec<LodNode> = Vec::new();
    for p in &mut pend {
        p.node.inst_start = instances.len() as u32;
        instances.extend_from_slice(&p.verts);
        nodes.push(p.node);
    }
    // remap index (node ids unchanged; pend order == nodes order)
    let pool_bytes = (instances.len() * core::mem::size_of::<ImplVertex>()) as u64;
    LodRegion {
        nodes,
        instances,
        col_grid: [gcols, gcols],
        col_size,
        region_min,
        index,
        base_spacing,
        pool_bytes,
    }
}

/// Drop finest chunks globally until the pool fits `budget_bytes`
/// (coarse guarantee per column is preserved: chunk TMAX-1 always kept).
pub fn enforce_budget(lod: &mut LodRegion, budget_bytes: u64) -> usize {
    let mut dropped = 0;
    while lod.pool_bytes > budget_bytes {
        let finest = lod.nodes.iter().map(|n| n.chunk).min().unwrap_or(0);
        if finest >= (TMAX - 1) as u8 {
            break;
        }
        let keep: Vec<bool> = lod.nodes.iter().map(|n| n.chunk != finest).collect();
        rebuild_pool(lod, &keep);
        dropped += 1;
    }
    dropped
}

fn rebuild_pool(lod: &mut LodRegion, keep: &[bool]) {
    let mut instances: Vec<ImplVertex> = Vec::new();
    let mut nodes: Vec<LodNode> = Vec::new();
    let mut remap: Vec<i32> = vec![-1; lod.nodes.len()];
    for (i, k) in keep.iter().enumerate() {
        if !k {
            continue;
        }
        remap[i] = nodes.len() as i32;
        let mut n = lod.nodes[i];
        n.inst_start = instances.len() as u32;
        let start = lod.nodes[i].inst_start as usize;
        let end = start + lod.nodes[i].inst_count as usize;
        let mut verts = lod.instances[start..end].to_vec();
        for v in verts.iter_mut() {
            v.node_id = remap[i] as u32;
        }
        instances.extend_from_slice(&verts);
        nodes.push(n);
    }
    for slot in lod.index.iter_mut() {
        if *slot >= 0 {
            *slot = remap[*slot as usize];
        }
    }
    lod.nodes = nodes;
    lod.pool_bytes = (instances.len() * core::mem::size_of::<ImplVertex>()) as u64;
    lod.instances = instances;
}

// ------------------------------------------------------------------ traversal

#[derive(Clone, Copy, Debug)]
pub struct Sel {
    pub node: u32,
    pub fade: f32,
}

#[derive(Clone, Copy)]
struct ColState {
    level: u8,
    prev: u8,
    fade: f32,
}

pub struct Traversal {
    state: std::collections::HashMap<(i32, i32, u8), ColState>,
    pub last_ms: f32,
    cache_pos: Vec3,
    cache_yaw: f32,
    cache_pitch: f32,
    cache_focal: f32,
    cache_sels: Vec<Sel>,
    cache_valid: bool,
}

impl Default for Traversal {
    fn default() -> Self {
        Self::new()
    }
}

impl Traversal {
    pub fn new() -> Self {
        Self {
            state: Default::default(),
            last_ms: 0.0,
            cache_pos: Vec3::ZERO,
            cache_yaw: 0.0,
            cache_pitch: 0.0,
            cache_focal: 0.0,
            cache_sels: Vec::new(),
            cache_valid: false,
        }
    }

    /// Screen-space-error selection with hysteresis + 180 ms cross-fade
    /// (plan §7.2), frustum cull and draw-budget truncation (§7.3).
    pub fn select(
        &mut self,
        lod: &LodRegion,
        view_proj: &Mat4,
        cam: Vec3,
        yaw: f32,
        pitch: f32,
        focal_px: f32,
        dt: f32,
        pixel_thr: f32,
        point_budget: u32,
        uploaded: &dyn Fn(u32) -> bool,
    ) -> Vec<Sel> {
        let t0 = std::time::Instant::now();
        const ANG: f32 = 0.5f32.to_radians();
        let settled = self.state.values().all(|st| st.fade >= 1.0);
        if self.cache_valid
            && settled
            && (cam - self.cache_pos).length() < 1.0
            && (yaw - self.cache_yaw).abs() < ANG
            && (pitch - self.cache_pitch).abs() < ANG
            && (focal_px - self.cache_focal).abs() < 1.0
        {
            self.last_ms = t0.elapsed().as_secs_f32() * 1000.0;
            return self.cache_sels.clone();
        }
        let planes = frustum_planes(view_proj);
        let thr_up = pixel_thr;
        let thr_dn = pixel_thr * 0.57; // 0.8 px at holo's 1.4
        let mut sels: Vec<Sel> = Vec::new();
        let mut drawn: Vec<((i32, i32, u8), u8, f32)> = Vec::new(); // key, level, sse
        for cx in 0..lod.col_grid[0] as i32 {
            for cy in 0..lod.col_grid[1] as i32 {
                // frustum test on the column prism (any layer)
                let Some(probe) = lod.node_at(cx, cy, 0, TMAX - 1).or_else(|| {
                    (0..layer::COUNT as u8)
                        .find_map(|l| lod.node_at(cx, cy, l, TMAX - 1))
                }) else {
                    continue;
                };
                let n0 = &lod.nodes[probe as usize];
                let center = Vec3::new(
                    (n0.aabb_min[0] + n0.aabb_max[0]) * 0.5,
                    (n0.aabb_min[1] + n0.aabb_max[1]) * 0.5,
                    (n0.aabb_min[2] + n0.aabb_max[2]) * 0.5,
                );
                let radius = lod.col_size * 0.75 + (n0.aabb_max[2] - n0.aabb_min[2]) * 0.5;
                if !sphere_visible(&planes, center, radius) {
                    continue;
                }
                let dist = (center - cam).length().max(1.0);
                for lay in 0..layer::COUNT as u8 {
                    let Some(finest) = lod.node_at(cx, cy, lay, 0) else {
                        continue;
                    };
                    let base = lod.nodes[finest as usize].spacing;
                    let sse = |t: u8| base * (1u32 << t) as f32 * focal_px / dist;
                    let up = |t: u8| {
                        lod.node_at(cx, cy, lay, t as usize)
                            .map(|ni| uploaded(ni))
                            .unwrap_or(false)
                    };
                    let st = self
                        .state
                        .entry((cx, cy, lay))
                        .or_insert(ColState { level: TMAX as u8 - 1, prev: TMAX as u8 - 1, fade: 1.0 });
                    // target level with hysteresis around current
                    let mut target = st.level;
                    while target > 0 && sse(target - 1) >= thr_up && up(target - 1) {
                        target -= 1;
                    }
                    while (target as usize) < TMAX - 1 && sse(target) < thr_dn {
                        target += 1;
                    }
                    // clamp to uploaded chunks
                    while target < TMAX as u8 - 1 && !up(target) {
                        target += 1;
                    }
                    if target != st.level {
                        st.prev = st.level;
                        st.level = target;
                        st.fade = 0.0;
                    }
                    st.fade = (st.fade + dt / 0.18).min(1.0);
                    if st.fade >= 1.0 {
                        st.prev = st.level;
                    }
                    drawn.push(((cx, cy, lay), st.level, sse(st.level)));
                }
            }
        }
        // draw-budget truncation: coarsest SSE first stays; finest get coarsened
        drawn.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let mut total: u32 = 0;
        let mut levels: std::collections::HashMap<(i32, i32, u8), u8> = Default::default();
        for (key, level, sse) in &drawn {
            let mut lv = *level;
            let count = |lv: u8| -> u32 {
                (lv..TMAX as u8)
                    .filter_map(|t| lod.node_at(key.0, key.1, key.2, t as usize))
                    .map(|ni| lod.nodes[ni as usize].inst_count)
                    .sum()
            };
            let mut c = count(lv);
            while total + c > point_budget && (lv as usize) < TMAX - 1 {
                lv += 1;
                c = count(lv);
                let _ = sse;
            }
            total += c;
            levels.insert(*key, lv);
        }
        // emit sels with cross-fade on the transitioning chunk
        for (key, lv) in &levels {
            let st = self.state.get(key).copied().unwrap_or(ColState { level: *lv, prev: *lv, fade: 1.0 });
            let lo = (*lv).min(st.prev);
            let hi = (*lv).max(st.prev);
            for t in *lv..TMAX as u8 {
                let Some(ni) = lod.node_at(key.0, key.1, key.2, t as usize) else {
                    continue;
                };
                let fade = if st.fade < 1.0 && t == lo && st.prev > st.level {
                    // chunk being removed fades out
                    1.0 - st.fade
                } else if st.fade < 1.0 && t == hi && st.level < st.prev {
                    st.fade
                } else if t >= lo && t <= hi && st.fade < 1.0 && st.prev != st.level {
                    if t == st.level {
                        st.fade
                    } else if t == st.prev {
                        1.0 - st.fade
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };
                if fade > 0.01 {
                    sels.push(Sel { node: ni, fade });
                }
            }
            let _ = hi;
        }
        self.last_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.cache_pos = cam;
        self.cache_yaw = yaw;
        self.cache_pitch = pitch;
        self.cache_focal = focal_px;
        self.cache_sels = sels.clone();
        self.cache_valid = true;
        sels
    }

    /// Invalidate after region swap / upload progress changes.
    pub fn invalidate(&mut self) {
        self.cache_valid = false;
    }
}

fn frustum_planes(m: &Mat4) -> [Vec4; 6] {
    let c = m.to_cols_array_2d(); // glam: c[col][row]
    let row = |i: usize| Vec4::new(c[0][i], c[1][i], c[2][i], c[3][i]);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let mk = |a: Vec4| {
        let l = a.truncate().length().max(1e-9);
        a / l
    };
    [
        mk(r3 + r0), // left
        mk(r3 - r0), // right
        mk(r3 + r1), // bottom
        mk(r3 - r1), // top
        mk(r3 + r2), // near
        mk(r3 - r2), // far
    ]
}

fn sphere_visible(planes: &[Vec4; 6], c: Vec3, r: f32) -> bool {
    planes
        .iter()
        .all(|p| p.truncate().dot(c) + p.w >= -r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hs_forge::{forge_region, ForgeParams};
    use hs_geo::demo_heightfield;

    fn demo_lod() -> LodRegion {
        let hf = demo_heightfield(2000.0, 10.0);
        let r = forge_region(
            &hf,
            &ForgeParams {
                size_m: 2000.0,
                target_points: 200_000,
                ..Default::default()
            },
        );
        build_lod(&r, COL_SIZE)
    }

    #[test]
    fn chunks_are_nested_and_halving() {
        let lod = demo_lod();
        // for one column/layer, chunk counts should roughly halve per tier
        let counts: Vec<u32> = (0..TMAX)
            .filter_map(|t| lod.node_at(3, 3, layer::TERRAIN, t))
            .map(|ni| lod.nodes[ni as usize].inst_count)
            .collect();
        assert!(counts.len() >= 3);
        // exclusive chunks halve until the terminal chunk, which holds the
        // remainder of the nested set (== previous chunk size by construction)
        for (wi, w) in counts.windows(2).enumerate() {
            if wi + 2 < counts.len() {
                assert!(w[1] <= w[0] / 2 + 8, "chunk sizes not halving: {counts:?}");
            } else {
                assert!(w[1] <= w[0] + 8, "terminal chunk grew: {counts:?}");
            }
        }
    }

    #[test]
    fn sse_picks_finer_near_coarser_far() {
        let lod = demo_lod();
        let mut tr = Traversal::new();
        let proj = Mat4::perspective_rh(0.7, 1.0, 0.5, 100_000.0);
        let near = Vec3::new(0.0, 0.0, 60.0);
        let far = Vec3::new(0.0, 0.0, 9_000.0);
        let vp_near = proj * Mat4::look_at_rh(near, Vec3::new(30.0, 30.0, 0.0), Vec3::Z);
        let vp_far = proj * Mat4::look_at_rh(far, Vec3::new(0.0, 0.0, 0.0), Vec3::Z);
        let uploaded = |_n: u32| true;
        let s_near = tr.select(&lod, &vp_near, near, 0.8, -0.4, 1000.0, 0.016, 1.4, 5_000_000, &uploaded);
        let fine_near = s_near
            .iter()
            .map(|s| lod.nodes[s.node as usize].chunk)
            .min()
            .unwrap_or(9);
        let s_far = tr.select(&lod, &vp_far, far, 0.0, -0.4, 1000.0, 0.016, 1.4, 5_000_000, &uploaded);
        let fine_far = s_far
            .iter()
            .map(|s| lod.nodes[s.node as usize].chunk)
            .min()
            .unwrap_or(9);
        assert!(fine_near < fine_far, "near={fine_near} far={fine_far}");
    }

    #[test]
    fn budget_shrinks_pool() {
        let mut lod = demo_lod();
        let before = lod.pool_bytes;
        enforce_budget(&mut lod, before / 2);
        assert!(lod.pool_bytes <= before / 2 + 64 * 1024);
        // coarse guarantee: every column-layer still has its coarsest chunk
        assert!(lod.node_at(3, 3, layer::TERRAIN, TMAX - 1).is_some());
    }
}

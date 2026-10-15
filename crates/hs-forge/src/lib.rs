//! hs-forge — point-cloud synthesis (plan §6).
//!
//! M0 scope: the 12-byte `GpuPoint` record, the palette/LUT system, and the
//! procedural demo-region generators (terrain, contours, roads, water,
//! buildings, vegetation + holo furniture: skirt, cage, grid walls, base
//! outline, beacons). Real geodata forging arrives in M1/M2.

pub mod palette;
pub mod record;

use hs_geo::{fbm, HeightField};

pub use record::{GpuPoint, ImplVertex, LineVert, NodeU, QuadVert, FLAG_EMISSIVE};

/// Layer ids — Appendix B convention (subset used in M0).
pub mod layer {
    pub const TERRAIN: u8 = 0;
    pub const CONTOUR: u8 = 1;
    pub const ROAD: u8 = 2;
    pub const RAIL: u8 = 7;
    pub const WATER: u8 = 8;
    pub const BUILDING: u8 = 9;
    pub const VEG: u8 = 11;
    pub const FURNITURE: u8 = 14;
    pub const BEACON: u8 = 15;
    pub const IMPORTED: u8 = 16;
    pub const COUNT: usize = 17;
}

/// Flat palette-index slots for non-elevation-colored layers.
pub mod slot {
    pub const CONTOUR: u8 = 70;
    pub const ROAD: u8 = 71;
    pub const RAIL: u8 = 72;
    pub const WATER: u8 = 73;
    pub const BUILDING: u8 = 74;
    pub const VEG: u8 = 75;
    pub const FURNITURE: u8 = 76;
    pub const FURNITURE_DIM: u8 = 77;
    pub const OUTLINE: u8 = 78;
    pub const BEACON: u8 = 79;
}

/// A render node (plan §7 `LodNode` seed): one point batch with its own
/// origin + quantization scale. M0 builds one node per layer.
#[derive(Clone)]
pub struct Node {
    pub name: String,
    pub layer: u8,
    pub points: Vec<GpuPoint>,
    pub origin: [f32; 4],
    pub quant_scale: f32,
    pub size_mul: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    /// true point spacing in metres (LOD SSE needs this; area/count guesses
    /// are meaningless for linear features like roads — BUG-M6.5-5)
    pub spacing_hint: f32,
}

impl Node {
    fn new(name: &str, layer: u8, size_mul: f32) -> Self {
        Self {
            name: name.into(),
            layer,
            points: Vec::new(),
            origin: [0.0; 4],
            quant_scale: 1.0,
            size_mul,
            fade_near: 20_000.0,
            fade_far: 60_000.0,
            spacing_hint: 4.0,
        }
    }
}

/// Builder helper: accumulate world positions + per-point color/flags.
struct Accum {
    world: Vec<[f32; 3]>,
    color: Vec<u8>,
    flags: Vec<u8>,
    intensity: Vec<u8>,
}

impl Accum {
    fn new() -> Self {
        Self {
            world: Vec::new(),
            color: Vec::new(),
            flags: Vec::new(),
            intensity: Vec::new(),
        }
    }
    #[inline]
    fn push(&mut self, p: [f32; 3], color: u8, flags: u8, intensity: u8) {
        self.world.push(p);
        self.color.push(color);
        self.flags.push(flags);
        self.intensity.push(intensity);
    }
    fn len(&self) -> usize {
        self.world.len()
    }
    fn into_node(self, name: &str, layer: u8, size_mul: f32, extent: f32) -> Node {
        self.into_node_spaced(name, layer, size_mul, extent, 4.0)
    }
    fn into_node_spaced(
        self,
        name: &str,
        layer: u8,
        size_mul: f32,
        extent: f32,
        spacing: f32,
    ) -> Node {
        let mut node = Node::new(name, layer, size_mul);
        node.spacing_hint = spacing;
        node.quant_scale = extent / 32767.0;
        let inv = 1.0 / node.quant_scale;
        node.points.reserve(self.world.len());
        for (i, w) in self.world.iter().enumerate() {
            node.points.push(GpuPoint {
                pos: [
                    (w[0] * inv).round().clamp(-32768.0, 32767.0) as i16,
                    (w[1] * inv).round().clamp(-32768.0, 32767.0) as i16,
                    (w[2] * inv).round().clamp(-32768.0, 32767.0) as i16,
                ],
                color: self.color[i],
                layer,
                flags: self.flags[i],
                intensity: self.intensity[i],
            });
        }
        node
    }
}

/// Build a node from world-space points with per-point color/flags/intensity
/// (used by the LiDAR import path, plan FR-12).
pub fn node_from_world(
    name: &str,
    layer: u8,
    extent: f32,
    pts: Vec<([f32; 3], u8, u8, u8)>,
) -> Node {
    let mut acc = Accum::new();
    for (p, c, f, i) in pts {
        acc.push(p, c, f, i);
    }
    acc.into_node(name, layer, 1.0, extent)
}

/// Everything the renderer needs for one region.
pub struct Region {
    pub nodes: Vec<Node>,
    pub lines: Vec<LineVert>,
    pub quads: Vec<QuadVert>,
    /// continuous ribbon cores under road/water points (palette-aware)
    pub road_quads: Vec<LineVert>,
    pub size_m: f32,
    pub max_h: f32,
    pub skirt_depth: f32,
    pub cage_top: f32,
    pub point_count: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Beacon {
    pub x: f32,
    pub y: f32,
}

/// Neutral vector input (converted from OSM by hs-app; plan §6.2).
/// All coordinates are region-local ENU meters.
#[derive(Clone, Debug, Default)]
pub struct VectorData {
    /// (class rank 0=motorway..4=path, centerline)
    pub roads: Vec<(u8, Vec<[f32; 2]>)>,
    pub rail: Vec<Vec<[f32; 2]>>,
    pub water_polys: Vec<Vec<[f32; 2]>>,
    /// (width m, centerline)
    pub water_lines: Vec<(f32, Vec<[f32; 2]>)>,
    /// (height m, footprint ring, closed)
    pub buildings: Vec<(f32, Vec<[f32; 2]>)>,
    pub veg_polys: Vec<Vec<[f32; 2]>>,
}

pub struct ForgeParams {
    pub size_m: f32,
    pub target_points: u32,
    pub seed: u32,
    pub beacons: Vec<Beacon>,
    /// synthetic street grid / river / blocks (demo region only)
    pub vectors: bool,
    /// real OSM vectors (M2); when present, replaces synthetics
    pub osm: Option<VectorData>,
    /// vector density preset multiplier (user: low .5 / med 1 / high 2 / ultra 4)
    pub vector_density: f32,
}

impl Default for ForgeParams {
    fn default() -> Self {
        Self {
            size_m: 4000.0,
            target_points: 1_500_000,
            seed: 20261012,
            vectors: true,
            osm: None,
            vector_density: 1.0,
            beacons: vec![
                Beacon { x: -350.0, y: -950.0 },
                Beacon { x: 750.0, y: -550.0 },
                Beacon { x: 1350.0, y: 250.0 },
            ],
        }
    }
}

#[inline]
fn jitter(i: u32, salt: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9e3779b1) ^ salt.wrapping_mul(0x85ebca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    (h as f32 / u32::MAX as f32) - 0.5
}

#[inline]
fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Valley mask: 1 on the flat city side, 0 in the mountains
/// (matches hs-geo::demo_heightfield composition).
fn valley_mask(x: f32, y: f32, size: f32) -> f32 {
    let u = x / size;
    let v = y / size;
    1.0 - smooth01((v + 0.10) * 2.4 - u * 0.5)
}

/// Forge the full demo region.
pub fn forge_region(hf: &HeightField, params: &ForgeParams) -> Region {
    let size = params.size_m;
    let half = size * 0.5;
    let skirt_depth = 90.0f32;
    let max_h = hf.zs.iter().cloned().fold(f32::MIN, f32::max).max(1.0);
    let cage_top = max_h + size * 0.16;

    let n_target = params.target_points as f32;
    let vd = params.vector_density.clamp(0.25, 8.0);
    let n_terrain = (n_target * 0.52) as usize;
    let n_veg = (n_target * 0.10 * vd) as usize;
    let n_build = (n_target * 0.16 * vd) as usize;
    let n_road = (n_target * 0.10 * vd) as usize;
    let n_water = (n_target * 0.04 * vd) as usize;
    let n_contour = (n_target * 0.05) as usize;
    let n_skirt = (n_target * 0.03) as usize;

    // ---------------- terrain ----------------
    let mut acc = Accum::new();
    let spacing = (size * size / n_terrain as f32).sqrt();
    let cols = (size / spacing) as u32;
    for i in 0..(cols * cols) {
        if acc.len() >= n_terrain {
            break;
        }
        let gx = i % cols;
        let gy = i / cols;
        let x = -half + gx as f32 * spacing + jitter(i, 1) * spacing * 0.7;
        let y = -half + gy as f32 * spacing + jitter(i, 2) * spacing * 0.7;
        if x < -half || x > half || y < -half || y > half {
            continue;
        }
        let h = hf.sample(x, y);
        let t = (h / max_h).clamp(0.0, 1.0);
        let color = (t * 63.0) as u8; // elevation ramp slots 0..63
        let inten = (90.0 + jitter(i, 3) * 60.0).clamp(35.0, 170.0) as u8;
        acc.push([x, y, h], color, 0, inten);
    }
    let terrain = acc.into_node_spaced("terrain", layer::TERRAIN, 1.0, size, spacing);

    // ---------------- contours (marching squares, 25 m interval) ----------------
    let mut acc = Accum::new();
    let interval = 25.0f32;
    let step = 7.0f32; // along-line spacing
    let mut level = interval;
    while level < max_h && acc.len() < n_contour {
        for iy in 0..hf.ny - 1 {
            for ix in 0..hf.nx - 1 {
                if acc.len() >= n_contour {
                    break;
                }
                let a = hf.get(ix, iy);
                let b = hf.get(ix + 1, iy);
                let c = hf.get(ix + 1, iy + 1);
                let d = hf.get(ix, iy + 1);
                let mut code = 0u8;
                if a > level {
                    code |= 1;
                }
                if b > level {
                    code |= 2;
                }
                if c > level {
                    code |= 4;
                }
                if d > level {
                    code |= 8;
                }
                if code == 0 || code == 15 {
                    continue;
                }
                let x0 = hf.min_x + ix as f32 * hf.spacing;
                let y0 = hf.min_y + iy as f32 * hf.spacing;
                let lerp = |p: f32, q: f32| ((level - p) / (q - p)).clamp(0.0, 1.0);
                let e = [
                    [x0 + hf.spacing * lerp(a, b), y0],
                    [x0 + hf.spacing, y0 + hf.spacing * lerp(b, c)],
                    [x0 + hf.spacing * (1.0 - lerp(d, c)), y0 + hf.spacing],
                    [x0, y0 + hf.spacing * lerp(a, d)],
                ];
                let segs: [[i8; 2]; 2] = match code {
                    1 | 14 => [[0, 3], [-1, -1]],
                    2 | 13 => [[0, 1], [-1, -1]],
                    3 | 12 => [[1, 3], [-1, -1]],
                    4 | 11 => [[1, 2], [-1, -1]],
                    6 | 9 => [[2, 3], [-1, -1]],
                    7 | 8 => [[0, 2], [-1, -1]],
                    5 => [[0, 3], [1, 2]],
                    10 => [[0, 1], [2, 3]],
                    _ => [[-1, -1], [-1, -1]],
                };
                for s in segs.iter() {
                    if s[0] < 0 || s[1] < 0 {
                        continue;
                    }
                    let p = e[s[0] as usize];
                    let q = e[s[1] as usize];
                    let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
                    let n = (len / step).ceil() as usize;
                    for k in 0..=n {
                        let f = if n == 0 { 0.0 } else { k as f32 / n as f32 };
                        let x = p[0] + (q[0] - p[0]) * f;
                        let y = p[1] + (q[1] - p[1]) * f;
                        acc.push([x, y, level + 0.4], slot::CONTOUR, 0, 80);
                    }
                }
            }
        }
        level += interval;
    }
    let contours = acc.into_node_spaced("contours", layer::CONTOUR, 0.8, size, step);

    // ---------------- roads: synthetic street grid in the valley ----------------
    let mut road_node: Option<Node> = None;
    let mut water_node: Option<Node> = None;
    let mut build_node: Option<Node> = None;
    let mut syn_lines: Vec<(u8, Vec<[f32; 2]>)> = Vec::new();
    if params.vectors && params.osm.is_none() {
        let grid = [250.0f32, 500.0, 1000.0];
        for (gi, g) in grid.iter().enumerate() {
            let rank = [2u8, 2, 0][gi];
            let mut y = -half + g * 0.5;
            while y < half {
                let mut run: Vec<[f32; 2]> = Vec::new();
                let mut x = -half;
                while x < half {
                    if valley_mask(x, y, size) > 0.35 {
                        run.push([x, y]);
                    } else if run.len() > 2 {
                        syn_lines.push((rank, std::mem::take(&mut run)));
                    } else {
                        run.clear();
                    }
                    x += 8.0;
                }
                if run.len() > 2 {
                    syn_lines.push((rank, run));
                }
                y += g;
            }
            let mut x = -half + g * 0.5;
            while x < half {
                let mut run: Vec<[f32; 2]> = Vec::new();
                let mut y = -half;
                while y < half {
                    if valley_mask(x, y, size) > 0.35 {
                        run.push([x, y]);
                    } else if run.len() > 2 {
                        syn_lines.push((rank, std::mem::take(&mut run)));
                    } else {
                        run.clear();
                    }
                    y += 8.0;
                }
                if run.len() > 2 {
                    syn_lines.push((rank, run));
                }
                x += g;
            }
        }
        for d in 0..2 {
            let (s0, e0) = if d == 0 {
                ([-half, -half * 0.9], [half * 0.7, half])
            } else {
                ([-half * 0.6, -half], [half, half * 0.2])
            };
            let mut run = Vec::new();
            let len = ((e0[0] - s0[0]).powi(2) + (e0[1] - s0[1]).powi(2)).sqrt();
            let n = (len / 8.0) as usize;
            for k in 0..=n {
                let f = k as f32 / n as f32;
                let x = s0[0] + (e0[0] - s0[0]) * f;
                let y = s0[1] + (e0[1] - s0[1]) * f;
                if valley_mask(x, y, size) > 0.2 {
                    run.push([x, y]);
                }
            }
            if run.len() > 2 {
                syn_lines.push((1, run));
            }
        }
    }
    let mut acc = Accum::new();
    let mut road_quads_local: Vec<LineVert> = Vec::new();
    if params.vectors && params.osm.is_none() {
        emit_roads(hf, &syn_lines, n_road, vd, &mut acc, &mut road_quads_local);
    }

    // ---------------- water: meandering river through the valley ----------------
    let mut acc = Accum::new();
    if params.vectors && params.osm.is_none() {
    let nst = ((size / 4.0) as u32).min((n_water / 6) as u32); // ~4 m stations
    let mut i = 0u32;
    while i < nst {
        let t = i as f32 / nst as f32;
        let x = -half + t * size;
        let yc = -half * 0.72 + (t * 6.0).sin() * size * 0.055 + fbm(t * 4.0, 0.5, 3, 4242) * size * 0.05;
        let w = 26.0 + fbm(t * 9.0, 2.5, 2, 88) * 22.0;
        for l in 0..6 {
            let f = (l as f32 + jitter(i, l + 5)) / 6.0;
            let y = yc + (f - 0.5) * 2.0 * w;
            let h = hf.sample(x, y) - 0.6;
            acc.push([x + jitter(i, 9) * 3.0, y, h], slot::WATER, 0, 100);
        }
        i += 1;
    }
    water_node = Some(acc.into_node_spaced("water", layer::WATER, 1.0, size, 3.0));
    }

    // ---------------- buildings: blocks inside the street grid ----------------
    let mut acc = Accum::new();
    if params.vectors && params.osm.is_none() {
    let mut bi = 0u32;
    let cell = 250.0f32;
    let mut cy = -half + cell * 0.5;
    'outer: while cy < half {
        let mut cx = -half + cell * 0.5;
        while cx < half {
            if valley_mask(cx, cy, size) > 0.4 {
                for b in 0..6 {
                    let r = |s: u32| jitter(bi * 31 + b * 7 + s, params.seed);
                    let bx = cx + (r(1) - 0.5) * cell * 0.72;
                    let by = cy + (r(2) - 0.5) * cell * 0.72;
                    let wd = 10.0 + r(3).abs() * 16.0;
                    let dp = 10.0 + r(4).abs() * 22.0;
                    let hh = 6.0 + r(5).abs() * 16.0 + if r(6) > 0.42 { 14.0 } else { 0.0 };
                    let base = hf.sample(bx, by);
                    let shell = 1.0f32;
                    let per = 2.0 * (wd + dp);
                    let n = (per / shell) as usize;
                    for k in 0..n {
                        let d = k as f32 * shell;
                        let (px, py) = rect_perimeter(bx, by, wd, dp, d);
                        let floors = (hh / 3.0) as usize;
                        for fl in 0..floors {
                            let z = base + fl as f32 * 3.0 + 1.2;
                            let win = jitter(k as u32 * 91 + fl as u32 * 17 + b, 777);
                            let inten = if win > 0.78 { 150 } else { 70 };
                            acc.push([px, py, z], slot::BUILDING, 0, inten);
                        }
                    }
                    let nroof = (per / 2.2) as usize;
                    for k in 0..nroof {
                        let (px, py) = rect_perimeter(bx, by, wd, dp, k as f32 * 2.2);
                        acc.push([px, py, base + hh], slot::BUILDING, 0, 110);
                    }
                    bi += 1;
                    if acc.len() >= n_build {
                        break 'outer;
                    }
                }
            }
            cx += cell;
        }
        cy += cell;
    }
    build_node = Some(acc.into_node_spaced("buildings", layer::BUILDING, 1.0, size, 1.5));
    }

    // ---------------- vegetation ----------------
    let mut acc = Accum::new();
    if let Some(osm) = &params.osm {
        gen_veg(hf, osm, n_veg, vd, &mut acc);
    }
    let mut i = 0u32;
    while params.osm.is_none() && acc.len() < n_veg {
        let x = -half + (jitter(i, 21) + 0.5) * size;
        let y = -half + (jitter(i, 22) + 0.5) * size;
        let h = hf.sample(x, y);
        let m = smooth01((y / size + 0.08) * 2.2 - (x / size) * 0.55);
        if m > 0.25 && h > 25.0 {
            let dome = (jitter(i, 23) + 0.5).sqrt() * 9.0 + 1.5;
            acc.push([x, y, h + dome], slot::VEG, 0, 80);
        }
        i += 1;
    }
    let veg = acc.into_node_spaced("vegetation", layer::VEG, 0.9, size, 4.0);

    // ---------------- skirt: boundary ring falling to the base plane ----------------
    let mut acc = Accum::new();
    let ring_step = 16.0f32;
    let cols_ring = (4.0 * size / ring_step) as u32;
    let depth_steps = (skirt_depth / 4.2) as u32;
    'skirt: for di in 0..depth_steps {
        for c in 0..cols_ring {
            if acc.len() >= n_skirt {
                break 'skirt;
            }
            let i = di * cols_ring + c;
            let d = c as f32 * ring_step;
            let (x, y) = if d < size {
                (-half + d, -half)
            } else if d < 2.0 * size {
                (half, -half + (d - size))
            } else if d < 3.0 * size {
                (half - (d - 2.0 * size), half)
            } else {
                (-half, half - (d - 3.0 * size))
            };
            let h0 = hf.sample(x, y);
            let depth = di as f32 * 4.2 + jitter(i, 31).abs() * 4.2;
            let z = h0 - depth;
            let fade = 1.0 - depth / skirt_depth;
            // no skirt on mountain edges (reference shows skirt only on low sides)
            let edge = 1.0 - smooth01((h0 / max_h - 0.22) / 0.30);
            if edge < 0.05 {
                continue;
            }
            acc.push(
                [x + jitter(i, 32) * 2.0, y + jitter(i, 33) * 2.0, z],
                slot::FURNITURE,
                0,
                ((30.0 + 70.0 * fade) * edge) as u8,
            );
        }
    }
    let skirt = acc.into_node_spaced("skirt", layer::FURNITURE, 1.0, size * 1.2, 9.0);

    // ---------------- grid walls (point lattices on two orthogonal planes) ----
    let mut acc = Accum::new();
    let wall_step = size / 26.0;
    let mut y = -half;
    while y <= half {
        let mut z = -skirt_depth * 0.85;
        while z <= cage_top {
            acc.push([-half - 4.0, y, z], slot::FURNITURE_DIM, 0, 90);
            z += wall_step;
        }
        y += wall_step;
    }
    let walls = acc.into_node_spaced("grid-walls", layer::FURNITURE, 0.9, size * 1.4, wall_step);

    // ---------------- lines: cage + base outline ----------------
    let mut lines: Vec<LineVert> = Vec::new();
    let mut seg = |p: [f32; 3], q: [f32; 3], col: [f32; 4]| {
        lines.push(LineVert { pos: p, col, slot: 80 });
        lines.push(LineVert { pos: q, col, slot: 80 });
    };
    let cyan = [0.10, 0.85, 0.95, 0.55];
    let cyan_dim = [0.10, 0.85, 0.95, 0.30];
    let lime = [0.62, 1.00, 0.10, 0.95];
    let z0 = -skirt_depth;
    let z1 = cage_top;
    let m = size * 0.02;
    let (x0, x1, y0, y1) = (-half - m, half + m, -half - m, half + m);
    seg([x0, y0, z0], [x1, y0, z0], cyan);
    seg([x0, y1, z0], [x1, y1, z0], cyan);
    seg([x0, y0, z1], [x1, y0, z1], cyan);
    seg([x0, y1, z1], [x1, y1, z1], cyan);
    seg([x0, y0, z0], [x0, y0, z1], cyan);
    seg([x1, y0, z0], [x1, y0, z1], cyan);
    seg([x0, y1, z0], [x0, y1, z1], cyan);
    seg([x1, y1, z0], [x1, y1, z1], cyan);
    for (px, py) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        let t = size * 0.03;
        let zm = z0 + (z1 - z0) * 0.5;
        seg([px - t, py, zm], [px + t, py, zm], cyan_dim);
    }
    let rect = |inset: f32, col: [f32; 4], seg: &mut dyn FnMut([f32; 3], [f32; 3], [f32; 4])| {
        seg([x0 + inset, y0 + inset, z0], [x1 - inset, y0 + inset, z0], col);
        seg([x0 + inset, y1 - inset, z0], [x1 - inset, y1 - inset, z0], col);
        seg([x0 + inset, y0 + inset, z0], [x0 + inset, y1 - inset, z0], col);
        seg([x1 - inset, y0 + inset, z0], [x1 - inset, y1 - inset, z0], col);
    };
    rect(0.0, lime, &mut seg);
    rect(size * 0.035, [lime[0] * 0.6, lime[1] * 0.6, lime[2] * 0.6, 0.5], &mut seg);

    // ---------------- quads: beacon beams + diamonds ----------------
    let quads = beacon_quads(&params.beacons, &|x, y| hf.sample(x, y), cage_top, size);
    {
        let _ = &params.beacons;
    }

    // ---------------- real OSM vectors (M2) ----------------
    if let Some(osm) = &params.osm {
        let mut ra = Accum::new();
        emit_roads(hf, &osm.roads, n_road, vd, &mut ra, &mut road_quads_local);
        gen_rail(hf, osm, n_road / 4, &mut ra);
        road_node = Some(ra.into_node_spaced("osm-roads", layer::ROAD, 1.15, size, 3.0 / vd));
        let mut wa = Accum::new();
        gen_water(hf, osm, n_water, vd, &mut wa, &mut road_quads_local);
        water_node = Some(wa.into_node_spaced("osm-water", layer::WATER, 1.0, size, 3.0 / vd));
        let mut ba = Accum::new();
        gen_buildings(hf, osm, n_build, vd, &mut ba);
        build_node = Some(ba.into_node_spaced("osm-buildings", layer::BUILDING, 1.0, size, 1.0 / vd));
    }

    let mut nodes = vec![terrain, contours];
    if let Some(n) = road_node {
        nodes.push(n);
    }
    if let Some(n) = water_node {
        nodes.push(n);
    }
    if let Some(n) = build_node {
        nodes.push(n);
    }
    nodes.extend([veg, skirt, walls]);
    let point_count = nodes.iter().map(|n| n.points.len() as u64).sum();
    Region {
        nodes,
        lines,
        quads,
        road_quads: road_quads_local,
        size_m: size,
        max_h,
        skirt_depth,
        cage_top,
        point_count,
    }
}

/// Sutherland–Hodgman clip of a ring against the axis-aligned extent square
/// [-half, half]² (closes BUG-M2-2 for boundary-straddling polygons).
pub fn clip_poly_to_extent(poly: &[[f32; 2]], half: f32) -> Vec<[f32; 2]> {
    let mut out = poly.to_vec();
    for edge in 0..4 {
        if out.is_empty() {
            break;
        }
        let inside = |p: [f32; 2]| match edge {
            0 => p[0] >= -half,
            1 => p[0] <= half,
            2 => p[1] >= -half,
            _ => p[1] <= half,
        };
        let intersect = |a: [f32; 2], b: [f32; 2]| -> [f32; 2] {
            let t = match edge {
                0 | 1 => {
                    let x = if edge == 0 { -half } else { half };
                    if (b[0] - a[0]).abs() < 1e-9 {
                        0.0
                    } else {
                        ((x - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0)
                    }
                }
                _ => {
                    let y = if edge == 2 { -half } else { half };
                    if (b[1] - a[1]).abs() < 1e-9 {
                        0.0
                    } else {
                        ((y - a[1]) / (b[1] - a[1])).clamp(0.0, 1.0)
                    }
                }
            };
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let mut nxt = Vec::with_capacity(out.len());
        for w in out.windows(2) {
            let (a, b) = (w[0], w[1]);
            if inside(a) {
                nxt.push(a);
                if !inside(b) {
                    nxt.push(intersect(a, b));
                }
            } else if inside(b) {
                nxt.push(intersect(a, b));
            }
        }
        // close the loop pair
        let (a, b) = (out[out.len() - 1], out[0]);
        if inside(a) {
            if !inside(b) {
                nxt.push(intersect(a, b));
            }
        } else if inside(b) {
            nxt.push(intersect(a, b));
        }
        out = nxt;
    }
    if out.len() >= 3 {
        out.push(out[0]);
        out
    } else {
        Vec::new()
    }
}

/// Beam + diamond quads for a beacon list (used dynamically by M4 ray-pick).
pub fn beacon_quads(
    beacons: &[Beacon],
    ground: &dyn Fn(f32, f32) -> f32,
    top: f32,
    size: f32,
) -> Vec<QuadVert> {
    let mut quads: Vec<QuadVert> = Vec::new();
    for b in beacons {
        let h = ground(b.x, b.y);
        let top = top * 0.82;
        let bw = size * 0.0016;
        let beam_col = [0.72, 1.0, 0.12, 0.38];
        let beam_top = [0.72, 1.0, 0.12, 0.05];
        for rot in [0.0f32, std::f32::consts::FRAC_PI_2] {
            let (dx, dy) = (rot.cos() * bw, rot.sin() * bw);
            let v = [
                QuadVert { pos: [b.x - dx, b.y - dy, h], col: beam_col, slot: 80 },
                QuadVert { pos: [b.x + dx, b.y + dy, h], col: beam_col, slot: 80 },
                QuadVert { pos: [b.x + dx, b.y + dy, top], col: beam_top, slot: 80 },
                QuadVert { pos: [b.x - dx, b.y - dy, top], col: beam_top, slot: 80 },
            ];
            quads.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
        }
        let ds = size * 0.038;
        let dc = [0.80, 1.0, 0.10, 0.60];
        let de = [0.55, 0.85, 0.05, 0.20];
        let c = QuadVert { pos: [b.x, b.y, top], col: dc, slot: 80 };
        let p0 = QuadVert { pos: [b.x - ds, b.y, top], col: de, slot: 80 };
        let p1 = QuadVert { pos: [b.x, b.y + ds, top], col: de, slot: 80 };
        let p2 = QuadVert { pos: [b.x + ds, b.y, top], col: de, slot: 80 };
        let p3 = QuadVert { pos: [b.x, b.y - ds, top], col: de, slot: 80 };
        quads.extend_from_slice(&[c, p0, p1, c, p1, p2, c, p2, p3, c, p3, p0]);
    }
    quads
}

#[inline]
fn rect_perimeter(bx: f32, by: f32, wd: f32, dp: f32, d: f32) -> (f32, f32) {
    if d < wd {
        (bx - wd * 0.5 + d, by - dp * 0.5)
    } else if d < wd + dp {
        (bx + wd * 0.5, by - dp * 0.5 + (d - wd))
    } else if d < 2.0 * wd + dp {
        (bx + wd * 0.5 - (d - wd - dp), by + dp * 0.5)
    } else {
        (bx - wd * 0.5, by + dp * 0.5 - (d - 2.0 * wd - dp))
    }
}

// ---------------------------------------------------------------- M2 generators

/// Ray-cast point-in-polygon (ring in ENU meters).
pub fn point_in_poly(x: f32, y: f32, poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi + 1e-12) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn walk(line: &[[f32; 2]], spacing: f32, mut f: impl FnMut([f32; 2], [f32; 2])) {
    // f(point, unit-direction)
    for w in line.windows(2) {
        let dx = w[1][0] - w[0][0];
        let dy = w[1][1] - w[0][1];
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let dir = [dx / len, dy / len];
        let steps = (len / spacing).max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            f([w[0][0] + dx * t, w[0][1] + dy * t], dir);
        }
    }
}

const ROAD_WIDTH: [f32; 5] = [12.0, 10.0, 8.0, 6.0, 1.6];
const ROAD_LANES: [usize; 5] = [4, 3, 2, 2, 1];
const ROAD_INTEN: [u8; 5] = [185, 170, 150, 140, 120];

fn emit_roads(
    hf: &hs_geo::HeightField,
    roads: &[(u8, Vec<[f32; 2]>)],
    budget: usize,
    vd: f32,
    acc: &mut Accum,
    ribbons: &mut Vec<LineVert>,
) {
    let poly_len = |line: &[[f32; 2]]| -> f32 {
        line.windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum()
    };
    let demand: f32 = roads
        .iter()
        .map(|(rank, line)| {
            let r = (*rank as usize).min(4);
            (poly_len(line) / 3.0).ceil() * ROAD_LANES[r] as f32
        })
        .sum();
    let spacing = (3.0 / vd) * (demand / budget.max(1) as f32).max(1.0);
    for (rank, line) in roads {
        if acc.len() >= budget {
            break;
        }
        let r = (*rank as usize).min(4);
        let lanes = ROAD_LANES[r];
        walk(line, spacing, |p, dir| {
            let perp = [-dir[1], dir[0]];
            for l in 0..lanes {
                let off = (l as f32 - (lanes - 1) as f32 * 0.5) * (ROAD_WIDTH[r] / lanes as f32);
                let x = p[0] + perp[0] * off;
                let y = p[1] + perp[1] * off;
                let h = hf.sample(x, y) + 0.25;
                acc.push([x, y, h], slot::ROAD, FLAG_EMISSIVE, ROAD_INTEN[r]);
            }
        });
        // continuous ribbon core (solid glow under the point texture)
        let hw = ROAD_WIDTH[r] * 0.35;
        let mut stations: Vec<([f32; 3], [f32; 2])> = Vec::new();
        walk(line, spacing.max(4.0), |p, dir| {
            let h = hf.sample(p[0], p[1]) + 0.18;
            stations.push(([p[0], p[1], h], [-dir[1], dir[0]]));
        });
        for w in stations.windows(2) {
            let (p0, n0) = w[0];
            let (p1, n1) = w[1];
            let a = [p0[0] - n0[0] * hw, p0[1] - n0[1] * hw, p0[2]];
            let b = [p0[0] + n0[0] * hw, p0[1] + n0[1] * hw, p0[2]];
            let c = [p1[0] + n1[0] * hw, p1[1] + n1[1] * hw, p1[2]];
            let d = [p1[0] - n1[0] * hw, p1[1] - n1[1] * hw, p1[2]];
            let col = [1.0, 1.0, 1.0, 0.34];
            ribbons.push(LineVert { pos: a, col, slot: 71 });
            ribbons.push(LineVert { pos: b, col, slot: 71 });
            ribbons.push(LineVert { pos: c, col, slot: 71 });
            ribbons.push(LineVert { pos: a, col, slot: 71 });
            ribbons.push(LineVert { pos: c, col, slot: 71 });
            ribbons.push(LineVert { pos: d, col, slot: 71 });
        }
    }
}

fn gen_rail(hf: &hs_geo::HeightField, osm: &VectorData, budget: usize, acc: &mut Accum) {
    let demand: f32 = osm
        .rail
        .iter()
        .map(|l| {
            l.windows(2)
                .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
                .sum::<f32>()
                / 2.0
        })
        .sum::<f32>()
        * 2.0;
    let spacing = 2.0f32 * (demand / budget.max(1) as f32).max(1.0);
    for line in &osm.rail {
        if acc.len() >= budget {
            break;
        }
        walk(line, spacing, |p, dir| {
            let perp = [-dir[1], dir[0]];
            for s in [-0.72f32, 0.72] {
                let x = p[0] + perp[0] * s;
                let y = p[1] + perp[1] * s;
                let h = hf.sample(x, y) + 0.2;
                acc.push([x, y, h], slot::RAIL, FLAG_EMISSIVE, 150);
            }
        });
    }
}

fn gen_water(
    hf: &hs_geo::HeightField,
    osm: &VectorData,
    budget: usize,
    vd: f32,
    acc: &mut Accum,
    ribbons: &mut Vec<LineVert>,
) {
    // rough interior-area demand at 5 m fill
    let area: f32 = osm
        .water_polys
        .iter()
        .map(|p| {
            let mut a = 0.0f32;
            for w in p.windows(2) {
                a += w[0][0] * w[1][1] - w[1][0] * w[0][1];
            }
            (a * 0.5).abs()
        })
        .sum();
    let fill_factor = ((area / 25.0) / budget.max(1) as f32).sqrt().max(1.0) / vd.sqrt();
    let mut seed = 0u32;
    for poly in &osm.water_polys {
        if acc.len() >= budget {
            break;
        }
        // shoreline ring
        walk(poly, 1.5, |p, _| {
            let h = hf.sample(p[0], p[1]) - 0.4;
            acc.push([p[0], p[1], h], slot::WATER, FLAG_EMISSIVE, 160);
        });
        // interior fill on a jittered grid
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in poly {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let step = 5.0f32 * fill_factor;
        let mut gy = 0;
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            let mut gx = 0;
            while x <= x1 {
                let jx = x + jitter(seed + gx, 41) * step;
                let jy = y + jitter(seed + gy, 42) * step;
                if point_in_poly(jx, jy, poly) {
                    let h = hf.sample(jx, jy) - 0.6;
                    acc.push([jx, jy, h], slot::WATER, 0, 100);
                }
                x += step;
                gx += 1;
            }
            y += step;
            gy += 1;
        }
        seed += 7919;
    }
    for (width, line) in &osm.water_lines {
        if acc.len() >= budget {
            break;
        }
        walk(line, 3.0 / vd, |p, dir| {
            let perp = [-dir[1], dir[0]];
            for l in 0..3 {
                let off = (l as f32 - 1.0) * (width / 3.0);
                let x = p[0] + perp[0] * off;
                let y = p[1] + perp[1] * off;
                let h = hf.sample(x, y) - 0.6;
                acc.push([x, y, h], slot::WATER, FLAG_EMISSIVE, 140);
            }
        });
        // continuous water ribbon
        let hw = width * 0.5;
        let mut stations: Vec<([f32; 3], [f32; 2])> = Vec::new();
        walk(line, 6.0, |p, dir| {
            let h = hf.sample(p[0], p[1]) - 0.5;
            stations.push(([p[0], p[1], h], [-dir[1], dir[0]]));
        });
        for w in stations.windows(2) {
            let (p0, n0) = w[0];
            let (p1, n1) = w[1];
            let col = [1.0, 1.0, 1.0, 0.30];
            let a = [p0[0] - n0[0] * hw, p0[1] - n0[1] * hw, p0[2]];
            let b = [p0[0] + n0[0] * hw, p0[1] + n0[1] * hw, p0[2]];
            let c = [p1[0] + n1[0] * hw, p1[1] + n1[1] * hw, p1[2]];
            let d = [p1[0] - n1[0] * hw, p1[1] - n1[1] * hw, p1[2]];
            ribbons.push(LineVert { pos: a, col, slot: 73 });
            ribbons.push(LineVert { pos: b, col, slot: 73 });
            ribbons.push(LineVert { pos: c, col, slot: 73 });
            ribbons.push(LineVert { pos: a, col, slot: 73 });
            ribbons.push(LineVert { pos: c, col, slot: 73 });
            ribbons.push(LineVert { pos: d, col, slot: 73 });
        }
    }
}

fn gen_buildings(hf: &hs_geo::HeightField, osm: &VectorData, budget: usize, vd: f32, acc: &mut Accum) {
    // estimate demand at 1 m shell / 3 m floors, then coarsen uniformly so the
    // whole city appears (sparse-everywhere beats dense-only-some, BUG-M6-2)
    let perim = |ring: &[[f32; 2]]| -> f32 {
        ring.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum()
    };
    let shell_base = 1.0 / vd;
    let demand: f32 = osm
        .buildings
        .iter()
        .map(|(h, ring)| perim(ring) / shell_base * (h / 3.0).max(1.0) + perim(ring) / 2.2)
        .sum();
    let factor = (demand / budget.max(1) as f32).max(1.0);
    let shell = shell_base * factor.clamp(1.0, 5.0);
    let floor_step = 3.0f32 * (factor / 5.0).clamp(1.0, 3.0); // coarsen floors only after shells
    let stride = (factor / 15.0).ceil().max(1.0) as usize; // last resort: skip every nth
    for (bi, (height, ring)) in osm.buildings.iter().enumerate() {
        if acc.len() >= budget {
            break;
        }
        if bi % stride != 0 {
            continue;
        }
        let mut cx = 0.0;
        let mut cy = 0.0;
        for p in ring {
            cx += p[0];
            cy += p[1];
        }
        let npt = ring.len().max(1) as f32;
        let base = hf.sample(cx / npt, cy / npt);
        let floors = (*height / floor_step).max(1.0) as usize;
        let mut k = 0u32;
        walk(ring, shell, |p, _| {
            for fl in 0..floors {
                let z = base + fl as f32 * floor_step + 1.2;
                let win = jitter(bi as u32 * 91 + k * 17 + fl as u32, 777);
                let inten = if win > 0.78 { 150 } else { 70 };
                acc.push([p[0], p[1], z], slot::BUILDING, 0, inten);
            }
            k += 1;
        });
        walk(ring, 2.2 * shell, |p, _| {
            acc.push([p[0], p[1], base + height], slot::BUILDING, 0, 110);
        });
    }
}

fn gen_veg(hf: &hs_geo::HeightField, osm: &VectorData, budget: usize, vd: f32, acc: &mut Accum) {
    let area: f32 = osm
        .veg_polys
        .iter()
        .map(|p| {
            let mut a = 0.0f32;
            for w in p.windows(2) {
                a += w[0][0] * w[1][1] - w[1][0] * w[0][1];
            }
            (a * 0.5).abs()
        })
        .sum();
    let veg_factor = (((area / 16.0) / budget.max(1) as f32).sqrt() / vd.sqrt()).max(1.0);
    let mut seed = 1u32;
    for poly in &osm.veg_polys {
        if acc.len() >= budget {
            break;
        }
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in poly {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let step = 4.0f32 * veg_factor;
        let mut gy = 0;
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            let mut gx = 0;
            while x <= x1 {
                let jx = x + jitter(seed + gx, 51) * step;
                let jy = y + jitter(seed + gy, 52) * step;
                if point_in_poly(jx, jy, poly) {
                    let h = hf.sample(jx, jy);
                    let dome = (jitter(seed + gx * 31 + gy, 53) + 0.5).sqrt() * 8.0 + 4.0;
                    acc.push([jx, jy, h + dome], slot::VEG, 0, 80);
                }
                x += step;
                gx += 1;
            }
            y += step;
            gy += 1;
        }
        seed += 104729;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pip_square() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]];
        assert!(point_in_poly(5.0, 5.0, &sq));
        assert!(!point_in_poly(15.0, 5.0, &sq));
        assert!(!point_in_poly(-1.0, -1.0, &sq));
    }

    #[test]
    fn osm_vectors_forge() {
        let hf = hs_geo::demo_heightfield(2000.0, 10.0);
        let mut vd = VectorData::default();
        vd.roads.push((3, vec![[-800.0, -800.0], [800.0, 800.0]]));
        vd.buildings.push((9.0, vec![[-100.0, -100.0], [100.0, -100.0], [100.0, 100.0], [-100.0, 100.0], [-100.0, -100.0]]));
        vd.water_polys.push(vec![[300.0, 300.0], [500.0, 300.0], [500.0, 500.0], [300.0, 500.0], [300.0, 300.0]]);
        vd.veg_polys.push(vec![[-500.0, 300.0], [-300.0, 300.0], [-300.0, 500.0], [-500.0, 500.0], [-500.0, 300.0]]);
        let r = forge_region(&hf, &ForgeParams { size_m: 2000.0, target_points: 60_000, vectors: false, osm: Some(vd), ..Default::default() });
        let names: Vec<&str> = r.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"osm-roads"));
        assert!(names.contains(&"osm-buildings"));
        let roads = r.nodes.iter().find(|n| n.name == "osm-roads").unwrap();
        assert!(roads.points.len() > 100);
        let b = r.nodes.iter().find(|n| n.name == "osm-buildings").unwrap();
        assert!(b.points.len() > 100);
    }
}

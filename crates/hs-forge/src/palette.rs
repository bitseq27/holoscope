//! Palette → 256-entry RGB LUT (plan §6.2 / Appendix B).
//!
//! Slots 0..=63  : terrain elevation ramp (low → high)
//! Slots 70..=79 : flat per-layer colors (contour, road, rail, water,
//!                 building, veg, furniture, furniture-dim, outline, beacon)

pub const PALETTES: &[&str] = &["hologreen", "cyanice", "magentascan", "amberops", "monowhite"];

pub fn palette_index(name: &str) -> usize {
    PALETTES.iter().position(|p| *p == name).unwrap_or(0)
}

type Rgb = [f32; 3];

#[inline]
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn ramp(stops: &[(f32, Rgb)], t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let f = ((t - w[0].0) / (w[1].0 - w[0].0)).clamp(0.0, 1.0);
            return mix(w[0].1, w[1].1, f);
        }
    }
    stops.last().unwrap().1
}

/// Default display colour for a layer under a palette (for UI pickers).
pub fn layer_default_color(name: &str, layer: u8) -> [f32; 3] {
    let lut = build_lut(name);
    let slot: usize = match layer {
        crate::layer::TERRAIN => 24, // mid ramp
        crate::layer::CONTOUR => 70,
        crate::layer::ROAD => 71,
        crate::layer::RAIL => 72,
        crate::layer::WATER => 73,
        crate::layer::BUILDING => 74,
        crate::layer::VEG => 75,
        crate::layer::FURNITURE => 76,
        crate::layer::BEACON => 79,
        crate::layer::IMPORTED => 81,
        _ => 76,
    };
    let c = lut[slot];
    [c[0], c[1], c[2]]
}

/// Apply per-layer tints on top of a palette LUT (M6 user request):
/// terrain tint multiplies the elevation ramp; flat layers are replaced.
pub fn apply_tints(
    lut: &mut [[f32; 4]],
    tints: &[[f32; 3]; crate::layer::COUNT],
    on: &[bool; crate::layer::COUNT],
) {
    use crate::layer::*;
    if on[TERRAIN as usize] {
        let t = tints[TERRAIN as usize];
        for i in 0..64 {
            lut[i][0] *= t[0] * 2.0;
            lut[i][1] *= t[1] * 2.0;
            lut[i][2] *= t[2] * 2.0;
        }
    }
    let flat = [
        (CONTOUR, 70), (ROAD, 71), (RAIL, 72), (WATER, 73),
        (BUILDING, 74), (VEG, 75), (FURNITURE, 76), (BEACON, 79),
        (IMPORTED, 81),
    ];
    for (layer, slot) in flat {
        let l = layer as usize;
        if on[l] {
            let t = tints[l];
            lut[slot] = [t[0], t[1], t[2], 1.0];
            if layer == FURNITURE {
                lut[77] = [t[0] * 0.4, t[1] * 0.4, t[2] * 0.4, 1.0];
            }
        }
    }
}

/// Build the 256×4-float LUT (alpha = 1; brightness carried by intensity byte).
pub fn build_lut(name: &str) -> Vec<[f32; 4]> {
    //                    low          mid-low        mid           high         peak
    let (terrain, contour, road, water, building, veg, furn, furn_dim, outline, beacon) = match name {
        "cyanice" => (
            vec![
                (0.00, [0.02, 0.20, 0.35]),
                (0.35, [0.05, 0.55, 0.75]),
                (0.70, [0.35, 0.85, 1.00]),
                (1.00, [0.90, 0.98, 1.00]),
            ],
            [0.10, 0.45, 0.60],
            [0.45, 0.95, 1.00],
            [0.10, 0.40, 0.90],
            [0.55, 0.80, 0.95],
            [0.20, 0.50, 0.60],
            [0.20, 0.70, 0.90],
            [0.08, 0.30, 0.42],
            [0.80, 0.95, 1.00],
            [1.00, 0.95, 0.40],
        ),
        "magentascan" => (
            vec![
                (0.00, [0.10, 0.55, 0.65]),
                (0.40, [0.20, 0.75, 0.80]),
                (0.75, [0.55, 0.30, 0.80]),
                (1.00, [1.00, 0.30, 0.80]),
            ],
            [0.30, 0.55, 0.60],
            [0.30, 0.90, 0.95],
            [0.30, 0.30, 0.85],
            [1.00, 0.20, 0.75],
            [0.45, 0.25, 0.65],
            [0.85, 0.25, 0.75],
            [0.35, 0.10, 0.35],
            [1.00, 0.75, 0.95],
            [1.00, 0.85, 0.20],
        ),
        "amberops" => (
            vec![
                (0.00, [0.35, 0.12, 0.02]),
                (0.40, [0.80, 0.35, 0.05]),
                (0.75, [1.00, 0.70, 0.15]),
                (1.00, [1.00, 0.95, 0.70]),
            ],
            [0.55, 0.30, 0.08],
            [1.00, 0.75, 0.20],
            [0.80, 0.30, 0.10],
            [0.95, 0.60, 0.25],
            [0.50, 0.30, 0.10],
            [1.00, 0.60, 0.15],
            [0.40, 0.22, 0.06],
            [1.00, 0.85, 0.30],
            [1.00, 0.40, 0.10],
        ),
        "monowhite" => (
            vec![
                (0.00, [0.15, 0.15, 0.16]),
                (0.50, [0.55, 0.55, 0.58]),
                (1.00, [1.00, 1.00, 1.00]),
            ],
            [0.35, 0.35, 0.36],
            [0.90, 0.90, 0.92],
            [0.45, 0.45, 0.50],
            [0.75, 0.75, 0.78],
            [0.35, 0.35, 0.36],
            [0.60, 0.60, 0.62],
            [0.25, 0.25, 0.26],
            [1.00, 1.00, 1.00],
            [1.00, 1.00, 1.00],
        ),
        // hologreen — the reference look (map1 3d.jpg)
        _ => (
            vec![
                (0.00, [0.005, 0.18, 0.09]),
                (0.30, [0.01, 0.30, 0.19]),
                (0.55, [0.02, 0.26, 0.34]),
                (0.80, [0.02, 0.14, 0.24]),
                (1.00, [0.10, 0.26, 0.36]),
            ],
            [0.04, 0.32, 0.26],
            [0.10, 0.78, 0.80],
            [0.03, 0.22, 0.55],
            [0.08, 0.55, 0.45],
            [0.08, 0.38, 0.22],
            [0.06, 0.55, 0.60],
            [0.03, 0.22, 0.26],
            [0.62, 1.00, 0.10],
            [0.78, 1.00, 0.10],
        ),
    };

    let mut lut = vec![[0.0f32; 4]; 256];
    for i in 0..64 {
        let c = ramp(&terrain, i as f32 / 63.0);
        lut[i] = [c[0], c[1], c[2], 1.0];
    }
    let mut set = |slot: usize, c: Rgb| lut[slot] = [c[0], c[1], c[2], 1.0];
    set(70, contour);
    set(71, road);
    set(72, [0.90, 0.60, 0.10]); // rail
    set(73, water);
    set(74, building);
    set(75, veg);
    set(76, furn);
    set(77, furn_dim);
    set(78, outline);
    set(79, beacon);
    set(80, [1.0, 1.0, 1.0]);
    // imported LiDAR default: warm scan amber
    set(81, [1.0, 0.72, 0.25]);
    lut
}

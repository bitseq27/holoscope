//! hs-geo — coordinate math, noise, heightfields (plan §4.2 / Appendix A).
//!
//! M0 scope: WGS84 geodetic ⇄ ECEF ⇄ ENU (f64), deterministic value-noise fBm,
//! and the procedural demo heightfield used until real DEM ingest lands (M1).

pub use glam;
use glam::DVec3;

// ---------------------------------------------------------------- geodesy
pub const WGS84_A: f64 = 6378137.0;
pub const WGS84_F: f64 = 1.0 / 298.257223563;

/// Geodetic (lat°, lon°, ellipsoidal height m) → ECEF (m).
pub fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, h: f64) -> DVec3 {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let e2 = WGS84_F * (2.0 - WGS84_F);
    let sin_lat = lat.sin();
    let n = WGS84_A / (1.0 - e2 * sin_lat * sin_lat).sqrt();
    DVec3::new(
        (n + h) * lat.cos() * lon.cos(),
        (n + h) * lat.cos() * lon.sin(),
        (n * (1.0 - e2) + h) * sin_lat,
    )
}

/// East-North-Up frame anchored at a geodetic origin (Appendix A convention).
#[derive(Clone, Copy, Debug)]
pub struct Enu {
    pub origin: DVec3, // ECEF of origin
    sin_lat: f64,
    cos_lat: f64,
    sin_lon: f64,
    cos_lon: f64,
}

impl Enu {
    pub fn new(lat_deg: f64, lon_deg: f64, h: f64) -> Self {
        let lat = lat_deg.to_radians();
        let lon = lon_deg.to_radians();
        Self {
            origin: geodetic_to_ecef(lat_deg, lon_deg, h),
            sin_lat: lat.sin(),
            cos_lat: lat.cos(),
            sin_lon: lon.sin(),
            cos_lon: lon.cos(),
        }
    }

    /// ECEF → ENU (meters), f64 throughout.
    pub fn ecef_to_enu(&self, p: DVec3) -> DVec3 {
        let d = p - self.origin;
        DVec3::new(
            -self.sin_lon * d.x + self.cos_lon * d.y,
            -self.sin_lat * self.cos_lon * d.x - self.sin_lat * self.sin_lon * d.y + self.cos_lat * d.z,
            self.cos_lat * self.cos_lon * d.x + self.cos_lat * self.sin_lon * d.y + self.sin_lat * d.z,
        )
    }

    /// ENU → ECEF (meters).
    pub fn enu_to_ecef(&self, e: DVec3) -> DVec3 {
        DVec3::new(
            -self.sin_lon * e.x - self.sin_lat * self.cos_lon * e.y + self.cos_lat * self.cos_lon * e.z,
            self.cos_lon * e.x - self.sin_lat * self.sin_lon * e.y + self.cos_lat * self.sin_lon * e.z,
            self.cos_lat * e.y + self.sin_lat * e.z,
        )
        + self.origin
    }
}

/// ECEF → geodetic (lat°, lon°, ellipsoidal h), iterative (≤ 1 µm).
pub fn ecef_to_geodetic(p: DVec3) -> (f64, f64, f64) {
    let e2 = WGS84_F * (2.0 - WGS84_F);
    let lon = p.y.atan2(p.x);
    let r = (p.x * p.x + p.y * p.y).sqrt();
    let mut lat = p.z.atan2(r * (1.0 - e2));
    let mut h = 0.0;
    for _ in 0..10 {
        let sin_lat = lat.sin();
        let n = WGS84_A / (1.0 - e2 * sin_lat * sin_lat).sqrt();
        h = r / lat.cos().max(1e-12) - n;
        lat = p.z.atan2(r * (1.0 - e2 * n / (n + h)));
    }
    (lat.to_degrees(), lon.to_degrees(), h)
}

// ---------------------------------------------------------------- noise
#[inline]
fn hash2(ix: i32, iy: i32, seed: u32) -> f32 {
    let mut h = (ix as u32)
        .wrapping_mul(0x27d4eb2d)
        ^ (iy as u32).wrapping_mul(0x165667b1)
        ^ seed.wrapping_mul(0x9e3779b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// 2D value noise in [0, 1].
pub fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = smooth(x - ix as f32);
    let fy = smooth(y - iy as f32);
    let a = hash2(ix, iy, seed);
    let b = hash2(ix + 1, iy, seed);
    let c = hash2(ix, iy + 1, seed);
    let d = hash2(ix + 1, iy + 1, seed);
    a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
}

/// Fractal Brownian motion, `oct` octaves, result in ~[0, 1].
pub fn fbm(x: f32, y: f32, oct: u32, seed: u32) -> f32 {
    let mut amp = 0.5;
    let mut f = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..oct {
        sum += amp * vnoise(x * f, y * f, seed.wrapping_add(o * 77));
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}

/// Ridged multifractal in ~[0, 1] (sharp crests → mountain look).
pub fn ridged(x: f32, y: f32, oct: u32, seed: u32) -> f32 {
    let mut amp = 0.5;
    let mut f = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..oct {
        let n = 1.0 - (vnoise(x * f, y * f, seed.wrapping_add(o * 131)) * 2.0 - 1.0).abs();
        let n = n * n;
        sum += amp * n;
        norm += amp;
        amp *= 0.5;
        f *= 2.11;
    }
    sum / norm
}

// ---------------------------------------------------------------- heightfield
/// Regular square grid of elevations (m), axis-aligned in region-local ENU.
#[derive(Clone)]
pub struct HeightField {
    pub nx: usize,
    pub ny: usize,
    pub spacing: f32,
    pub min_x: f32,
    pub min_y: f32,
    pub zs: Vec<f32>,
}

impl HeightField {
    #[inline]
    pub fn get(&self, ix: usize, iy: usize) -> f32 {
        self.zs[iy * self.nx + ix]
    }

    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let gx = ((x - self.min_x) / self.spacing).clamp(0.0, (self.nx - 1) as f32);
        let gy = ((y - self.min_y) / self.spacing).clamp(0.0, (self.ny - 1) as f32);
        let ix = gx as usize;
        let iy = gy as usize;
        let fx = gx - ix as f32;
        let fy = gy - iy as f32;
        let ix1 = (ix + 1).min(self.nx - 1);
        let iy1 = (iy + 1).min(self.ny - 1);
        let a = self.get(ix, iy);
        let b = self.get(ix1, iy);
        let c = self.get(ix, iy1);
        let d = self.get(ix1, iy1);
        a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
    }
}

/// Procedural demo region (M0 empty-state, plan §11.3): mountains on the
/// +Y/-X side, flat valley floor with a synthetic street grid on -Y/+X,
/// echoing the composition of reference `map1 3d.jpg`.
pub fn demo_heightfield(size_m: f32, spacing: f32) -> HeightField {
    let n = (size_m / spacing) as usize + 1;
    let half = size_m * 0.5;
    let mut zs = vec![0.0f32; n * n];
    for iy in 0..n {
        let y = -half + iy as f32 * spacing;
        for ix in 0..n {
            let x = -half + ix as f32 * spacing;
            let u = x / size_m;
            let v = y / size_m;
            // mountain mask: strong on +Y, fading toward the valley
            let m = smooth((((v + 0.08) * 2.2) - u * 0.55).clamp(0.0, 1.0));
            let ridge = ridged(u * 7.0 + 3.1, v * 7.0 - 1.7, 5, 1201);
            let base = fbm(u * 3.0, v * 3.0, 4, 77);
            let micro = fbm(u * 40.0, v * 40.0, 3, 991) - 0.5;
            let h = m * (ridge * 430.0 + base * 120.0) + (1.0 - m) * base * 14.0 + micro * 1.6;
            zs[iy * n + ix] = h;
        }
    }
    HeightField {
        nx: n,
        ny: n,
        spacing,
        min_x: -half,
        min_y: -half,
        zs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enu_roundtrip() {
        let enu = Enu::new(35.0198, 135.7690, 40.0);
        let ecef = geodetic_to_ecef(35.0205, 135.7701, 60.0);
        let e = enu.ecef_to_enu(ecef);
        let back = enu.enu_to_ecef(e);
        assert!((back - ecef).length() < 1e-6);
    }

    #[test]
    fn polar_enu_roundtrip() {
        let enu = Enu::new(78.2, 15.6, 0.0); // Svalbard
        let ecef = geodetic_to_ecef(78.201, 15.602, 50.0);
        let e = enu.ecef_to_enu(ecef);
        let back = enu.enu_to_ecef(e);
        assert!((back - ecef).length() < 1e-6);
        let enu_pole = Enu::new(89.9, 0.0, 0.0);
        let ecef_p = geodetic_to_ecef(89.9005, 10.0, 0.0);
        let ep = enu_pole.ecef_to_enu(ecef_p);
        let back_p = enu_pole.enu_to_ecef(ep);
        assert!((back_p - ecef_p).length() < 1e-6);
    }

    #[test]
    fn noise_bounds() {
        for i in 0..200 {
            let x = i as f32 * 0.37;
            let v = vnoise(x, x * 0.5, 5);
            assert!((0.0..=1.0).contains(&v));
        }
        let hf = demo_heightfield(100.0, 5.0);
        assert!(hf.zs.iter().all(|z| z.is_finite()));
    }
}

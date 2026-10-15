//! hs-ingest — providers + region pipeline (plan §5, M1 slice).
//!
//! M1: Copernicus DEM GLO-30 (COG, ranged GETs) + Photon/Nominatim geocoding,
//! all cache-first through hs-cache. Vectors (OSM/Overture) arrive in M2.

pub mod cog;
pub mod geocode;
pub mod http;
pub mod lidar;
pub mod osm;
pub mod ot;

use anyhow::{anyhow, Result};
use hs_cache::Cache;
use hs_geo::{Enu, HeightField};

/// Regular geodetic elevation grid (row 0 = north).
#[derive(Clone, Debug)]
pub struct GeoGrid {
    pub nx: usize,
    pub ny: usize,
    pub lon0: f64,
    pub lat0: f64,
    pub dlon: f64,
    pub dlat: f64, // negative (southward)
    pub zs: Vec<f32>,
}

impl GeoGrid {
    pub fn sample(&self, lon: f64, lat: f64) -> f32 {
        let gx = ((lon - self.lon0) / self.dlon).clamp(0.0, self.nx as f64 - 1.0);
        let gy = ((lat - self.lat0) / self.dlat).clamp(0.0, self.ny as f64 - 1.0);
        let ix = gx as usize;
        let iy = gy as usize;
        let fx = (gx - ix as f64) as f32;
        let fy = (gy - iy as f64) as f32;
        let ix1 = (ix + 1).min(self.nx - 1);
        let iy1 = (iy + 1).min(self.ny - 1);
        let a = self.zs[iy * self.nx + ix];
        let b = self.zs[iy * self.nx + ix1];
        let c = self.zs[iy1 * self.nx + ix];
        let d = self.zs[iy1 * self.nx + ix1];
        a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
    }
}

pub struct RegionPlan {
    pub lat0: f64,
    pub lat1: f64,
    pub lon0: f64,
    pub lon1: f64,
    pub tiles: Vec<(i64, i64)>, // (lat_floor, lon_floor) of each 1° COG
    pub est_download_mb: f32,
}

/// Plan the DEM fetch for a bbox + extent (plan §5.2 step 2-3).
pub fn plan_region(lat0: f64, lat1: f64, lon0: f64, lon1: f64) -> RegionPlan {
    // antimeridian-safe: iterate by span, wrapping tile longitudes to [-180,180)
    let lon0 = lon0;
    let mut lon1 = lon1;
    if lon1 < lon0 {
        lon1 += 360.0;
    }
    let mut tiles = Vec::new();
    let mut la = lat0.floor() as i64;
    while la <= lat1.floor() as i64 {
        let mut lo = lon0.floor() as i64;
        while lo <= lon1.floor() as i64 {
            let wrapped = ((lo + 180).rem_euclid(360)) - 180;
            tiles.push((la, wrapped));
            lo += 1;
        }
        la += 1;
    }
    // ~25 MB compressed per full 1° tile; scale by the bbox area fraction
    let frac = ((lat1 - lat0).min(1.0) * (lon1 - lon0).min(1.0)) as f32;
    let nt = tiles.len() as f32;
    RegionPlan {
        lat0,
        lat1,
        lon0,
        lon1,
        tiles,
        est_download_mb: 25.0 * frac * nt,
    }
}

/// Fetch one COG tile payload through the cache (keyed by url#index).
pub fn fetch_tile_cached(cache: &Cache, client: &http::Client, meta: &cog::CogMeta, idx: usize) -> Result<Vec<u8>> {
    let key = format!("{}#{}", meta.url, idx);
    let off = *meta
        .tile_offsets
        .get(idx)
        .ok_or_else(|| anyhow!("tile {idx} missing"))?;
    let cnt = *meta
        .tile_counts
        .get(idx)
        .ok_or_else(|| anyhow!("tile {idx} missing"))? as u64;
    cache.blob_fetch("cog-tile", &key, || client.get_range(&meta.url, Some((off, cnt))))
}

/// Open a COG (header parse, cached) for the tile containing (lat, lon).
pub fn open_cog(cache: &Cache, client: &http::Client, lat: f64, lon: f64) -> Result<cog::CogMeta> {
    let url = cog::copernicus_url(lat.floor() as i64, lon.floor() as i64);
    let head_key = format!("{}#head", url);
    let head = cache.blob_fetch("cog-head", &head_key, || client.get_range(&url, Some((0, 262_144))))?;
    cog::parse_header(&head, &url)
}

/// Resample a geodetic DEM window into a region-local ENU heightfield
/// (plan App. A: f64 geodetic → ENU; grid spacing chosen from extent).
pub fn dem_to_heightfield(grid: &GridSet, enu: &Enu, extent_m: f64, target_cells: usize) -> HeightField {
    let spacing = (extent_m / target_cells as f64).max(8.0) as f32;
    let n = target_cells + 1;
    let half = extent_m * 0.5;
    let mut zs = vec![0.0f32; n * n];
    for iy in 0..n {
        let y = -half + iy as f64 * spacing as f64;
        for ix in 0..n {
            let x = -half + ix as f64 * spacing as f64;
            let ecef = enu.enu_to_ecef(glam_dvec3(x, y, 0.0));
            let (lat, lon, _h) = hs_geo::ecef_to_geodetic(ecef);
            let mut h = grid.sample(lon, lat);
            if h < -1000.0 {
                h = 0.0;
            }
            zs[iy * n + ix] = h;
        }
    }
    let _ = enu;
    HeightField {
        nx: n,
        ny: n,
        spacing,
        min_x: -half as f32,
        min_y: -half as f32,
        zs,
    }
}

fn glam_dvec3(x: f64, y: f64, z: f64) -> hs_geo::glam::DVec3 {
    hs_geo::glam::DVec3::new(x, y, z)
}

/// Full M1 load pipeline: plan → open COGs → read windows → ENU heightfield.
/// `progress(idx, total)` is called per 1° tile (for the staged progress bar).
pub fn load_dem(
    cache: &Cache,
    client: &http::Client,
    plan: &RegionPlan,
    enu: &Enu,
    extent_m: f64,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(usize, usize),
) -> Result<HeightField> {
    // Merge all 1° windows into one GeoGrid by reading each and stitching via
    // a common geodetic raster at DEM resolution would be ideal; M1 takes the
    // simpler path: read each tile window separately and build a merger grid.
    let mut grids: Vec<GeoGrid> = Vec::new();
    for (i, (la, lo)) in plan.tiles.iter().enumerate() {
        if cancel() {
            return Err(anyhow!("cancelled"));
        }
        progress(i, plan.tiles.len());
        let meta = open_cog(cache, client, *la as f64 + 0.5, *lo as f64 + 0.5)?;
        // clamp the window to this tile's 1° span (matters at the antimeridian)
        let tlo0 = *lo as f64;
        let tlo1 = tlo0 + 1.0;
        let mut wlo0 = plan.lon0.max(tlo0);
        let mut wlo1 = plan.lon1.max(tlo0);
        if plan.lon1 < plan.lon0 {
            // wrapped region: this tile sees the shifted window
            wlo0 = plan.lon0.max(tlo0);
            wlo1 = (plan.lon1 + 360.0).min(tlo1).max(tlo0);
            if wlo1 <= wlo0 {
                wlo0 = plan.lon0.min(tlo1);
                wlo1 = plan.lon1.max(tlo0);
            }
        }
        wlo0 = wlo0.clamp(tlo0, tlo1);
        wlo1 = wlo1.clamp(tlo0, tlo1);
        if wlo1 <= wlo0 {
            continue;
        }
        let g = cog::read_window(&meta, wlo0, plan.lat0, wlo1, plan.lat1, |m, idx| {
            fetch_tile_cached(cache, client, m, idx)
        })?;
        grids.push(g);
    }
    progress(plan.tiles.len(), plan.tiles.len());
    let set = GridSet(grids);
    Ok(dem_to_heightfield(&set, enu, extent_m, 480))
}

/// A set of disjoint geodetic tile grids with contained-tile sampling.
/// Handles antimeridian-straddling regions naturally: each ENU cell asks for
/// its own (lat, lon) and exactly one tile answers (plan §13 edge cases).
pub struct GridSet(pub Vec<GeoGrid>);

impl GridSet {
    pub fn sample(&self, lon: f64, lat: f64) -> f32 {
        let eps = 1e-9;
        for g in &self.0 {
            let lon_end = g.lon0 + g.dlon * (g.nx as f64 - 1.0);
            let lat_end = g.lat0 + g.dlat * (g.ny as f64 - 1.0);
            let (lo0, lo1) = if g.dlon >= 0.0 {
                (g.lon0, lon_end)
            } else {
                (lon_end, g.lon0)
            };
            let (la0, la1) = if g.dlat >= 0.0 {
                (g.lat0, lat_end)
            } else {
                (lat_end, g.lat0)
            };
            if lon >= lo0 - eps && lon <= lo1 + eps && lat >= la0 - eps && lat <= la1 + eps {
                return g.sample(lon, lat);
            }
        }
        self.0.first().map(|g| g.sample(lon, lat)).unwrap_or(0.0)
    }
}

/// Attribution string for the sources actually used (FR-15 / NFR-10).
pub fn attribution(with_osm: bool) -> String {
    let mut a = "elevation: Copernicus DEM GLO-30 (© ESA/Copernicus) · geocoding: Photon + Nominatim (© OpenStreetMap contributors)".to_string();
    if with_osm {
        a.push_str(" · map data: © OpenStreetMap contributors (ODbL)");
    }
    a
}

pub fn attribution_dem() -> &'static str {
    "elevation: Copernicus DEM GLO-30 (© ESA/Copernicus) · geocoding: Photon + Nominatim (© OpenStreetMap contributors)"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attribution_snapshot() {
        assert!(attribution(false).contains("Copernicus"));
        assert!(!attribution(false).contains("ODbL)"));
        let with = attribution(true);
        assert!(with.contains("OpenStreetMap contributors (ODbL)"));
    }

    #[test]
    fn antimeridian_plan_wraps_tiles() {
        let p = plan_region(-5.0, -4.0, 179.5, -179.5); // crosses 180°
        let lons: Vec<i64> = p.tiles.iter().map(|t| t.1).collect();
        assert!(lons.contains(&179), "west tile: {lons:?}");
        assert!(lons.contains(&-180), "east tile: {lons:?}");
    }

    #[test]
    fn gridset_samples_across_seam() {
        let mk = |lon0: f64| GeoGrid {
            nx: 3,
            ny: 1,
            lon0,
            lat0: 0.5,
            dlon: 0.5,
            dlat: -0.5,
            zs: vec![10.0, 20.0, 30.0],
        };
        let set = GridSet(vec![mk(179.0), mk(-180.0 as f64 + 360.0 - 360.0)]);
        assert_eq!(set.sample(179.5, 0.5), 20.0);
        // wrapped tile stored at -180: sample just west of the seam
        assert_eq!(set.sample(-179.5, 0.5), 20.0);
    }
}

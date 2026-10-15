//! hs-app — HoloScope main binary (plan §4.2; M0 slice + M1 ingest wiring).
//!
//! Modes:
//!   * windowed  : winit + wgpu surface + egui overlay (the real app)
//!   * headless  : offscreen render → PNG screenshot (CI / golden renders /
//!                 sandboxes without a display)

use anyhow::{anyhow, bail, Result};
use glam::Vec3;
use hs_camera::{Camera, Mode};
use hs_forge::{Beacon, ForgeParams, Region};
use hs_ingest::geocode::{parse_nominatim, parse_photon, parse_raw_coords, nominatim_url, photon_url};
use hs_ingest::{load_dem, plan_region, RegionPlan};
use hs_lod::{build_lod, enforce_budget, LodRegion, Traversal, COL_SIZE};
use hs_render::{AdapterPrefs, Renderer, Settings};
use hs_ui::UiState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const UPLOAD_PER_FRAME: u64 = 48 * 1024 * 1024; // plan NFR-05
const POINT_BUDGET: u32 = 20_000_000; // plan §7.3 default @1440p
const VRAM_BUDGET_GB: f32 = 14.0; // plan NFR-02

struct Args {
    headless: bool,
    out: String,
    width: u32,
    height: u32,
    points: u32,
    palette: String,
    ui: bool,
    frames: u32,
    hide: Vec<String>,
    place: Option<String>,
    extent_km: f32,
    vectors: bool,
    bench: Option<String>,
    clarity: bool,
    bg: u32,
    sweep: bool,
    eye: Option<(f32, f32)>,
    help: bool,
    soak_min: f32,
    density: f32,
    no_lidar: bool,
    las: Option<String>,
    lidar_url: Option<String>,
    las_center: Option<(f64, f64)>,
    lidar_max: u32,
    software: bool,
    adapter: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        headless: false,
        out: "renders/holoscope_m0.png".into(),
        width: 1200,
        height: 900,
        points: 10_000_000,
        palette: "hologreen".into(),
        ui: true,
        frames: 1,
        hide: Vec::new(),
        place: None,
        extent_km: 10.0,
        vectors: false,
        bench: None,
        clarity: false,
        bg: 0,
        sweep: false,
        eye: None,
        help: false,
        soak_min: 0.0,
        density: 2.0,
        no_lidar: false,
        las: None,
        lidar_url: None,
        las_center: None,
        lidar_max: 4_000_000,
        software: std::env::var("HOLOSCOPE_SOFTWARE").is_ok(),
        adapter: std::env::var("HOLOSCOPE_ADAPTER").ok(),
    };
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--headless" => a.headless = true,
            "--no-ui" => a.ui = false,
            "--out" => {
                a.out = argv.get(i + 1).cloned().unwrap_or(a.out);
                i += 1;
            }
            "--width" => {
                a.width = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.width);
                i += 1;
            }
            "--height" => {
                a.height = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.height);
                i += 1;
            }
            "--points" => {
                a.points = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.points);
                i += 1;
            }
            "--palette" => {
                a.palette = argv.get(i + 1).cloned().unwrap_or(a.palette.clone());
                i += 1;
            }
            "--frames" => {
                a.frames = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.frames);
                i += 1;
            }
            "--hide" => {
                a.hide = argv
                    .get(i + 1)
                    .map(|v| v.split(',').map(|t| t.trim().to_string()).collect())
                    .unwrap_or_default();
                i += 1;
            }
            "--place" => {
                a.place = argv.get(i + 1).cloned();
                i += 1;
            }
            "--extent" => {
                a.extent_km = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.extent_km);
                i += 1;
            }
            "--vectors" => a.vectors = true,
            "--clarity" => a.clarity = true,
            "--sweep" => a.sweep = true,
            "--help" => a.help = true,
            "--no-lidar" => a.no_lidar = true,
            "--density" => {
                a.density = match argv.get(i + 1).map(|s| s.as_str()) {
                    Some("low") => 0.5,
                    Some("med") => 1.0,
                    Some("high") => 2.0,
                    Some("ultra") => 4.0,
                    Some(v) => v.parse().ok().unwrap_or(a.density),
                    None => a.density,
                };
                i += 1;
            }
            "--las" => {
                a.las = argv.get(i + 1).cloned();
                i += 1;
            }
            "--lidar-url" => {
                a.lidar_url = argv.get(i + 1).cloned();
                i += 1;
            }
            "--las-center" => {
                a.las_center = argv.get(i + 1).and_then(|v| {
                    let mut it = v.split(',');
                    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                });
                i += 1;
            }
            "--software" | "-sw" => a.software = true,
            "--adapter" => {
                a.adapter = argv.get(i + 1).cloned();
                i += 1;
            }
            "--lidar-max" => {
                a.lidar_max = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.lidar_max);
                i += 1;
            }
            "--soak" => {
                a.soak_min = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(1.0);
                i += 1;
            }
            "--eye" => {
                a.eye = argv.get(i + 1).and_then(|v| {
                    let mut it = v.split(',');
                    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                });
                i += 1;
            }
            "--bg" => {
                a.bg = argv.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(a.bg);
                i += 1;
            }
            "--bench" => {
                a.bench = argv.get(i + 1).cloned();
                i += 1;
            }
            other => eprintln!("[args] ignoring unknown flag {other}"),
        }
        i += 1;
    }
    a
}

fn forge_demo(points: u32) -> (hs_geo::HeightField, Region, f64) {
    let t0 = Instant::now();
    let hf = hs_geo::demo_heightfield(4000.0, 8.0);
    let region = hs_forge::forge_region(
        &hf,
        &ForgeParams {
            target_points: points,
            ..Default::default()
        },
    );
    (hf, region, t0.elapsed().as_secs_f64())
}

fn demo_camera() -> Camera {
    let mut cam = Camera::default();
    let pos = Vec3::new(-3500.0, -4200.0, 3100.0);
    let target = Vec3::new(120.0, 320.0, 120.0);
    let dir = (target - pos).normalize();
    cam.pos = pos;
    cam.yaw = dir.y.atan2(dir.x);
    cam.pitch = dir.z.asin();
    cam.fov_deg = 40.0;
    cam
}

/// 3/4 hero framing for any region size.
fn frame_camera(size: f32, max_h: f32) -> Camera {
    let mut cam = Camera::default();
    let pos = Vec3::new(-0.88 * size, -1.05 * size, 0.62 * size + max_h);
    let target = Vec3::new(0.03 * size, 0.08 * size, max_h * 0.25);
    let dir = (target - pos).normalize();
    cam.pos = pos;
    cam.yaw = dir.y.atan2(dir.x);
    cam.pitch = dir.z.asin();
    cam.fov_deg = 40.0;
    cam.orbit_target = target;
    cam.orbit_dist = (target - pos).length();
    cam
}

fn settings_from_ui(ui: &UiState) -> Settings {
    Settings {
        point_size_px: ui.point_size,
        bloom: ui.bloom,
        exposure: ui.exposure,
        z_exag: ui.z_exag,
        intensity: ui.intensity,
        holo: ui.holo,
        scanlines: ui.scanlines,
        scan_sweep: ui.scan_sweep,
        hardness: ui.hardness,
        edl_strength: ui.edl_strength,
        edl_radius: ui.edl_radius,
        bg_mode: ui.bg_mode,
        ..Default::default()
    }
}

// ------------------------------------------------------------------ governor
/// Closed-loop adaptive quality controller (plan §12.3): EMA of GPU frame time
/// vs target; actuators in order — point budget ×, sprite min-px, bloom mip,
/// tier pixel threshold. Fast cut (3 bad frames), slow restore (90 good).
#[derive(Clone, Debug)]
struct Governor {
    enabled: bool,
    target_ms: f32,
    budget_mul: f32,
    min_px: f32,
    bloom_skip: u32,
    thr: f32,
    ema: f32,
    good: u32,
    bad: u32,
}

impl Governor {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            target_ms: 1000.0 / 60.0,
            budget_mul: 1.0,
            min_px: 1.0,
            bloom_skip: 0,
            thr: 1.4,
            ema: 0.0,
            good: 0,
            bad: 0,
        }
    }
    fn sample(&mut self, ms: f32) {
        if !self.enabled || ms <= 0.0 {
            return;
        }
        self.ema = if self.ema <= 0.0 { ms } else { self.ema * 0.9 + ms * 0.1 };
        if self.ema > self.target_ms * 1.10 {
            self.bad += 1;
            self.good = 0;
            if self.bad >= 3 {
                self.cut();
                self.bad = 0;
            }
        } else if self.ema < self.target_ms * 0.85 {
            self.good += 1;
            self.bad = 0;
            if self.good >= 90 {
                self.restore();
                self.good = 0;
            }
        } else {
            self.good = 0;
            self.bad = 0;
        }
    }
    fn cut(&mut self) {
        if self.budget_mul > 0.41 {
            self.budget_mul = (self.budget_mul * 0.8).max(0.4);
        } else if self.min_px > 0.76 {
            self.min_px = 0.75;
        } else if self.bloom_skip == 0 {
            self.bloom_skip = 1;
        } else {
            self.thr = 2.0;
        }
    }
    fn restore(&mut self) {
        if self.thr > 1.5 {
            self.thr = 1.4;
        } else if self.bloom_skip == 1 {
            self.bloom_skip = 0;
        } else if self.min_px < 0.99 {
            self.min_px = 1.0;
        } else if self.budget_mul < 1.49 {
            self.budget_mul = (self.budget_mul * 1.1).min(1.5);
        }
    }
    fn label(&self) -> String {
        if !self.enabled {
            return "manual".into();
        }
        format!(
            "budget {:.2}× · minpx {:.2} · bloom-{} · thr {:.1}",
            self.budget_mul, self.min_px, self.bloom_skip, self.thr
        )
    }
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

// ------------------------------------------------------------------ fly-to & viewpoints
/// Animated ease between camera poses (position lerp + shortest-arc angles).
#[derive(Clone, Copy, Debug)]
struct FlyTo {
    from_pos: Vec3,
    to_pos: Vec3,
    from_yaw: f32,
    to_yaw: f32,
    from_pitch: f32,
    to_pitch: f32,
    t: f32,
    dur: f32,
}

impl FlyTo {
    fn new(cam: &Camera, to_pos: Vec3, to_yaw: f32, to_pitch: f32, dur: f32) -> Self {
        let mut dy = (to_yaw - cam.yaw) % std::f32::consts::TAU;
        if dy > std::f32::consts::PI {
            dy -= std::f32::consts::TAU;
        }
        if dy < -std::f32::consts::PI {
            dy += std::f32::consts::TAU;
        }
        Self {
            from_pos: cam.pos,
            to_pos,
            from_yaw: cam.yaw,
            to_yaw: cam.yaw + dy,
            from_pitch: cam.pitch,
            to_pitch,
            t: 0.0,
            dur: dur.max(0.1),
        }
    }
    /// Returns pose while active; None when finished.
    fn step(&mut self, dt: f32) -> Option<(Vec3, f32, f32)> {
        self.t += dt;
        let k = (self.t / self.dur).clamp(0.0, 1.0);
        let e = k * k * (3.0 - 2.0 * k);
        if k >= 1.0 {
            return None;
        }
        Some((
            self.from_pos.lerp(self.to_pos, e),
            self.from_yaw + (self.to_yaw - self.from_yaw) * e,
            self.from_pitch + (self.to_pitch - self.from_pitch) * e,
        ))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Viewpoint {
    name: String,
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    fov: f32,
}

// ------------------------------------------------------------------ data dir
fn data_dir() -> PathBuf {
    if let Ok(ld) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(ld).join("HoloScope").join("cache");
    }
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("holoscope");
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".cache")
        .join("holoscope")
}

// ------------------------------------------------------------------ job msgs
pub enum JobMsg {
    Ac(Vec<(String, String, f64, f64)>),
    Progress(String, f32),
    Region(
        Box<Region>,
        String,
        String,
        hs_geo::HeightField,
        Vec<(String, Vec3, u8)>,
        Option<hs_forge::VectorData>,
        Option<hs_forge::Node>,
    ),
    Failed(String),
}

fn scaled_beacons(size: f32) -> Vec<Beacon> {
    [(-0.09, -0.24), (0.19, -0.14), (0.34, 0.06)]
        .iter()
        .map(|(x, y)| Beacon { x: x * size, y: y * size })
        .collect()
}

/// Synchronous M1 load pipeline (used by the job thread and by headless mode).
fn load_sync(
    cache: &hs_cache::Cache,
    client: &hs_ingest::http::Client,
    query: &str,
    resolved: Option<(String, f64, f64)>,
    extent_km: f32,
    points: u32,
    want_vectors: bool,
    vd: f32,
    want_lidar: bool,
    cancel: &AtomicBool,
    progress: &dyn Fn(&str, f32),
) -> Result<(
    Region,
    String,
    String,
    hs_geo::HeightField,
    Vec<(String, Vec3, u8)>,
    Option<hs_forge::VectorData>,
    Option<hs_forge::Node>,
)> {
    progress("resolve", 0.05);
    let (label, lat, lon, bbox) = match resolved {
        Some((l, la, lo)) => (l, la, lo, None),
        None => {
            if let Some((la, lo)) = parse_raw_coords(query) {
                (query.to_string(), la, lo, None)
            } else {
                let v = if let Some(j) = cache.geocode_get("nominatim", query)? {
                    serde_json::from_str(&j)?
                } else {
                    let v = client.get_json(&nominatim_url(query))?;
                    cache.geocode_put("nominatim", query, &v.to_string())?;
                    v
                };
                let p = parse_nominatim(&v).ok_or_else(|| anyhow!("place not found: {query}"))?;
                (p.label, p.lat, p.lon, p.bbox)
            }
        }
    };
    let ext = extent_km as f64 * 1000.0;
    let cos = (lat.to_radians().cos()).max(0.05);
    // EXTENT IS CENTERED ON THE RESOLVED POINT — the ENU origin and the DEM
    // window must share one center or the heightfield clamps flat and OSM
    // vertices leave the quantization range (BUILD_LOG BUG-M2-1).
    // (Nominatim bboxes are admin-boundary suggestions, e.g. all of Kyoto
    // city incl. rural north — never use their center.)
    let _ = &bbox;
    let dlat = ext * 0.5 / 111_320.0;
    let dlon = ext * 0.5 / (111_320.0 * cos);
    let (lat0, lat1, lon0, lon1) = (lat - dlat, lat + dlat, lon - dlon, lon + dlon);
    progress("plan", 0.10);
    let plan: RegionPlan = plan_region(lat0, lat1, lon0, lon1);
    let enu = hs_geo::Enu::new(lat, lon, 0.0);
    let hf = load_dem(
        cache,
        client,
        &plan,
        &enu,
        ext,
        &|| cancel.load(Ordering::Relaxed),
        &|i, n| {
            progress(
                &format!("DEM {}/{}", i + 1, n),
                0.10 + 0.80 * (i as f32 / (n as f32).max(1.0)),
            );
        },
    )?;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    // M2: real vector layers via Overpass (bounded extent, cache-first)
    let mut osm_used = false;
    let mut labels_enu: Vec<(String, Vec3, u8)> = Vec::new();
    let osm = if want_vectors && ext <= 10_000.0 {
        progress("vectors", 0.88);
        match hs_ingest::osm::fetch_osm(cache, client, [lat0, lon0, lat1, lon1]) {
            Ok(data) => {
                use hs_geo::geodetic_to_ecef;
                for n in &data.nodes {
                    // prefer name:en (Japanese features rarely have ASCII `name`)
                    let name = n
                        .tags
                        .get("name:en")
                        .cloned()
                        .or_else(|| n.tags.get("name").filter(|v| v.is_ascii()).cloned())
                        .unwrap_or_default();
                    if name.is_empty() || !name.is_ascii() {
                        continue;
                    }
                    let rank = match n.tags.get("place").map(|s| s.as_str()) {
                        Some("city") => 0,
                        Some("town") => 1,
                        Some("village") => 2,
                        Some("suburb") => 3,
                        _ => 4,
                    };
                    let e = enu.ecef_to_enu(geodetic_to_ecef(n.lat, n.lon, 0.0));
                    let gz = hf.sample(e.x as f32, e.y as f32) + 60.0;
                    labels_enu.push((name, Vec3::new(e.x as f32, e.y as f32, gz), rank));
                }
                let vd = osm_to_vector(&data, &enu, ext as f32 * 0.5);
                osm_used = true;
                progress("vectors", 0.91);
                Some(vd)
            }
            Err(e) => {
                progress(&format!("vectors skipped"), 0.91);
                eprintln!("[load] vectors skipped: {e}");
                None
            }
        }
    } else {
        None
    };
    // M6.5: auto LiDAR from the free OpenTopography catalog (keyless):
    // catalog → tile index → LAZ tiles, central 3 km window, ≤4 tiles.
    let mut lidar_node: Option<hs_forge::Node> = None;
    let mut lidar_src = String::new();
    if want_lidar {
        let half = (extent_km.min(3.0) as f64 * 1000.0) * 0.5 / 111_320.0;
        let lbox = [lon - half * 1.4, lat - half, lon + half * 1.4, lat + half];
        match hs_ingest::ot::fetch_lidar(client, cache, lbox, 4, &|st, p| {
            progress(st, 0.915 + 0.005 * p);
        }) {
            Ok((pts, name)) => {
                let n = pts.len();
                lidar_node = Some(las_points_to_node(&pts, None, ext as f32, 1.0));
                lidar_src = name;
                progress(&format!("lidar {n} pts"), 0.92);
            }
            Err(e) => {
                progress(&format!("lidar skipped ({e})"), 0.92);
                eprintln!("[load] auto-lidar: {e}");
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    progress("forge", 0.92);
    let osm_for_ret = osm.clone();
    let mut region = hs_forge::forge_region(
        &hf,
        &ForgeParams {
            size_m: ext as f32,
            target_points: points,
            vectors: false,
            vector_density: vd,
            osm,
            beacons: scaled_beacons(ext as f32),
            ..Default::default()
        },
    );
    if let Some(n) = lidar_node.take() {
        region.point_count += n.points.len() as u64;
        region.nodes.push(n);
    }
    let mut short = label.split(',').next().unwrap_or(&label).to_string();
    if !short.is_ascii() || short.trim().is_empty() {
        short = query.to_string(); // keep HUD glyph-safe (egui default fonts)
    }
    let mut attr = hs_ingest::attribution(osm_used);
    if !lidar_src.is_empty() {
        attr.push_str(&format!(" · LiDAR: OpenTopography/{lidar_src} (per-dataset licence)"));
    }
    let labels = labels_enu;
    Ok((region, short, attr, hf, labels, osm_for_ret, lidar_node))
}

/// Overpass ways → region-local ENU `VectorData` (plan §6.2 class mapping).
/// Ways are clipped to the region extent: Overpass returns full way geometry,
/// so unclipped vertices spray streaks far outside the slab (BUG-M2-2).
fn osm_to_vector(data: &hs_ingest::osm::OsmData, enu: &hs_geo::Enu, half: f32) -> hs_forge::VectorData {
    use hs_geo::geodetic_to_ecef;
    let xy = |lat: f64, lon: f64| -> [f32; 2] {
        let e = enu.ecef_to_enu(geodetic_to_ecef(lat, lon, 0.0));
        [e.x as f32, e.y as f32]
    };
    let inside = |p: [f32; 2]| p[0].abs() <= half && p[1].abs() <= half;
    // split a polyline into runs of in-extent vertices
    let clipped_runs = |w: &hs_ingest::osm::OsmWay| -> Vec<Vec<[f32; 2]>> {
        let mut runs = Vec::new();
        let mut cur: Vec<[f32; 2]> = Vec::new();
        for (la, lo) in &w.geom {
            let p = xy(*la, *lo);
            if inside(p) {
                cur.push(p);
            } else if cur.len() >= 2 {
                runs.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
        if cur.len() >= 2 {
            runs.push(cur);
        }
        runs
    };
    let clipped_poly = |w: &hs_ingest::osm::OsmWay| -> Option<Vec<[f32; 2]>> {
        let ring: Vec<[f32; 2]> = w.geom.iter().map(|(la, lo)| xy(*la, *lo)).collect();
        if ring.iter().all(|p| inside(*p)) {
            Some(ring)
        } else {
            let c = hs_forge::clip_poly_to_extent(&ring, half);
            if c.len() >= 4 {
                Some(c)
            } else {
                None
            }
        }
    };
    let line = |w: &hs_ingest::osm::OsmWay| w.geom.iter().map(|(la, lo)| xy(*la, *lo)).collect::<Vec<_>>();
    let _ = &line;
    let mut vd = hs_forge::VectorData::default();
    for w in &data.ways {
        if let Some(hw) = w.tag("highway") {
            let rank = match hw {
                "motorway" | "trunk" => 0,
                "primary" => 1,
                "secondary" | "tertiary" => 2,
                "path" | "cycleway" | "footway" | "steps" | "track" => 4,
                "proposed" | "construction" | "platform" => continue,
                _ => 3,
            };
            for run in clipped_runs(w) {
                vd.roads.push((rank, run));
            }
        } else if let Some(rw) = w.tag("railway") {
            if matches!(rw, "rail" | "tram") {
                vd.rail.extend(clipped_runs(w));
            }
        } else if w.tag("natural") == Some("water") || w.tag("waterway") == Some("riverbank") {
            if w.closed() {
                if let Some(ring) = clipped_poly(w) {
                    vd.water_polys.push(ring);
                }
            }
        } else if let Some(ww) = w.tag("waterway") {
            let width = match ww {
                "canal" => 10.0,
                "river" => 8.0,
                "stream" => 2.0,
                _ => 4.0,
            };
            for run in clipped_runs(w) {
                vd.water_lines.push((width, run));
            }
        } else if w.tag("building").is_some() {
            if w.closed() {
                let h = w
                    .tag("height")
                    .and_then(|h| h.trim().trim_end_matches('m').trim().parse::<f32>().ok())
                    .or_else(|| w.tag("levels").and_then(|l| l.parse::<f32>().ok()).map(|l| l * 3.0))
                    .unwrap_or_else(|| match w.tag("building").unwrap_or("") {
                        "commercial" | "retail" | "office" => 10.0,
                        "industrial" | "warehouse" => 8.0,
                        "residential" | "apartments" | "house" => 7.0,
                        "garage" | "shed" => 4.0,
                        _ => 6.0,
                    });
                vd.buildings.push((h, line(w))); // buildings are small; closed+inside checked below
            }
        } else if w.tag("leisure").is_some()
            || w.tag("natural") == Some("wood")
            || w.tag("natural") == Some("scrub")
        {
            if w.closed() {
                if let Some(ring) = clipped_poly(w) {
                    vd.veg_polys.push(ring);
                }
            }
        }
    }
    vd
}

fn spawn_autocomplete(cache: Arc<hs_cache::Cache>, client: Arc<hs_ingest::http::Client>, q: String, tx: Sender<JobMsg>) {
    std::thread::Builder::new()
        .name("hs-ac".into())
        .spawn(move || {
            let res = (|| -> Result<Vec<(String, String, f64, f64)>> {
                let v = if let Some(j) = cache.geocode_get("photon", &q)? {
                    serde_json::from_str(&j)?
                } else {
                    let v = client.get_json(&photon_url(&q))?;
                    cache.geocode_put("photon", &q, &v.to_string())?;
                    v
                };
                Ok(parse_photon(&v)
                    .into_iter()
                    .map(|h| (h.label, h.sub, h.lat, h.lon))
                    .collect())
            })();
            if let Ok(hits) = res {
                let _ = tx.send(JobMsg::Ac(hits));
            }
        })
        .ok();
}

/// Convert decoded LAS points (geographic via ENU, or projected/local) into
/// a region node. Classification drives colour: ground→elevation ramp,
/// buildings/veg→their slots, rest→imported amber.
fn las_points_to_node(
    pts_in: &[hs_ingest::lidar::LasPoint],
    enu: Option<&hs_geo::Enu>,
    extent: f32,
    stride: f32,
) -> hs_forge::Node {
    let n0 = pts_in.len();
    let mut xs = pts_in.iter().map(|p| p.x);
    let mut ys = pts_in.iter().map(|p| p.y);
    let c = [
        (xs.clone().fold(f64::MAX, f64::min) + xs.fold(f64::MIN, f64::max)) * 0.5,
        (ys.clone().fold(f64::MAX, f64::min) + ys.fold(f64::MIN, f64::max)) * 0.5,
        0.0,
    ];
    let geo = enu.is_some();
    let zmin = pts_in.iter().map(|p| p.z).fold(f64::INFINITY, f64::min);
    let zmax = pts_in.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max);
    let zspan = (zmax - zmin).max(1.0);
    // ground reference = median of ground-class z (fallback: min) so the scan
    // can be sat on any region's terrain at merge time (BUG-M6.5-3)
    let mut gz: Vec<f64> = pts_in
        .iter()
        .filter(|p| p.classification == 2)
        .map(|p| p.z)
        .collect();
    gz.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let ground_ref = gz.get(gz.len() / 2).copied().unwrap_or(zmin);
    let mut pts: Vec<([f32; 3], u8, u8, u8)> = Vec::with_capacity((n0 as f32 / stride) as usize);
    let mut i = 0usize;
    while i < n0 {
        let p = &pts_in[i];
        let (x, y, z) = if geo {
            let e = enu.expect("geo mode implies enu");
            use hs_geo::geodetic_to_ecef;
            let v = e.ecef_to_enu(geodetic_to_ecef(p.y, p.x, p.z));
            (v.x as f32, v.y as f32, v.z as f32)
        } else {
            // projected/local coords: centre horizontally; z stays relative
            // to the scan's own ground and is aligned at merge time
            (
                p.x as f32 - c[0] as f32,
                p.y as f32 - c[1] as f32,
                p.z as f32 - ground_ref as f32,
            )
        };
        let t = ((p.z - zmin) / zspan).clamp(0.0, 1.0) as f32;
        let (color, inten) = match p.classification {
            2 => ((t * 63.0) as u8, 85),                  // ground → elevation ramp
            6 | 17 => (74, 150),                          // building
            3 | 4 | 5 => (75, 100),                       // vegetation
            9 | 13 => (73, 130),                          // water
            _ => (81, 95),                                // unclassified/other
        };
        let inten = ((inten as f32) * (0.6 + 0.4 * (p.intensity as f32 / 250.0).min(1.0))) as u8;
        pts.push(([x, y, z], color, 0, inten));
        i = i + stride as usize;
    }
    eprintln!(
        "[lidar] {} points (of {n0}, stride {stride:.1}) → node, z span {zspan:.1} m",
        pts.len()
    );
    let mut node =
        hs_forge::node_from_world("imported-lidar", hs_forge::layer::IMPORTED, extent.max(100.0), pts);
    node.spacing_hint = 1.0;
    // stash the ground reference in origin.w; merge sets origin.z per region
    node.origin = [0.0, 0.0, 0.0, ground_ref as f32];
    eprintln!("[lidar] ground_ref={ground_ref:.2} m, z span {zspan:.1} m");
    node
}

/// Load a LAS file (path or URL, cached) and convert to a region node
/// (plan FR-12).
fn load_lidar_node(
    cache: &hs_cache::Cache,
    client: &hs_ingest::http::Client,
    a: &Args,
    extent: f32,
) -> Option<hs_forge::Node> {
    let bytes = if let Some(p) = &a.las {
        std::fs::read(p).ok()?
    } else if let Some(u) = &a.lidar_url {
        let uu = u.clone();
        cache
            .blob_fetch("lidar-las", &uu, || client.get_range(&uu, None))
            .ok()?
    } else {
        return None;
    };
    let las = hs_ingest::lidar::parse_las(&bytes).ok()?;
    let n0 = las.points.len();
    let stride = (n0 as f32 / a.lidar_max.max(100_000) as f32).max(1.0);
    let enu = a.las_center.map(|(la, lo)| hs_geo::Enu::new(la, lo, 0.0));
    let geo = las.looks_geographic() && enu.is_some();
    let node = las_points_to_node(&las.points, if geo { enu.as_ref() } else { None }, extent, stride);
    Some(node)
}

/// Clone the lidar node into a region, sitting it on the region terrain
/// (origin.z = terrain height at region centre minus the scan ground ref).
fn attach_lidar(region: &mut Region, lidar: &hs_forge::Node, hf: &hs_geo::HeightField) {
    let mut n = lidar.clone();
    n.origin[2] = hf.sample(0.0, 0.0); // z already baked relative to scan ground
    region.point_count += n.points.len() as u64;
    region.nodes.push(n);
}

fn adapter_prefs(a: &Args) -> hs_render::AdapterPrefs {
    hs_render::AdapterPrefs {
        software: a.software,
        name: a.adapter.clone(),
    }
}

// ------------------------------------------------------------------ headless
fn run_headless(a: &Args) -> Result<()> {
    let cache = Arc::new(hs_cache::Cache::open(&data_dir())?);
    let client = Arc::new(hs_ingest::http::Client::default());

    let (region, region_name, attribution, hf, place_labels, _osm_vd, lidar_file_node) = match &a.place {
        Some(place) => {
            let t0 = Instant::now();
            let (r, name, attr, hf, labels, osm_vd, lidar) = load_sync(
                &cache,
                &client,
                place,
                None,
                a.extent_km,
                a.points,
                a.vectors,
                a.density,
                !a.no_lidar,
                &AtomicBool::new(false),
                &|stage, pct| eprintln!("[load] {stage} {:.0}%", pct * 100.0),
            )?;
            eprintln!("[load] {} in {:.2}s (cache-first)", name, t0.elapsed().as_secs_f64());
            (r, name, attr, hf, labels, osm_vd, lidar)
        }
        None => {
            let (hf, region, forge_s) = forge_demo(a.points);
            eprintln!("[forge] demo region {} points in {:.2}s", region.point_count, forge_s);
            (
                region,
                "demo_valley_4km".to_string(),
                String::new(),
                hf,
                Vec::new(),
                None,
                None,
            )
        }
    };
    let _ = (&hf, &place_labels);
    let mut region = region;
    let file_lidar = load_lidar_node(&cache, &client, a, region.size_m);
    let auto_lidar = lidar_file_node.or(file_lidar);
    if let Some(node) = &auto_lidar {
        attach_lidar(&mut region, node, &hf);
    }
    eprintln!(
        "[forge] {} points, {} nodes, {} line verts, {} quad verts",
        region.point_count,
        region.nodes.len(),
        region.lines.len(),
        region.quads.len()
    );

    let instance = Renderer::create_instance();
    let mut renderer = pollster::block_on(Renderer::new_with_prefs(
        &instance,
        None,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &adapter_prefs(a),
    ))?;
    eprintln!("[gpu] adapter: {}", renderer.adapter_name);
    renderer.set_palette(&a.palette);
    let mut lod = build_lod(&region, COL_SIZE);
    let dropped = enforce_budget(&mut lod, (VRAM_BUDGET_GB * 1e9) as u64);
    eprintln!(
        "[lod] {} nodes, pool {:.1} MB (dropped finest chunks: {dropped})",
        lod.nodes.len(),
        lod.pool_bytes as f64 / 1e6
    );
    renderer.upload_lod(&lod);
    renderer.upload_furniture(&region);
    renderer.resize(a.width, a.height);
    let mut upload_off: u64 = 0;
    let mut traversal = Traversal::new();

    let off = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: a.width,
            height: a.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let off_view = off.create_view(&wgpu::TextureViewDescriptor::default());

    let mut cam = frame_camera(region.size_m, region.max_h);
    let mut ui_state = UiState {
        palette: a.palette.clone(),
        layer_tint: std::array::from_fn(|l| {
            hs_forge::palette::layer_default_color(&a.palette, l as u8)
        }),
        adapter: renderer.adapter_name.clone(),
        points: region.point_count,
        region: region_name.clone(),
        attribution: attribution.clone(),
        extent_km: region.size_m / 1000.0,
        holo: !a.clarity,
        bg_mode: a.bg,
        scan_sweep: a.sweep,
        help_open: a.help,
        ..Default::default()
    };
    if let Some((ex, ey)) = a.eye {
        // street-level hero pose (walk mode eye height).
        // NB: shader exaggerates world z, so camera heights must match (BUG-M5-1).
        let g = hf.sample(ex, ey) * ui_state.z_exag;
        cam.pos = Vec3::new(ex, ey, g + 1.7);
        let look = Vec3::new(0.0, 0.0, region.max_h * 0.35 * ui_state.z_exag);
        let dir = (look - cam.pos).normalize();
        cam.yaw = dir.y.atan2(dir.x);
        cam.pitch = (dir.z.asin()).clamp(-0.2, 0.2);
    }
    for h in &a.hide {
        let id = match h.as_str() {
            "terrain" => hs_forge::layer::TERRAIN,
            "contours" => hs_forge::layer::CONTOUR,
            "roads" => hs_forge::layer::ROAD,
            "water" => hs_forge::layer::WATER,
            "buildings" => hs_forge::layer::BUILDING,
            "veg" => hs_forge::layer::VEG,
            "furniture" => hs_forge::layer::FURNITURE,
            _ => continue,
        };
        ui_state.layers[id as usize] = false;
        eprintln!("[diag] hid layer {h}");
    }
    let ctx = egui::Context::default();
    hs_ui::apply_theme(&ctx);
    let screen_rect =
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(a.width as f32, a.height as f32));

    if let Some((ex, ey)) = a.eye {
        let nid = renderer.near_node_id();
        let (verts, q) = build_nearfield(&hf, ex, ey, region.max_h, nid);
        renderer.upload_nearfield(&verts, [ex, ey, 0.0], q);
        eprintln!("[nearfield] {} pts around eye", verts.len());
    }
    let _ = &hf;

    // soak mode (plan §14): idle-orbit frames + RSS/GPU sampling to bench/soak.csv
    let soak_frames = (a.soak_min * 60.0) as u32;
    if soak_frames > 0 {
        std::fs::create_dir_all("bench").ok();
        let mut csv = String::from("frame,cpu_ms,gpu_ms,rss_kb,points\n");
        let mut gov = Governor::new(true);
        let mut cam = cam.clone();
        let rss0 = rss_kb();
        for f in 0..soak_frames {
            let t0 = Instant::now();
            cam.yaw += 0.002; // idle orbit drift
            let sels = traversal.select(
                &lod,
                &cam.view_proj(a.width as f32 / a.height as f32),
                cam.pos,
                cam.yaw,
                cam.pitch,
                (a.height as f32 * 0.5) / (cam.fov_deg.to_radians() * 0.5).tan(),
                1.0 / 60.0,
                1.4,
                POINT_BUDGET,
                &|ni| renderer.node_uploaded(ni),
            );
            let stats = renderer.render(
                &off_view,
                &cam,
                &settings_from_ui(&ui_state),
                f as f32 / 60.0,
                &ui_state.layers,
                &hs_render::LayerTune::default(),
                &sels,
            );
            let cpu = t0.elapsed().as_secs_f32() * 1000.0;
            let gpu = renderer.read_gpu_ms().unwrap_or(0.0);
            gov.sample(gpu.max(cpu * 0.5));
            if f % 60 == 0 || f + 1 == soak_frames {
                let line = format!("{},{:.1},{:.1},{},{}\n", f, cpu, gpu, rss_kb(), stats.points_drawn);
                eprint!("[soak] {}", line);
                csv.push_str(&line);
            }
        }
        std::fs::write("bench/soak.csv", &csv)?;
        let rss1 = rss_kb();
        eprintln!(
            "[soak] done {} frames · rss {} → {} KB (Δ {} KB) · gov: {}",
            soak_frames,
            rss0,
            rss1,
            rss1 as i64 - rss0 as i64,
            gov.label()
        );
        return Ok(());
    }

    let mut last_png = String::new();
    for frame in 0..a.frames.max(1) {
        let t0 = Instant::now();
        let time = frame as f32 / 60.0;
        let settings = settings_from_ui(&ui_state);
        let tune = hs_render::LayerTune {
            size_mul: ui_state.layer_size,
            intensity: ui_state.layer_inten,
        };
        // progressive pool upload (≤48 MB/frame; single-frame runs drain fully)
        let pump_chunks = if a.frames > 1 { 1 } else { 64 };
        for _ in 0..pump_chunks {
            if upload_off >= lod.pool_bytes {
                break;
            }
            let end = (upload_off + UPLOAD_PER_FRAME).min(lod.pool_bytes);
            let v0 = (upload_off / 16) as usize;
            let v1 = (end / 16) as usize;
            renderer.write_pool(upload_off, bytemuck::cast_slice(&lod.instances[v0..v1]));
            upload_off = end;
        }
        let sels = traversal.select(
            &lod,
            &cam.view_proj(a.width as f32 / a.height as f32),
            cam.pos,
            cam.yaw,
            cam.pitch,
            (a.height as f32 * 0.5) / (cam.fov_deg.to_radians() * 0.5).tan(),
            1.0, // stills: settle cross-fades in one frame
            if settings.holo { 1.4 } else { 1.0 },
            POINT_BUDGET,
            &|ni| renderer.node_uploaded(ni),
        );
        ui_state.uploaded_pct = renderer.pool_progress() * 100.0;
        ui_state.pool_mb = lod.pool_bytes as f32 / 1e6;
        ui_state.trav_ms = traversal.last_ms;
        let stats = renderer.render(&off_view, &cam, &settings, time, &ui_state.layers, &tune, &sels);

        if a.ui {
            let raw = egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            };
            ui_state.fps = 60.0;
            ui_state.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
            ui_state.labels_screen =
                project_labels(&place_labels, &cam, a.width, a.height, Some(&hf));
            ui_state.beacon_count = 3;
            ui_state.points = stats.points_drawn;
            ui_state.nodes_drawn = stats.nodes_drawn;
            let full = ctx.run(raw, |ctx| hs_ui::draw(ctx, &mut ui_state));
            for (id, img) in full.textures_delta.set {
                renderer
                    .egui_renderer
                    .update_texture(&renderer.device, &renderer.queue, id, &img);
            }
            let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
            let sd = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [a.width, a.height],
                pixels_per_point: 1.0,
            };
            let mut enc = renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ui") });
            renderer
                .egui_renderer
                .update_buffers(&renderer.device, &renderer.queue, &mut enc, &prims, &sd);
            {
                let mut rp = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("ui-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &off_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                renderer.egui_renderer.render(&mut rp, &prims, &sd);
            }
            renderer.queue.submit(Some(enc.finish()));
            for id in full.textures_delta.free {
                renderer.egui_renderer.free_texture(&id);
            }
        }

        let px = renderer.download_rgba8(&off, a.width, a.height);
        let path = if a.frames <= 1 {
            a.out.clone()
        } else {
            format!("{}_{:02}.png", a.out.trim_end_matches(".png"), frame)
        };
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        image::save_buffer(&path, &px, a.width, a.height, image::ColorType::Rgba8)?;
        last_png = path;
        eprintln!(
            "[frame {}] {:.1} ms · {} pts · {} nodes",
            frame,
            t0.elapsed().as_secs_f64() * 1000.0,
            stats.points_drawn,
            stats.nodes_drawn
        );
    }
    println!("{}", last_png);
    Ok(())
}

/// Dense 0.6 m jittered patch around (cx, cy), quantized at 2 cm (plan §1.2).
fn build_nearfield(hf: &hs_geo::HeightField, cx: f32, cy: f32, max_h: f32, node_id: u32) -> (Vec<hs_forge::ImplVertex>, f32) {
    let radius = 60.0f32;
    let spacing = 0.6f32;
    let q = 0.02f32;
    let inv = 1.0 / q;
    let mut verts: Vec<hs_forge::ImplVertex> = Vec::new();
    let mut gy = 0u32;
    let mut y = cy - radius;
    while y <= cy + radius {
        let mut gx = 0u32;
        let mut x = cx - radius;
        while x <= cx + radius {
            let jx = x + (jitter01(gx, 0xA53) - 0.5) * spacing;
            let jy = y + (jitter01(gy, 0xB71) - 0.5) * spacing;
            let h = hf.sample(jx, jy) + jitter01(gx ^ gy, 0xC99) * 0.05;
            let t = (h / max_h).clamp(0.0, 1.0);
            verts.push(hs_forge::ImplVertex {
                pos: [
                    ((jx - cx) * inv) as i16,
                    ((jy - cy) * inv) as i16,
                    (h * inv) as i16,
                    0,
                ],
                rgba: [(t * 63.0) as u8, 0, 0, 130],
                node_id,
            });
            x += spacing;
            gx += 1;
        }
        y += spacing;
        gy += 1;
    }
    (verts, q)
}

#[inline]
fn jitter01(i: u32, salt: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9e3779b1) ^ salt.wrapping_mul(0x85ebca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

// ------------------------------------------------------------------ windowed
struct App {
    instance: wgpu::Instance,
    adapter_prefs: hs_render::AdapterPrefs,
    window: Option<Arc<winit::window::Window>>,
    surface: Option<wgpu::Surface<'static>>,
    renderer: Option<Renderer>,
    egui_state: Option<egui_winit::State>,
    ctx: egui::Context,
    ui: UiState,
    cam: Camera,
    region: Option<Region>,
    keys: [bool; 6],
    boost: bool,
    mouse_delta: (f32, f32),
    dragging: bool,
    last_cursor: Option<(f64, f64)>,
    palette_applied: String,
    last_frame: Instant,
    fps_ema: f32,
    screen_rect: egui::Rect,
    screen_px: [u32; 2],
    // M1 ingest
    cache: Arc<hs_cache::Cache>,
    client: Arc<hs_ingest::http::Client>,
    job_tx: Sender<JobMsg>,
    job_rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    last_search: String,
    last_search_at: Instant,
    ac_inflight: bool,
    pending_region:
        Option<(Region, String, String, hs_geo::HeightField, Vec<(String, Vec3, u8)>, Option<hs_forge::VectorData>)>,
    lidar: Option<hs_forge::Node>,
    last_src: Option<(
        hs_geo::HeightField,
        Option<hs_forge::VectorData>,
        f32,
        Vec<Beacon>,
        String,
        String,
    )>,
    last_density: usize,
    hf: Option<hs_geo::HeightField>,
    labels: Vec<(String, Vec3, u8)>,
    beacons: Vec<Beacon>,
    ctrl: bool,
    pending_pick: Option<(f64, f64)>,
    want_shot: bool,
    no_lidar: bool,
    fly_to: Option<FlyTo>,
    auto_orbit: bool,
    viewpoints: Vec<Viewpoint>,
    vp_idx: usize,
    dragging_right: bool,
    last_hc: bool,
    region_center_h: f32,
    near_center: Option<(f32, f32)>,
    region_max_h: f32,
    gov: Governor,
    gpu_sample_tick: u32,
    touches: std::collections::HashMap<u64, (f64, f64)>,
    pinch_dist: Option<f64>,
    last_tint: (
        [[f32; 3]; hs_forge::layer::COUNT],
        [bool; hs_forge::layer::COUNT],
    ),
    lod: Option<LodRegion>,
    traversal: Traversal,
    upload_off: u64,
}

impl App {
    fn new(region: Region, ui: UiState, cache: Arc<hs_cache::Cache>, client: Arc<hs_ingest::http::Client>) -> Self {
        let (job_tx, job_rx) = channel();
        Self {
            instance: Renderer::create_instance(),
            adapter_prefs: AdapterPrefs::default(),
            window: None,
            surface: None,
            renderer: None,
            egui_state: None,
            ctx: egui::Context::default(),
            ui,
            cam: demo_camera(),
            region: Some(region),
            keys: [false; 6],
            boost: false,
            mouse_delta: (0.0, 0.0),
            dragging: false,
            last_cursor: None,
            palette_applied: String::new(),
            last_frame: Instant::now(),
            fps_ema: 60.0,
            screen_rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            screen_px: [1, 1],
            cache,
            client,
            job_tx,
            job_rx,
            cancel: Arc::new(AtomicBool::new(false)),
            last_search: String::new(),
            last_search_at: Instant::now(),
            ac_inflight: false,
            pending_region: None,
            lod: None,
            traversal: Traversal::new(),
            upload_off: 0,
            hf: None,
            labels: Vec::new(),
            beacons: Vec::new(),
            ctrl: false,
            pending_pick: None,
            want_shot: false,
            no_lidar: false,
            fly_to: None,
            lidar: None,
            last_src: None,
            last_density: 2,
            auto_orbit: true,
            viewpoints: Vec::new(),
            vp_idx: 0,
            dragging_right: false,
            last_hc: false,
            region_center_h: 120.0,
            near_center: None,
            region_max_h: 300.0,
            gov: Governor::new(true),
            gpu_sample_tick: 0,
            touches: Default::default(),
            pinch_dist: None,
            last_tint: (
                [[0.0; 3]; hs_forge::layer::COUNT],
                [false; hs_forge::layer::COUNT],
            ),
        }
    }

    fn configure_surface(&mut self, w: u32, h: u32) {
        let (Some(surface), Some(renderer)) = (&self.surface, &self.renderer) else {
            return;
        };
        let caps = surface.get_capabilities(&renderer.adapter);
        let present_mode = caps
            .present_modes
            .iter()
            .find(|m| **m == wgpu::PresentMode::Mailbox)
            .copied()
            .unwrap_or(wgpu::PresentMode::Fifo);
        let cfg = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: renderer.format,
            width: w.max(1),
            height: h.max(1),
            present_mode,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&renderer.device, &cfg);
        self.screen_rect =
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(w as f32, h as f32));
        self.screen_px = [w, h];
        if let Some(r) = &mut self.renderer {
            r.resize(w, h);
        }
    }

    fn vp_path() -> PathBuf {
        data_dir().join("viewpoints.json")
    }

    fn save_viewpoint(&mut self) {
        let mut vps: Vec<Viewpoint> = std::fs::read_to_string(Self::vp_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        vps.push(Viewpoint {
            name: format!("vp{}", vps.len() + 1),
            pos: self.cam.pos.to_array(),
            yaw: self.cam.yaw,
            pitch: self.cam.pitch,
            fov: self.cam.fov_deg,
        });
        if let Ok(json) = serde_json::to_string_pretty(&vps) {
            std::fs::create_dir_all(data_dir()).ok();
            std::fs::write(Self::vp_path(), json).ok();
        }
        self.viewpoints = vps;
        self.ui.vp_count = self.viewpoints.len();
        eprintln!("[vp] saved {} viewpoints", self.ui.vp_count);
    }

    fn load_viewpoint(&mut self) {
        if self.viewpoints.is_empty() {
            self.viewpoints = std::fs::read_to_string(Self::vp_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            self.ui.vp_count = self.viewpoints.len();
        }
        if self.viewpoints.is_empty() {
            return;
        }
        let vp = &self.viewpoints[self.vp_idx % self.viewpoints.len()];
        self.vp_idx += 1;
        let to_pos = Vec3::from_array(vp.pos);
        self.fly_to = Some(FlyTo::new(&self.cam, to_pos, vp.yaw, vp.pitch, self.ui.fly_dur));
        self.cam.fov_deg = vp.fov;
    }

    /// Per-frame ingest bookkeeping (debounce, job poll, region swap).
    fn ingest_tick(&mut self, now: Instant) {
        while let Ok(msg) = self.job_rx.try_recv() {
            match msg {
                JobMsg::Ac(hits) => {
                    self.ui.search_results = hits;
                    self.ac_inflight = false;
                }
                JobMsg::Progress(stage, pct) => {
                    if stage.starts_with("vectors skipped") {
                        let chip = "vector layers degraded (source down/slow)".to_string();
                        if !self.ui.warn_chips.contains(&chip) {
                            self.ui.warn_chips.push(chip);
                        }
                    }
                    self.ui.job_stage = stage;
                    self.ui.job_pct = pct;
                }
                JobMsg::Region(r, name, attr, hf, labels, vd, lidar) => {
                    if let Some(n) = lidar {
                        self.lidar = Some(n); // reuse on re-forge; merge below
                    }
                    self.pending_region = Some((*r, name, attr, hf, labels, vd));
                    self.ui.job_active = false;
                }
                JobMsg::Failed(e) => {
                    self.ui.job_active = false;
                    self.ui.job_error = e.clone();
                    let chip = if e.to_lowercase().contains("dns")
                        || e.to_lowercase().contains("connect")
                        || e.to_lowercase().contains("network")
                        || e.to_lowercase().contains("tls")
                    {
                        "offline — cached data only".to_string()
                    } else if e.to_lowercase().contains("vectors") || e.to_lowercase().contains("overpass")
                    {
                        format!("vectors unavailable: {e}")
                    } else {
                        e.clone()
                    };
                    if !self.ui.warn_chips.contains(&chip) {
                        self.ui.warn_chips.push(chip);
                        if self.ui.warn_chips.len() > 3 {
                            self.ui.warn_chips.remove(0);
                        }
                    }
                }
            }
        }
        if self.ui.search != self.last_search {
            self.last_search = self.ui.search.clone();
            self.last_search_at = now;
            self.ac_inflight = false;
            self.ui.resolved = None;
            self.ui.search_results.clear();
        }
        if !self.ac_inflight
            && !self.ui.search.is_empty()
            && self.ui.resolved.is_none()
            && parse_raw_coords(&self.ui.search).is_none()
            && !self.ui.job_active
            && now.duration_since(self.last_search_at) > Duration::from_millis(250)
        {
            self.ac_inflight = true;
            spawn_autocomplete(
                self.cache.clone(),
                self.client.clone(),
                self.ui.search.clone(),
                self.job_tx.clone(),
            );
        }
        // live estimate line (plan §6.3 Standard density ≈ 0.88 M pts/km²)
        let ext = self.ui.extent_km;
        self.ui.est_points_m = 0.88 * ext * ext;
        let frac = (ext / 111.0).min(1.0).powi(2);
        self.ui.est_mb = 25.0 * frac * ((ext / 111.0).ceil().max(1.0).powi(2));

        if self.ui.load_requested {
            self.ui.load_requested = false;
            self.ui.search_results.clear(); // dropdown dismisses on commit
            if !self.ui.job_active {
                self.ui.job_active = true;
                self.ui.job_error.clear();
                self.ui.job_pct = 0.0;
                self.ui.job_stage = "queue".into();
                self.cancel.store(false, Ordering::Relaxed);
                let (cache, client) = (self.cache.clone(), self.client.clone());
                let tx = self.job_tx.clone();
                let cancel = self.cancel.clone();
                let query = self.ui.search.clone();
                let resolved = self.ui.resolved.clone();
                let extent = self.ui.extent_km;
                let vd = hs_ui::density_value(self.ui.density_idx);
                let no_lidar = self.no_lidar;
                std::thread::Builder::new()
                    .name("hs-load".into())
                    .spawn(move || {
                        let res = load_sync(
                            &cache,
                            &client,
                            &query,
                            resolved,
                            extent,
                            10_000_000,
                            true,
                            vd,
                            !no_lidar,
                            &cancel,
                            &|stage, pct| {
                                let _ = tx.send(JobMsg::Progress(stage.to_string(), pct));
                            },
                        );
                        match res {
                            Ok((r, name, attr, hf, labels, vd_data, lidar)) => {
                                let _ = tx.send(JobMsg::Region(
                                    Box::new(r),
                                    name,
                                    attr,
                                    hf,
                                    labels,
                                    vd_data,
                                    lidar,
                                ));
                            }
                            Err(e) => {
                                let _ = tx.send(JobMsg::Failed(e.to_string()));
                            }
                        }
                    })
                    .ok();
            }
        }
        if self.ui.cancel_requested {
            self.ui.cancel_requested = false;
            self.cancel.store(true, Ordering::Relaxed);
            self.ui.job_active = false;
            self.ui.job_stage = "cancelled".into();
        }
        if let Some((mut region, name, attr, hf, labels, _vd)) = self.pending_region.take() {
            if let Some(renderer) = &mut self.renderer {
                let has_lidar = region.nodes.iter().any(|n| n.layer == hs_forge::layer::IMPORTED);
                if !has_lidar {
                    if let Some(l) = &self.lidar {
                        attach_lidar(&mut region, l, &hf);
                        eprintln!("[lidar] merged into region at z={:.1}", hf.sample(0.0, 0.0));
                    }
                }
                if let Some(l) = &self.lidar {
                    self.ui.lidar_pts = l.points.len() as u64;
                }
                self.ui.points = region.point_count;
                self.ui.region = name.clone();
                self.ui.attribution = attr;
                self.ui.search_results.clear();
                self.hf = Some(hf);
                self.labels = labels;
                self.region_max_h = region.max_h;
                self.near_center = None;
                self.beacons = scaled_beacons(region.size_m);
                let top = region.max_h + region.size_m * 0.16;
                let quads = hs_forge::beacon_quads(
                    &self.beacons,
                    &|x, y| self.hf.as_ref().map(|h| h.sample(x, y)).unwrap_or(0.0),
                    top,
                    region.size_m,
                );
                renderer.upload_beacons(&quads);
                self.ui.beacon_count = self.beacons.len();
                let mut lod = build_lod(&region, COL_SIZE);
                enforce_budget(&mut lod, (VRAM_BUDGET_GB * 1e9) as u64);
                self.ui.pool_mb = lod.pool_bytes as f32 / 1e6;
                renderer.upload_lod(&lod);
                renderer.upload_furniture(&region);
                self.lod = Some(lod);
                self.upload_off = 0;
                self.traversal = Traversal::new(); // fresh cache per region
                let target = frame_camera(region.size_m, region.max_h);
                self.region_center_h = region.max_h * 0.25;
                self.fly_to = Some(FlyTo::new(
                    &self.cam,
                    target.pos,
                    target.yaw,
                    target.pitch,
                    self.ui.fly_dur,
                ));
                eprintln!("[region] loaded {name}: {} pts", region.point_count);
            }
        }
    }

    /// Ray-march the camera ray through pixel (px,py) against the heightfield.
    fn ray_pick(&self, px: f64, py: f64) -> Option<Vec3> {
        let hf = self.hf.as_ref()?;
        let aspect = self.screen_px[0] as f32 / self.screen_px[1].max(1) as f32;
        let vp = self.cam.view_proj(aspect);
        let inv = vp.inverse();
        let nx = px as f32 / self.screen_px[0] as f32 * 2.0 - 1.0;
        let ny = 1.0 - py as f32 / self.screen_px[1] as f32 * 2.0;
        let un = |z: f32| -> Vec3 {
            let p = inv * Vec3::new(nx, ny, z).extend(1.0);
            Vec3::new(p.x, p.y, p.z) / p.w
        };
        let p0 = un(-1.0);
        let dir = (un(1.0) - p0).normalize();
        let half = hf.min_x.abs();
        let mut t = 0.0f32;
        let max_t = half * 6.0;
        let step = max_t / 400.0;
        while t < max_t {
            let p = p0 + dir * t;
            if p.x.abs() <= half && p.y.abs() <= half && p.z < hf.sample(p.x, p.y) * self.ui.z_exag {
                return Some(p);
            }
            t += step;
        }
        None
    }

    fn frame(&mut self, window: &Arc<winit::window::Window>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if dt > 0.0 {
            self.fps_ema += (1.0 / dt - self.fps_ema) * 0.05;
        }
        self.ingest_tick(now);

        // a11y: zoom + high contrast (plan §11.3)
        self.ctx.set_zoom_factor(self.ui.ui_scale);
        if self.ui.high_contrast != self.last_hc {
            self.last_hc = self.ui.high_contrast;
            if self.last_hc {
                hs_ui::apply_theme_hc(&self.ctx);
            } else {
                hs_ui::apply_theme(&self.ctx);
            }
        }
        // density preset change → re-forge from cached sources (no network)
        if self.ui.density_idx != self.last_density {
            self.last_density = self.ui.density_idx;
            if let Some((hf, vd, size, beacons, name, attr)) = self.last_src.clone() {
                self.ui.job_active = true;
                self.ui.job_stage = "re-forge".into();
                self.ui.job_pct = 0.5;
                let tx = self.job_tx.clone();
                let vd_mul = hs_ui::density_value(self.ui.density_idx);
                let lidar = self.lidar.clone();
                std::thread::Builder::new()
                    .name("hs-reforge".into())
                    .spawn(move || {
                        let mut region = hs_forge::forge_region(
                            &hf,
                            &ForgeParams {
                                size_m: size,
                                target_points: 1_500_000,
                                vectors: vd.is_none(),
                                vector_density: vd_mul,
                                osm: vd,
                                beacons,
                                ..Default::default()
                            },
                        );
                        let has_lidar = region.nodes.iter().any(|n| n.layer == hs_forge::layer::IMPORTED);
                        if !has_lidar {
                            if let Some(l) = &lidar {
                                attach_lidar(&mut region, l, &hf);
                            }
                        }
                        let _ = tx.send(JobMsg::Region(
                            Box::new(region),
                            name,
                            attr,
                            hf,
                            Vec::new(),
                            None,
                            None, // lidar already merged into region above
                        ));
                    })
                    .ok();
            }
        }
        // frame history sparkline
        self.ui.frame_hist.push(dt * 1000.0);
        if self.ui.frame_hist.len() > 120 {
            self.ui.frame_hist.remove(0);
        }
        // beacon list + jump requests
        self.ui.beacon_list = self
            .beacons
            .iter()
            .enumerate()
            .map(|(i, b)| (format!("B{}", i + 1), b.x, b.y))
            .collect();
        if let Some(i) = self.ui.jump_beacon.take() {
            if let (Some(b), Some(hf)) = (self.beacons.get(i), &self.hf) {
                let g = hf.sample(b.x, b.y) * self.ui.z_exag;
                let to = Vec3::new(b.x - 260.0, b.y - 260.0, g + 140.0);
                let look = Vec3::new(b.x, b.y, g + 30.0);
                let dir = (look - to).normalize();
                self.fly_to = Some(FlyTo::new(
                    &self.cam,
                    to,
                    dir.y.atan2(dir.x),
                    dir.z.asin(),
                    self.ui.fly_dur,
                ));
            }
        }

        let raw = self.egui_state.as_mut().map(|s| s.take_egui_input(window));
        let full = if let Some(raw) = raw {
            let ui = &mut self.ui;
            ui.fps = self.fps_ema;
            ui.frame_ms = dt * 1000.0;
            Some(self.ctx.run(raw, |ctx| hs_ui::draw(ctx, ui)))
        } else {
            None
        };
        if let Some(full) = &full {
            if let Some(s) = self.egui_state.as_mut() {
                s.handle_platform_output(&window, full.platform_output.clone());
            }
        }
        self.cam.fov_deg = self.ui.fov;
        self.cam.sensitivity = self.ui.cam_sens;
        self.cam.damping = self.ui.cam_damp;
        self.cam.head_height = self.ui.head_height;
        self.cam.ground_clamp = self.ui.ground_clamp;
        let new_mode = match self.ui.cam_mode.as_str() {
            "orbit" => Mode::Orbit,
            "walk" => Mode::Walk,
            _ => Mode::Fly,
        };
        if new_mode != self.cam.mode && new_mode == Mode::Orbit {
            // pivot = ray through screen centre vs terrain (fallback: ahead)
            let pivot = self
                .ray_pick(self.screen_px[0] as f64 * 0.5, self.screen_px[1] as f64 * 0.5)
                .unwrap_or_else(|| self.cam.pos + self.cam.look_dir() * 1500.0);
            self.cam.orbit_target = pivot;
            self.cam.orbit_dist = (self.cam.pos - pivot).length().max(20.0);
        }
        self.cam.mode = new_mode;
        self.cam.sync_orbit();
        // dense near-field patch regen when the camera walks > 15 m (plan §1.2)
        if let Some(hf) = &self.hf {
            let (cx, cy) = (self.cam.pos.x, self.cam.pos.y);
            let need = match self.near_center {
                None => true,
                Some((px, py)) => (px - cx).powi(2) + (py - cy).powi(2) > 15.0 * 15.0,
            };
            if need && cx.abs() < hf.min_x.abs() && cy.abs() < hf.min_y.abs() {
                if let Some(renderer) = &mut self.renderer {
                    let nid = renderer.near_node_id();
                    let (verts, q) = build_nearfield(hf, cx, cy, self.region_max_h, nid);
                    renderer.upload_nearfield(&verts, [cx, cy, 0.0], q);
                }
                self.near_center = Some((cx, cy));
            }
        }

        // fly-to animation overrides manual control while active
        if let Some(mut ft) = self.fly_to {
            if let Some((pos, yaw, pitch)) = ft.step(dt) {
                self.cam.pos = pos;
                self.cam.yaw = yaw;
                self.cam.pitch = pitch;
                self.fly_to = Some(ft);
            } else {
                self.fly_to = None;
            }
        } else if self.auto_orbit && self.hf.is_some() {
            // empty-state: slow turntable until first input (plan §11.3)
            let a = dt * 0.06;
            let (sx, sy) = (self.cam.pos.x, self.cam.pos.y);
            self.cam.pos.x = sx * a.cos() - sy * a.sin();
            self.cam.pos.y = sx * a.sin() + sy * a.cos();
            let look = Vec3::new(0.0, 0.0, self.region_center_h);
            let dir = (look - self.cam.pos).normalize();
            self.cam.yaw = dir.y.atan2(dir.x);
            self.cam.pitch = dir.z.asin();
        } else if !self.ctx.wants_pointer_input() {
            let md = std::mem::take(&mut self.mouse_delta);
            match self.cam.mode {
                Mode::Walk => {
                    let ex = self.ui.z_exag;
                    let hf_ref = self.hf.as_ref();
                    let g = move |x: f32, y: f32| hf_ref.map(|h| h.sample(x, y) * ex).unwrap_or(0.0);
                    self.cam
                        .update_walk(dt, [self.keys[0], self.keys[1], self.keys[2], self.keys[3]], md, if self.boost { 4.0 } else { 1.0 }, &g);
                }
                Mode::Orbit => {
                    if md != (0.0, 0.0) {
                        self.cam.orbit_drag(md.0, md.1, self.ui.cam_sens);
                    }
                }
                _ => {
                    self.cam
                        .update_fly(dt, self.keys, if self.boost { 4.0 } else { 1.0 }, md, self.ui.cam_sens);
                    if self.cam.ground_clamp {
                        let ex = self.ui.z_exag;
                        let hf_ref = self.hf.as_ref();
                        let g = move |x: f32, y: f32| hf_ref.map(|h| h.sample(x, y) * ex).unwrap_or(0.0);
                        self.cam.clamp_ground(&g);
                    }
                }
            }
        } else {
            self.mouse_delta = (0.0, 0.0);
        }

        self.ui.labels_screen = project_labels(
            &self.labels,
            &self.cam,
            self.screen_px[0],
            self.screen_px[1],
            self.hf.as_ref(),
        );
        // ctrl+click ray-pick → beacon (plan §11.3)
        if let Some((px, py)) = self.pending_pick.take() {
            if let (Some(hf), Some(_lod)) = (&self.hf, &self.lod) {
                let half = hf.min_x.abs();
                if let Some(p) = self.ray_pick(px, py) {
                    self.beacons.push(Beacon { x: p.x, y: p.y });
                    if let Some(renderer) = &mut self.renderer {
                        let top = hf.sample(0.0, 0.0) + half * 2.0 * 0.16 + 400.0;
                        let quads = hs_forge::beacon_quads(
                            &self.beacons,
                            &|x, y| hf.sample(x, y),
                            top,
                            half * 2.0,
                        );
                        renderer.upload_beacons(&quads);
                    }
                    self.ui.beacon_count = self.beacons.len();
                }
            }
        }
        if self.ui.clear_beacons {
            self.ui.clear_beacons = false;
            self.beacons.clear();
            if let Some(renderer) = &mut self.renderer {
                renderer.upload_beacons(&[]);
            }
            self.ui.beacon_count = 0;
        }

        let (Some(surface), Some(renderer)) = (&self.surface, &mut self.renderer) else {
            return;
        };
        if self.palette_applied != self.ui.palette {
            renderer.set_palette(&self.ui.palette);
            self.palette_applied = self.ui.palette.clone();
            for l in 0..hs_forge::layer::COUNT {
                self.ui.layer_tint[l] = hs_forge::palette::layer_default_color(&self.ui.palette, l as u8);
                self.ui.layer_tint_on[l] = false;
            }
        }
        // per-layer colour overrides → LUT rebuild on change
        if self.last_tint.0 != self.ui.layer_tint || self.last_tint.1 != self.ui.layer_tint_on {
            self.last_tint = (self.ui.layer_tint, self.ui.layer_tint_on);
            renderer.set_layer_tints(&self.ui.layer_tint, &self.ui.layer_tint_on);
        }
        let mut settings = settings_from_ui(&self.ui);
        self.gov.enabled = self.ui.gov_on;
        if self.gov.enabled {
            settings.min_px = self.gov.min_px;
            settings.bloom_skip = self.gov.bloom_skip;
        }
        let budget = if self.gov.enabled {
            (self.ui.point_budget_m * 1e6 * self.gov.budget_mul) as u32
        } else {
            (self.ui.point_budget_m * 1e6) as u32
        };
        let thr = if self.gov.enabled { self.gov.thr } else { 1.4 };
        let tune = hs_render::LayerTune {
            size_mul: self.ui.layer_size,
            intensity: self.ui.layer_inten,
        };
        // progressive upload + LOD traversal
        let mut sels = Vec::new();
        if let Some(lod) = &self.lod {
            if self.upload_off < lod.pool_bytes {
                let end = (self.upload_off + UPLOAD_PER_FRAME).min(lod.pool_bytes);
                let v0 = (self.upload_off / 16) as usize;
                let v1 = (end / 16) as usize;
                renderer.write_pool(self.upload_off, bytemuck::cast_slice(&lod.instances[v0..v1]));
                self.upload_off = end;
                self.traversal.invalidate();
            }
            let aspect = self.screen_px[0] as f32 / self.screen_px[1].max(1) as f32;
            let focal = (self.screen_px[1] as f32 * 0.5) / (self.cam.fov_deg.to_radians() * 0.5).tan();
            sels = self.traversal.select(
                lod,
                &self.cam.view_proj(aspect),
                self.cam.pos,
                self.cam.yaw,
                self.cam.pitch,
                focal,
                dt,
                if settings.holo { thr } else { thr * 0.7 },
                budget,
                &|ni| renderer.node_uploaded(ni),
            );
            self.ui.uploaded_pct = renderer.pool_progress() * 100.0;
            self.ui.trav_ms = self.traversal.last_ms;
        }
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let time = now.elapsed().as_secs_f32();
        let stats = renderer.render(&view, &self.cam, &settings, time, &self.ui.layers, &tune, &sels);
        self.ui.points = stats.points_drawn;
        self.ui.nodes_drawn = stats.nodes_drawn;
        // governor: sample GPU time every 10 frames (readback stalls the pipe)
        self.gpu_sample_tick = (self.gpu_sample_tick + 1) % 10;
        if self.gpu_sample_tick == 0 {
            let g = renderer.read_gpu_ms().unwrap_or(dt * 1000.0);
            self.gov.sample(g);
            self.ui.gpu_ema_ms = self.gov.ema;
        }
        self.ui.gov_label = self.gov.label();

        if let Some(full) = full {
            for (id, img) in full.textures_delta.set {
                renderer
                    .egui_renderer
                    .update_texture(&renderer.device, &renderer.queue, id, &img);
            }
            let prims = self.ctx.tessellate(full.shapes, full.pixels_per_point);
            let sd = egui_wgpu::ScreenDescriptor {
                size_in_pixels: self.screen_px,
                pixels_per_point: full.pixels_per_point,
            };
            let mut enc = renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ui") });
            renderer
                .egui_renderer
                .update_buffers(&renderer.device, &renderer.queue, &mut enc, &prims, &sd);
            {
                let mut rp = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("ui-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                renderer.egui_renderer.render(&mut rp, &prims, &sd);
            }
            renderer.queue.submit(Some(enc.finish()));
            for id in full.textures_delta.free {
                renderer.egui_renderer.free_texture(&id);
            }
        }
        frame.present();

        // F9: supersampled screenshot (FR-14)
        if self.want_shot {
            self.want_shot = false;
            let ss = self.ui.supersample.max(1);
            let (w, h) = (self.screen_px[0] * ss, self.screen_px[1] * ss);
            let tex = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("screenshot"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: renderer.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            renderer.resize(w, h);
            let aspect = w as f32 / h as f32;
            let focal = (h as f32 * 0.5) / (self.cam.fov_deg.to_radians() * 0.5).tan();
            let shot_sels = self.traversal.select(
                self.lod.as_ref().unwrap(),
                &self.cam.view_proj(aspect),
                self.cam.pos,
                self.cam.yaw,
                self.cam.pitch,
                focal,
                0.0,
                1.4,
                POINT_BUDGET,
                &|ni| renderer.node_uploaded(ni),
            );
            renderer.render(&view, &self.cam, &settings, time, &self.ui.layers, &tune, &shot_sels);
            let px = renderer.download_rgba8(&tex, w, h);
            std::fs::create_dir_all("screenshots").ok();
            let path = format!(
                "screenshots/holoscope_{}x{}.png",
                chrono_free_ts(),
                ss
            );
            if image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).is_ok() {
                eprintln!("[shot] {path}");
            }
            renderer.resize(self.screen_px[0], self.screen_px[1]);
        }
    }
}

fn project_labels(
    labels: &[(String, Vec3, u8)],
    cam: &Camera,
    w: u32,
    h: u32,
    occ: Option<&hs_geo::HeightField>,
) -> Vec<(String, f32, f32, f32)> {
    let mut out = Vec::new();
    if labels.is_empty() || w == 0 || h == 0 {
        return out;
    }
    let aspect = w as f32 / h as f32;
    let vp = cam.view_proj(aspect);
    let (wf, hf) = (w as f32, h as f32);
    for (name, pos, rank) in labels {
        let dist = (cam.pos - *pos).length();
        let show = match rank {
            0 => true,
            1 => dist > 2500.0,
            2 => dist > 1200.0,
            3 => dist > 600.0,
            _ => dist > 300.0,
        };
        if !show {
            continue;
        }
        // terrain occlusion: march camera→label, skip if a ridge blocks it
        if let Some(hf) = occ {
            let mut blocked = false;
            let half = hf.min_x.abs();
            for i in 1..48 {
                let f = i as f32 / 48.0;
                let p = cam.pos.lerp(*pos, f);
                if p.x.abs() < half && p.y.abs() < half {
                    let g = hf.sample(p.x, p.y) * 1.6; // match default exag; labels are cosmetic
                    if p.z < g + 1.5 && f < 0.98 {
                        blocked = true;
                        break;
                    }
                }
            }
            if blocked {
                continue;
            }
        }
        let c = vp * pos.extend(1.0);
        if c.w <= 0.0 {
            continue;
        }
        let nx = c.x / c.w;
        let ny = c.y / c.w;
        if nx.abs() > 1.0 || ny.abs() > 1.0 {
            continue;
        }
        let px = (nx * 0.5 + 0.5) * wf;
        let py = (1.0 - (ny * 0.5 + 0.5)) * hf;
        let fade = (1.0 - dist / 12_000.0).clamp(0.65, 1.0);
        out.push((name.clone(), px, py, fade));
    }
    out
}

fn chrono_free_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{t}")
}

impl winit::application::ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = winit::window::WindowAttributes::default()
            .with_title("HoloScope — M1 geo ingest")
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 860u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let surface = self.instance.create_surface(window.clone()).expect("surface");
        let prefs = self.adapter_prefs.clone();
        let mut renderer = pollster::block_on(Renderer::new_with_prefs(
            &self.instance,
            Some(&surface),
            wgpu::TextureFormat::Rgba8UnormSrgb,
            &prefs,
        ))
        .expect("renderer");
        self.ui.adapter = renderer.adapter_name.clone();
        if let Some(mut region) = self.region.take() {
            let dhf = hs_geo::demo_heightfield(4000.0, 8.0);
            if let Some(l) = &self.lidar {
                attach_lidar(&mut region, l, &dhf);
                self.ui.lidar_pts = l.points.len() as u64;
                eprintln!("[lidar] merged into demo region at z={:.1}", dhf.sample(0.0, 0.0));
            }
            self.ui.points = region.point_count;
            self.last_src = Some((
                dhf.clone(),
                None,
                region.size_m,
                scaled_beacons(region.size_m),
                "demo_valley_4km".into(),
                String::new(),
            ));
            self.hf = Some(dhf);
            self.beacons = scaled_beacons(region.size_m);
            let top = region.max_h + region.size_m * 0.16;
            let quads = hs_forge::beacon_quads(
                &self.beacons,
                &|x, y| self.hf.as_ref().map(|h| h.sample(x, y)).unwrap_or(0.0),
                top,
                region.size_m,
            );
            renderer.upload_beacons(&quads);
            self.ui.beacon_count = self.beacons.len();
            renderer.set_palette(&self.ui.palette);
            self.palette_applied = self.ui.palette.clone();
            let mut lod = build_lod(&region, COL_SIZE);
            enforce_budget(&mut lod, (VRAM_BUDGET_GB * 1e9) as u64);
            self.ui.pool_mb = lod.pool_bytes as f32 / 1e6;
            renderer.upload_lod(&lod);
            renderer.upload_furniture(&region);
            self.lod = Some(lod);
            self.upload_off = 0;
        }
        let size = window.inner_size();
        renderer.resize(size.width, size.height);
        self.egui_state = Some(egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        ));
        hs_ui::apply_theme(&self.ctx);
        self.window = Some(window);
        self.surface = Some(surface);
        self.renderer = Some(renderer);
        self.configure_surface(size.width, size.height);
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(state) = &mut self.egui_state {
            let _ = state.on_window_event(&window, &event);
        }
        use winit::event::WindowEvent as E;
        match &event {
            E::CloseRequested => event_loop.exit(),
            E::KeyboardInput { event: ke, .. } => {
                use winit::keyboard::{Key, NamedKey};
                if let Key::Named(NamedKey::Escape) = ke.logical_key {
                    if ke.state == winit::event::ElementState::Pressed {
                        event_loop.exit();
                    }
                }
                if let Key::Named(NamedKey::F9) = ke.logical_key {
                    if ke.state == winit::event::ElementState::Pressed {
                        self.want_shot = true;
                    }
                }
                if let Key::Named(NamedKey::F1) = ke.logical_key {
                    if ke.state == winit::event::ElementState::Pressed {
                        self.ui.help_open = !self.ui.help_open;
                    }
                }
                if let Key::Named(NamedKey::F3) = ke.logical_key {
                    if ke.state == winit::event::ElementState::Pressed {
                        self.ui.perf_open = !self.ui.perf_open;
                    }
                }
                let pressed_now = ke.state == winit::event::ElementState::Pressed;
                if pressed_now && self.ctrl {
                    use winit::keyboard::KeyCode as K;
                    if let winit::keyboard::PhysicalKey::Code(c) = ke.physical_key {
                        match c {
                            K::KeyS => self.save_viewpoint(),
                            K::KeyO => self.load_viewpoint(),
                            _ => {}
                        }
                    }
                }
                if pressed_now {
                    self.auto_orbit = false;
                    self.fly_to = None;
                }
                let pressed = ke.state == winit::event::ElementState::Pressed;
                if let winit::keyboard::PhysicalKey::Code(c) = ke.physical_key {
                    use winit::keyboard::KeyCode as K;
                    let idx = match c {
                        K::KeyW => 0,
                        K::KeyS => 1,
                        K::KeyA => 2,
                        K::KeyD => 3,
                        K::KeyE => 4,
                        K::KeyQ => 5,
                        K::ShiftLeft | K::ShiftRight => {
                            self.boost = pressed;
                            99
                        }
                        _ => 99,
                    };
                    if idx < 6 {
                        self.keys[idx] = pressed;
                    }
                }
            }
            E::CursorMoved { position, .. } => {
                if let Some((lx, ly)) = self.last_cursor {
                    let dx = (position.x - lx) as f32;
                    let dy = (position.y - ly) as f32;
                    if self.dragging {
                        self.mouse_delta.0 += dx;
                        self.mouse_delta.1 += dy;
                    }
                    if self.dragging_right && self.cam.mode == Mode::Orbit {
                        let right = self.cam.look_dir().cross(Vec3::Z).normalize_or_zero();
                        let up = Vec3::Z;
                        let k = self.cam.orbit_dist * 0.0015;
                        self.cam.orbit_target -= right * (dx * k);
                        self.cam.orbit_target += up * (dy * k);
                        self.cam.sync_orbit();
                    }
                }
                self.last_cursor = Some((position.x, position.y));
            }
            E::MouseInput { state, button, .. } => {
                let pressed = *state == winit::event::ElementState::Pressed;
                self.dragging = pressed && *button == winit::event::MouseButton::Left;
                self.dragging_right = pressed && *button == winit::event::MouseButton::Right;
                if pressed {
                    self.auto_orbit = false;
                    self.fly_to = None;
                }
                if pressed
                    && self.ctrl
                    && *button == winit::event::MouseButton::Left
                    && !self.ctx.wants_pointer_input()
                {
                    self.pending_pick = self.last_cursor;
                }
            }
            E::Touch(touch) => {
                use winit::event::TouchPhase;
                self.auto_orbit = false;
                self.fly_to = None;
                match touch.phase {
                    TouchPhase::Started => {
                        self.touches.insert(touch.id, (touch.location.x, touch.location.y));
                        self.pinch_dist = None;
                    }
                    TouchPhase::Moved => {
                        let n_touch = self.touches.len();
                        if let Some(prev) = self.touches.get_mut(&touch.id) {
                            let dx = (touch.location.x - prev.0) as f32;
                            let dy = (touch.location.y - prev.1) as f32;
                            if n_touch == 1 {
                                // one finger = look / rotate (iPad-friendly)
                                self.mouse_delta.0 += dx;
                                self.mouse_delta.1 += dy;
                            }
                            *prev = (touch.location.x, touch.location.y);
                        }
                        if n_touch == 2 {
                            let mut it = self.touches.values();
                            let a = *it.next().unwrap();
                            let b = *it.next().unwrap();
                            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                            if let Some(pd) = self.pinch_dist {
                                if d > 1.0 {
                                    self.cam.dolly((pd as f32 / d as f32).clamp(0.9, 1.1));
                                }
                            }
                            self.pinch_dist = Some(d);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.touches.remove(&touch.id);
                        self.pinch_dist = None;
                    }
                }
            }
            E::ModifiersChanged(mods) => {
                self.ctrl = mods.state().control_key();
            }
            E::MouseWheel { delta, .. } => {
                let amount = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.05,
                };
                self.cam.dolly((-amount * 0.08).exp());
            }
            E::Resized(size) => {
                self.configure_surface(size.width, size.height);
            }
            E::RedrawRequested => {
                self.frame(&window);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}


// ------------------------------------------------------------------ bench
#[derive(serde::Deserialize)]
struct BenchFile {
    segments: Vec<BenchSeg>,
}
#[derive(serde::Deserialize)]
struct BenchSeg {
    name: String,
    from: BenchPose,
    to: BenchPose,
    duration_s: f32,
    #[serde(default = "default_easing")]
    easing: String,
    #[serde(default)]
    assert: BenchAssert,
}
fn default_easing() -> String {
    "smoothstep".into()
}
#[derive(serde::Deserialize)]
struct BenchPose {
    enu: [f32; 3],
    look: [f32; 3],
}
#[derive(Default, serde::Deserialize)]
struct BenchAssert {
    #[serde(default)]
    p99_frame_ms: f32,
}

fn pct(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

fn run_bench(a: &Args, path: &str) -> Result<()> {
    let spec: BenchFile = toml::from_str(&std::fs::read_to_string(path)?)?;
    let cache = Arc::new(hs_cache::Cache::open(&data_dir())?);
    let client = Arc::new(hs_ingest::http::Client::default());
    let (region, _name, _attr) = match &a.place {
        Some(place) => {
            let (r, n, at, _h, _l, _v, _ld) = load_sync(
                &cache,
                &client,
                place,
                None,
                a.extent_km,
                a.points,
                a.vectors,
                1.0,
                false,
                &AtomicBool::new(false),
                &|st, p| eprintln!("[load] {st} {:.0}%", p * 100.0),
            )?;
            (r, n, at)
        }
        None => {
            let (_hf, r, _t) = forge_demo(a.points);
            (r, "demo".into(), String::new())
        }
    };
    let instance = Renderer::create_instance();
    let mut renderer = pollster::block_on(Renderer::new_with_prefs(
        &instance,
        None,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &adapter_prefs(a),
    ))?;
    eprintln!("[bench] adapter: {}", renderer.adapter_name);
    renderer.set_palette(&a.palette);
    let mut lod = build_lod(&region, COL_SIZE);
    enforce_budget(&mut lod, (VRAM_BUDGET_GB * 1e9) as u64);
    renderer.upload_lod(&lod);
    renderer.upload_furniture(&region);
    renderer.resize(a.width, a.height);
    let off = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench-target"),
        size: wgpu::Extent3d { width: a.width, height: a.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let off_view = off.create_view(&wgpu::TextureViewDescriptor::default());
    // drain the pool so the bench measures traversal+render, not uploads
    let mut up = 0u64;
    while up < lod.pool_bytes {
        let end = (up + UPLOAD_PER_FRAME).min(lod.pool_bytes);
        renderer.write_pool(up, bytemuck::cast_slice(&lod.instances[(up / 16) as usize..(end / 16) as usize]));
        up = end;
    }
    let settings = Settings { holo: true, ..Default::default() };
    let tune = hs_render::LayerTune::default();
    let vis = [true; hs_forge::layer::COUNT];
    let mut traversal = Traversal::new();
    let mut csv = String::from("segment,frame,cpu_ms,gpu_ms,points\n");
    for seg in &spec.segments {
        let frames = (seg.duration_s * 60.0).max(1.0) as u32;
        let mut cpu: Vec<f32> = Vec::with_capacity(frames as usize);
        let mut gpu: Vec<f32> = Vec::with_capacity(frames as usize);
        for f in 0..frames {
            let t0 = Instant::now();
            let raw = f as f32 / (frames - 1).max(1) as f32;
            let e = match seg.easing.as_str() {
                "linear" => raw,
                _ => raw * raw * (3.0 - 2.0 * raw),
            };
            let pos = Vec3::from(seg.from.enu).lerp(Vec3::from(seg.to.enu), e);
            let look = Vec3::from(seg.from.look).lerp(Vec3::from(seg.to.look), e);
            let mut cam = Camera::default();
            let dir = (look - pos).normalize();
            cam.pos = pos;
            cam.yaw = dir.y.atan2(dir.x);
            cam.pitch = dir.z.asin();
            cam.fov_deg = 40.0;
            let sels = traversal.select(
                &lod,
                &cam.view_proj(a.width as f32 / a.height as f32),
                cam.pos,
                cam.yaw,
                cam.pitch,
                (a.height as f32 * 0.5) / (cam.fov_deg.to_radians() * 0.5).tan(),
                1.0 / 60.0,
                1.4,
                POINT_BUDGET,
                &|ni| renderer.node_uploaded(ni),
            );
            let stats = renderer.render(
                &off_view,
                &cam,
                &settings,
                f as f32 / 60.0,
                &vis,
                &tune,
                &sels,
            );
            let g = renderer.read_gpu_ms().unwrap_or(0.0);
            let c = t0.elapsed().as_secs_f32() * 1000.0;
            cpu.push(c);
            gpu.push(g);
            csv.push_str(&format!("{},{},{:.2},{:.2},{}\n", seg.name, f, c, g, stats.points_drawn));
        }
        let mut cs = cpu.clone();
        cs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let p99 = pct(&cs, 0.99);
        let verdict = if seg.assert.p99_frame_ms > 0.0 {
            if p99 <= seg.assert.p99_frame_ms { "PASS" } else { "FAIL(vs target; software GL?)" }
        } else {
            "-"
        };
        eprintln!(
            "[bench] {:14} frames={} cpu p50={:.1} p99={:.1} ms | gpu p50={:.1} p99={:.1} ms | {verdict}",
            seg.name,
            frames,
            pct(&cs, 0.5),
            p99,
            {
                let mut g = gpu.clone();
                g.sort_by(|x, y| x.partial_cmp(y).unwrap());
                pct(&g, 0.5)
            },
            {
                let mut g = gpu.clone();
                g.sort_by(|x, y| x.partial_cmp(y).unwrap());
                pct(&g, 0.99)
            }
        );
    }
    std::fs::create_dir_all("bench").ok();
    std::fs::write("bench/out.csv", &csv)?;
    println!("bench/out.csv");
    Ok(())
}

fn main() -> Result<()> {
    let args = parse_args();
    if let Some(b) = &args.bench {
        return run_bench(&args, b);
    }
    if args.headless {
        return run_headless(&args);
    }
    // demo empty-state stays light for NFR-06 cold start; LOADs use args.points
    let (_hf, region, forge_s) = forge_demo(args.points.min(3_000_000));
    eprintln!("[forge] demo region {} points in {:.2}s", region.point_count, forge_s);
    let mut ui = UiState::default();
    ui.palette = args.palette.clone();
    for l in 0..hs_forge::layer::COUNT {
        ui.layer_tint[l] = hs_forge::palette::layer_default_color(&ui.palette, l as u8);
    }
    let cache = Arc::new(hs_cache::Cache::open(&data_dir())?);
    let client = Arc::new(hs_ingest::http::Client::default());
    eprintln!("[cache] {}", data_dir().display());
    let viewpoints: Vec<Viewpoint> = std::fs::read_to_string(data_dir().join("viewpoints.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = App::new(region, ui, cache.clone(), client.clone());
    app.adapter_prefs = adapter_prefs(&args);
    app.no_lidar = args.no_lidar;
    app.lidar = load_lidar_node(&cache, &client, &args, 4000.0);
    app.viewpoints = viewpoints;
    app.ui.vp_count = app.viewpoints.len();
    event_loop.run_app(&mut app)?;
    Ok(())
}

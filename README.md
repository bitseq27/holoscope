# HoloScope — M0

**Rust + wgpu + egui** desktop app that renders any place on Earth as a live,
navigable, LiDAR-style holographic point cloud (see `docs/holoscope_plan.md`).
This repo currently contains the **M0 first working version**: window + egui HUD,
additive point-sprite renderer with bloom/ACES post, procedural demo region in the
reference holographic look, fly/orbit camera, and a headless offscreen render mode
used for verification on GPU-less machines.

| | |
|---|---|
| Status | **M6+ complete**: governor, quarantine, antimeridian/polar, soak + density presets (Low–Ultra) and continuous road ribbons |
| Progress history | **`BUILD_LOG.md` — read this first** |
| Plan | `docs/holoscope_plan.md` |
| Reference look | `docs/reference/map1_3d.jpg` (3D holo), `ui.jpg` (HUD), `map_2d.jpg` (palette) |
| Hero renders | `renders/m5_kyoto_street.png` (street-level 1.7 m), `renders/m4_kyoto_clarity.png` (clarity+EDL+labels), `renders/m3_kyoto_lod.png` (LOD), `renders/holoscope_m0.png` (demo) |

## Quick start (Linux sandbox / dev machine)

```bash
scripts/bootstrap.sh     # idempotent: apt deps + lavapipe + rustup (toolchain in /opt/hs-tools)
scripts/build.sh         # cargo build -j1 (sandbox has 1 GiB RAM; see BUILD_LOG ENV-3)
scripts/verify.sh        # cargo test + headless golden render -> renders/verify.png
```

Run the app (Windows 10/11 x64 — the target platform):

```bat
launch.bat              :: build (debug) + run
launch.bat release      :: optimized build + run
launch.bat release --headless --out shot.png   :: extra args pass through
launch.bat --software   :: force software rasterizer (remote / GPU-less box;
                           also HOLOSCOPE_SOFTWARE=1, or --adapter <substring>
                           e.g. --adapter llvmpipe / "Basic Render" / WARP)
:: fly: WASD + QE, drag = look, wheel = speed, Shift = boost, Esc = quit
```

Run the app (Linux/mac dev machine with a display):

```bash
cargo run -p hs-app --release
```

Headless (no display / CI):

```bash
cargo run -p hs-app -- --headless --out renders/shot.png --width 1280 --height 960 \
    --points 1500000 --palette hologreen
# flags: --no-ui --frames N --hide terrain,roads,... (layer isolation diagnostics)
# bench flythrough (plan App. D): CPU+GPU frame stats -> bench/out.csv
scripts/bench.sh                      # default bench/fly_kyoto.toml
# look-dev flags: --clarity --bg 0..3 --sweep ; in-app: F9 supersample shot,
# ctrl+click place beacon, LAYER TUNE sliders, CLARITY/HOLO toggle
# street-level hero: --eye -500,-1100 ; overlays: F1 help, F3 perf graph;
# viewpoints: Ctrl+S / Ctrl+O ; camera modes fly/orbit/walk in CAMERA panel
# density presets: DENSITY combo (low/med/high/ultra) or --density; re-forges
# from cache live; roads/water have continuous ribbon cores under the points
# soak harness: SOAK_MIN=0.5 scripts/soak.sh -> bench/soak.csv (RSS/GPU samples)
# M1: load a real place (Copernicus DEM, ranged COG GETs, cached on disk):
cargo run -p hs-app -- --headless --place "Interlaken, Switzerland" --extent 14 \
    --out renders/shot.png --width 1280 --height 960
```

## LiDAR import (measured 3D)

```bash
# local file (LAS 1.2–1.4, formats 0–3/6–8):
cargo run -p hs-app -- --headless --las scan.las --eye 90,-140 --out shot.png
# or stream+cache a URL (OpenTopography apibeam, AHN/EA/3DEP tile portals):
cargo run -p hs-app -- --lidar-url "https://…/apibeam?…&outputFormat=las" --las-center 52.37,4.89
# options: --las-center lat,lon (geographic files) · --lidar-max 4000000
```
Classification colours the scan (ground→elevation ramp, buildings/veg/water→
their layers, rest→amber "imported lidar" layer with its own toggle+tint).
Use **CLARITY** mode to read measured structure; HOLO keeps the glow look.
**Auto-LiDAR:** every LOAD also tries the free OpenTopography catalog
(keyless): catalog → tile index → LAZ tiles for the central ≤3 km, ≤4 tiles
(regional coverage: US/UK/NL/NZ/…; `--no-lidar` opts out; state-plane CRS
datasets skip with a status chip).
Make a test scan: `cargo run -p hs-ingest --example make_demo_las -- path.las`.

## 3D building data (FAQ)

Shells today come from OSM footprints + `height`/`levels`/class priors.
Measured-3D sources evaluated (BUILD_LOG session 8): OpenTopography LiDAR
(USGS 3DEP / EA LiDAR / AHN — real roofs & trees), Microsoft Global ML
Building Heights, and (rejected: proprietary) Cesium/Google 3D Tiles; user
LAS/LAZ import is plan FR-12. The LiDAR provider + LAS reader are the next
ingest item (`hs-ingest::lidar`).

## Crates

| Crate | Role |
|---|---|
| `hs-geo` | WGS84⇄ECEFENU (f64), fBm/ridged noise, heightfields, demo region |
| `hs-cache` | SQLite index + SHA-256 blob store (geocode cache, COG tile cache) |
| `hs-ingest` | COG DEM ranged reader (predictor-3 aware), Photon/Nominatim, Overpass vectors, polite HTTPS, region planner |
| `hs-lod` | 256 m columns × nested density chunks, SSE traversal w/ hysteresis + cross-fade, frustum cull, draw budget, VRAM enforce |
| `hs-forge` | 12 B point record, palette LUTs, point/line/quad synthesis (terrain, contours, roads, water, buildings, veg, holo furniture) |
| `hs-camera` | fly/orbit controllers, reversed-Z infinite-far projection |
| `hs-render` | wgpu pipelines + WGSL (points/lines/quads, bloom, composite), egui-wgpu, readback |
| `hs-ui` | egui HUD in the holo-ops style of `ui.jpg` |
| `hs-app` | binary: windowed event loop + headless offscreen mode |

## Milestone map (from the plan)

M0 foundation/render spike ✅ → M1 geo ingest + real terrain ✅ → M2 vector layers ✅ → M3 LOD/streaming ✅ → M4 look-dev ✅ → M5 camera/UX ✅ → M6 perf/hardening ✅ →
M3 LOD/streaming → M4 look-dev sign-off → M5 camera/UX → M6 perf/hardening →
M7 packaging. Next-session checklist lives at the bottom of `BUILD_LOG.md`.

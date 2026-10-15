# HOLOSCOPE — Engineering Plan
### A Rust/Windows desktop app that renders any place on Earth as a live, navigable, LiDAR-style holographic point cloud

**Version:** 1.0 (2026-10-12) · **Status:** Draft for review · **Target platform:** Windows 10/11 x64, Vulkan-capable GPU (reference: NVIDIA Titan RTX, 24 GB)

---

## Table of contents

0. [Executive summary](#0-executive-summary)
1. [Product vision & reference look](#1-product-vision--reference-look)
2. [Requirements](#2-requirements)
3. [Technology selection](#3-technology-selection)
4. [Architecture overview](#4-architecture-overview)
5. [Geodata strategy (open sources)](#5-geodata-strategy-open-sources)
6. [Point-cloud synthesis](#6-point-cloud-synthesis)
7. [LOD, streaming & memory management](#7-lod-streaming--memory-management)
8. [Rendering system](#8-rendering-system)
9. [Camera & navigation](#9-camera--navigation)
10. [Layers, display & camera settings](#10-layers-display--camera-settings)
11. [UI / UX](#11-ui--ux)
12. [Performance engineering plan](#12-performance-engineering-plan)
13. [Error handling, offline mode, persistence](#13-error-handling-offline-mode-persistence)
14. [Testing & QA](#14-testing--qa)
15. [Packaging & distribution](#15-packaging--distribution)
16. [Roadmap & milestones](#16-roadmap--milestones)
17. [Risks & mitigations](#17-risks--mitigations)
18. [Appendices](#18-appendices)

---

## 0. Executive summary

HoloScope is a native Rust application for Windows that lets a user type any place name or coordinate ("Kyoto", "48.8584, 2.2945"), choose a map extent (e.g. 8 × 8 km), and within seconds fly through a dense, glowing point-cloud reconstruction of that place — terrain, roads, rail, water, buildings, vegetation — in the holographic LiDAR-scan aesthetic of the reference image (`map1.jpg`).

**Core design ideas**

1. **We synthesize the point cloud, we don't ship one.** Open raster (Copernicus DEM, ESA WorldCover) and vector (OpenStreetMap / Overture Maps) data are fetched on demand for the requested extent and converted into points by our own generator. This gives us full control over density, layers, colors and — critically — **level-of-detail mips**, which are trivial to produce for synthesized data and nearly impossible for raw scans.
2. **One render path, many providers.** Everything (terrain, roads, buildings, imported LAS/LAZ LiDAR) is reduced to a common `LodNode` unit: an AABB + a GPU-ready point buffer. The renderer only knows how to draw millions of `LodNode`s per frame with screen-space-error selection and a point budget.
3. **60 fps is a system property, not a hope.** A closed-loop adaptive quality controller (GPU-timestamp driven) tunes the drawn-point budget and sprite size every frame against a 16.6 ms target; a VRAM residency manager with LRU eviction caps memory; all heavy work (network, decode, synthesis, LOD build) happens on worker threads with a bounded per-frame GPU upload budget.
4. **Two visual modes:** *Holo* (additive glow + bloom, the reference look) and *Clarity* (depth-tested opaque points + eye-dome lighting, for readability), share one pipeline and differ in blend/post settings.

**Estimated effort:** ~15–18 developer-weeks across 8 milestones (single senior dev; parallelizable to ~9–11 weeks with two).

---

## 1. Product vision & reference look

### 1.1 Reference image decomposition (`map1.jpg`)

Every salient element of the reference becomes a concrete, toggleable feature:

| # | Element in reference | HoloScope feature | Layer group |
|---|----------------------|-------------------|-------------|
| 1 | Black void background, no sky | Background mode: `void / grid-floor / starfield / gradient` | Style |
| 2 | Terrain slab of dense glowing points (green→cyan by elevation/slope) | Terrain point layer, elevation/slope/layer palettes | Data |
| 3 | Vertical "skirt" of points falling from slab edges to a base plane | **Plinth/skirt** generator: boundary-ring points extruded downward with density falloff | Holo furniture |
| 4 | Yellow-green wireframe outline at slab base | Base outline (line list) at plinth bottom | Holo furniture |
| 5 | Cyan wireframe bounding cage around the volume | **Bounding cage** (12-edge line box, corner ticks) | Holo furniture |
| 6 | Dotted grid wall on left/back planes | **Grid walls** (sparse point lattices on 2 orthogonal planes, auto-scaled ticks) | Holo furniture |
| 7 | Bright cyan road/network lines glowing on terrain | Roads/rail/water vector layers rendered as point ribbons with emissive boost | Data |
| 8 | Yellow diamond markers with vertical light beams | **Beacons**: user-placed pins (click-to-place, ray-picked), beam + diamond billboard, label optional | Holo furniture |
| 9 | Overall neon additive-glow look | Additive blending + bloom chain + tone map | Style |
| 10 | Slight vertical exaggeration of relief | Elevation exaggeration slider (1×–5×) | Style |

### 1.2 Experience goals

- **Search → flying in ≤ 5 s** for a typical 10 × 10 km city on a 100 Mbit connection (coarse view first, detail streams in).
- **Continuous 60 fps** (99th-percentile frame ≤ 16.6 ms at 2560×1440) while flying from 5 km altitude down to 1.7 m head height.
- **Dense up close:** at head height, point spacing on screen ≤ 1 px for the active LOD tier (perceptually solid LiDAR surface).
- **Legible:** layer isolation, per-layer color/size/intensity, clarity mode with eye-dome lighting, distance fade, labels.

---

## 2. Requirements

### 2.1 Functional

| ID | Requirement |
|----|-------------|
| FR-01 | User can search a place by free text (autocomplete) or enter raw `lat, lon` (decimal degrees, DMS accepted). |
| FR-02 | User can specify map dimensions: width × height in km (0.5–50 km per side), or radius for circular extents; estimator shows projected point count + download size before commit. |
| FR-03 | App fetches open-license elevation + vector data for the extent and synthesizes a 3D point cloud. |
| FR-04 | Point cloud is rendered in LiDAR-scan style: small glowing sprites, additive blending, optional bloom; alternate clarity mode available. |
| FR-05 | Toggleable layers: terrain (topology), contours, roads (by class), rail, water (rivers/lakes/coast), buildings, vegetation/land cover, labels/POI, imported LiDAR, holo furniture (cage, skirt, grid walls, base outline, beacons). |
| FR-06 | Per-layer controls: visibility, color mode, point-size multiplier, intensity, vertical lift, density multiplier. |
| FR-07 | Free camera: 6-DoF fly, orbit, terrain-follow walk at configurable head height (default 1.7 m), animated fly-to transitions; seamless from ≥ 5 km AGL to head height with no precision artifacts. |
| FR-08 | Camera settings: FOV, sensitivity, speed range, damping, head height, ground clamp, near-plane mode, fly-to duration. |
| FR-09 | Display settings: point size mode/curve, blend mode, bloom, exposure, palette, elevation exaggeration, distance fade/fog, background, grid, scan-sweep animation, jitter, label scale, clarity toggles (EDL, high-contrast). |
| FR-10 | LOD: screen-space-error driven tier/node selection with cross-fade; no visible popping; density increases continuously as camera approaches. |
| FR-11 | Adaptive performance controller maintains target FPS (default 60, cap options 30/60/120/unlocked) by tuning point budget & sprite size; manual override available. |
| FR-12 | Import: drag-drop LAS/LAZ/EPT point clouds and GeoTIFF DEMs merge as extra layers (optional stretch, M6+). |
| FR-13 | Cache: all downloaded source data and synthesized nodes cached on disk; re-visiting a place is instant and offline-capable. |
| FR-14 | Screenshots (PNG, up to 4× supersample); save/load viewpoints and session settings. |
| FR-15 | Attribution overlay (OSM, Copernicus, ESA WorldCover, Overture) always visible when data layers are on; license page in About. |

### 2.2 Non-functional

| ID | Requirement |
|----|-------------|
| NFR-01 | 60 fps @ 2560×1440 on Titan RTX (24 GB) with ≥ 15 M points drawn/frame in Holo mode; ≥ 8 M @ 3840×2160. |
| NFR-02 | VRAM point-cache budget configurable (default 14 GB), hard-capped, LRU-evicted; app never exceeds cap. |
| NFR-03 | RAM during synthesis ≤ 1.5× the streamed region's compressed size + 2 GB working set (target ≤ 8 GB for a 10 km city at Standard). |
| NFR-04 | First pixels of the requested place on screen ≤ 4 s (median, 100 Mbit line, cached geocode); full Standard density ≤ 20 s streamed. |
| NFR-05 | No frame-time spike > 25 ms caused by loading (uploads chunked to ≤ 48 MB/frame). |
| NFR-06 | Cold start to interactive window ≤ 2 s. |
| NFR-07 | Respectful crawling: ≤ 1 req/s per host, ETag/conditional GETs, identifiable User-Agent, local cache-first (Nominatim policy compliant). |
| NFR-08 | Single-binary installer + portable ZIP; no admin rights needed for portable mode; runs on Windows 10 21H2+. |
| NFR-09 | Crash-free sessions ≥ 99.5%; full-diagnostic log ring buffer; graceful degradation when offline or when a data source is down. |
| NFR-10 | All source data open-license (ODbL / CC-BY / open government); attribution rendered in-app. |

---

## 3. Technology selection

| Concern | Choice | Rationale / alternatives considered |
|---|---|---|
| Language | **Rust (stable, MSVC toolchain)** | Memory safety for a streaming engine; fearless concurrency for the load pipeline; first-class wgpu/egui ecosystem. |
| GPU API | **wgpu (Vulkan backend)** | Safe, maintained, Windows/Vulkan-native performance; compute shaders available for future GPU culling/compaction. *Alternative:* raw `ash` (more control, 2–3× wrapper code, no portability win here). *Rejected:* DirectX12 via `windows-rs` (no ecosystem for our needs), OpenGL (legacy, driver variance). |
| Windowing/input | **winit** | Standard wgpu partner; raw mouse mode for fly-cam; DPI-aware. |
| UI | **egui + egui-wgpu + egui-winit** | Immediate mode, zero layout pain, renders into our wgpu surface, great sliders/panels/plot widgets for settings & FPS graph. *Alternative:* `iced` (heavier styling, weaker tooling for dense control panels). |
| Math | **glam** (f32/f64), custom geo module | SIMD-friendly; f64 paths where needed. |
| Async/IO | **tokio** (network streams) + **rayon** (CPU synthesis) | tokio for concurrent ranged HTTPS; rayon for embarrassingly-parallel point generation & zstd decode. |
| HTTP | **reqwest** (rustls, stream API) | Ranged GETs against S3/COG endpoints. |
| GeoTIFF/DEM | **`tiff` crate + manual COG ranged reader**; native `.hgt` parser for SRTM fallback | Avoids GDAL (C dep, licensing/packaging pain). COG = TIFF tiles + HTTP ranges; ~600 lines to a solid reader. |
| Vector data | **`osmpbf`** crate for PBF; **`arrow`/`parquet`** for Overture GeoParquet; custom Overpass/Photon/Nominatim JSON clients | Covers both primary (OSM) and fallback (Overture) sources. |
| Compression | **zstd** (`zstd` crate, streaming) | Disk cache + node payloads; ~3–5× on quantized point streams. |
| Serialization | **serde / serde_json / toml** | Settings, manifests, geocoder responses. |
| Local index | **rusqlite** (bundled) | Cache index, geocode cache, region manifest DB. |
| Fonts/labels | **fontdue** (SDF atlas bake) | Billboard labels in-scene; egui handles UI text. |
| Logging | **tracing + tracing-subscriber** (ring file) | Perf spans double as profiler input. |
| Errors | **anyhow** (app) / **thiserror** (libs) | Standard split. |
| Installer | **cargo-wix (WiX MSI)** + portable ZIP | Enterprise-friendly, no admin for portable. |

---

## 4. Architecture overview

### 4.1 Block diagram

```
                        ┌────────────────────────────────────────────────────────────┐
                        │                        MAIN THREAD                         │
  keyboard/mouse ──────▶│  winit event loop                                          │
                        │   ├─ egui UI (search, layers, style, camera, perf panels)  │
                        │   ├─ CameraRig (f64 geodetic ⇄ f64 ENU, f32 view export)   │
                        │   ├─ LodSelector  (frustum + SSE traverse → visible nodes) │
                        │   ├─ PointBudgetController (adaptive 60 fps loop)          │
                        │   ├─ RenderGraph encode: points→EDL→bloom→composite→UI    │
                        │   └─ Upload scheduler (≤48 MB/frame from inbound queue)    │
                        └──────────────▲─────────────────────────────▲───────────────┘
                                       │ NodePayloads (bounded MPSC) │ GPU-timestamp stats
                        ┌──────────────┴───────────────┐   ┌─────────┴────────────────┐
                        │        WORKER POOL (rayon)   │   │   TOKIO IO RUNTIME       │
                        │  decompress zstd             │   │  geocoder (Photon/Nomin.)│
                        │  synthesize points per layer │◀──│  DEM COG ranged GETs     │
                        │  build LOD tiers (mips)      │   │  OSM PBF / Overture PQ   │
                        │  quantize → GpuPointRecord   │   │  conditional GET/ETag    │
                        └──────────────▲───────────────┘   └─────────▲────────────────┘
                                       │ region jobs                │ HTTPS (ranged, pooled)
                        ┌──────────────┴────────────────────────────┴────────────────
                        │  DISK CACHE  %LOCALAPPDATA%\HoloScope\cache                │
                        │   raw/ (COG tiles, .pbf, parquet)  nodes/ (zstd node blobs) │
                        │   cache.db (SQLite: regions, nodes, geocodes, LRU ticks)    │
                        └────────────────────────────────────────────────────────────┘
```

### 4.2 Cargo workspace layout

```
holoscope/
├─ Cargo.toml                  (workspace)
├─ crates/
│  ├─ hs-app/                  main binary: window, event loop, wiring, CLI flags
│  ├─ hs-geo/                  WGS84/ECEF/ENU/UTM/Mercator math, tiling, extents, f64/f32 discipline
│  ├─ hs-ingest/               providers: cog_dem, srtm_hgt, osm_pbf, overture_pq, overpass,
│  │                           photon, nominatim, worldcover; rate-limiter; cache policy
│  ├─ hs-forge/                point synthesis: terrain, contours, roads, rail, water,
│  │                           buildings, vegetation, skirt/cage/walls generators; LOD mip builder
│  ├─ hs-lod/                  LodNode model, region manifest, SSE traversal, residency/VRAM LRU
│  ├─ hs-render/               wgpu device mgmt, pipelines, WGSL shaders, render graph,
│  │                           bloom/EDL/composite, screenshots, GPU timers
│  ├─ hs-camera/               fly/orbit/walk controllers, floating origin, fly-to splines, ray-pick
│  ├─ hs-ui/                   egui panels, search box, settings editors, FPS graph, help overlay
│  └─ hs-cache/                SQLite index, blob store, LRU, atomic writes, cache GC
├─ assets/                     (fonts, default palettes, icons)
├─ shaders/                    (*.wgsl, included via hs-render build step)
├─ tests/                      (integration + golden renders)
└─ bench/                      (flythrough perf harness + report generator)
```

### 4.3 Threading & data flow rules

- **Main thread** owns wgpu `Device/Queue`, encodes frames, runs egui, camera, LOD selection. Never blocks > 1 ms on IO or decode.
- **Worker pool** (`rayon`, `num_cpus - 2` threads) executes `RegionJob` stages: fetch → decode → synthesize → tier-build → quantize → compress → (disk write ‖ hand-off).
- **Hand-off:** bounded MPSC (`capacity = 96 nodes`); when full, workers slow down (back-pressure) instead of eating RAM.
- **Uploads:** main thread drains ≤ 48 MB/frame into a persistent staging ring; `queue.write_buffer` per node slice; GPU buffer = one big persistent pool with sub-allocated slabs (no per-node `Buffer` objects → no driver churn).
- **Shared state:** lock-free where possible (`AtomicU64` stats); one `RwLock<RegionManifest>`; no mutex on the render hot path.
- **Cancellation:** every job carries a `CancellationToken` (search replaced / extent changed → abort fetches mid-stream, TLS socket close).

### 4.4 Coordinate & precision strategy (critical)

- Authoritative position: **f64 geodetic (lat, lon, ellipsoidal height)**.
- Working space: **local ENU** (east-north-up) in meters, origin = region center, f64 on CPU.
- GPU space: **f32 relative to a floating origin** rebased to the camera each time the camera leaves a 1 km hysteresis sphere; node point positions are **int16/int32 offsets relative to their node origin**, so GPU-side absolute error is ≤ millimeters at any altitude.
- Depth: **reversed-Z floating-point depth buffer** (near ≈ 0.05 m, far ≈ 100 km) — no z-fighting at head height, no far-plane clipping from sky view.
- Appendix A gives the exact transforms.

---

## 5. Geodata strategy (open sources)

### 5.1 Source matrix

| Data | Primary source | Resolution / coverage | Access pattern | License |
|---|---|---|---|---|
| Elevation (terrain) | **Copernicus DEM GLO-30** (AWS open data bucket `copernicus-dem-30m`, COG GeoTIFF) | 30 m global (≈ 1 arc-sec) | HTTPS ranged GET per 1°×1° tile | CC-BY-ish (ESA/Copernicus open) — attribution required |
| Elevation fallback | SRTM/NASADEM `.hgt` (also mirrored on AWS open data) | 30 m | simple fixed-size binary read | public domain-ish (USGS) |
| Elevation boost (regional, optional) | OpenTopography REST (USGS 3DEP 1 m/3 m US, EA LiDAR UK, AHN NL, etc.) | 1–3 m where exists | REST + tiled fetch | per-agency open licenses |
| Roads / rail / water / buildings / POI | **OpenStreetMap** via Geofabrik regional `.pbf` extracts; **Overpass API** for small extents (< ~50 km²) | vector, global | download extract once per region; clip locally | **ODbL — attribution + share-alike aware** |
| Vector fallback / uniform global coverage | **Overture Maps** (S3, GeoParquet: transportation, buildings, places, base) | vector, global | ranged parquet row-group reads by bbox | CDLA-Permissive / ODbL mix per theme — show per-theme credit |
| Land cover / vegetation class | **ESA WorldCover 10 m** (AWS open data, COG) | 10 m global | ranged GET | CC-BY 4.0 |
| Geocoding (name → coord) | **Photon** (photon.komoot.io) for autocomplete; **Nominatim** for canonical resolve + bbox | global | JSON HTTPS, ≤ 1 req/s, cached | ODbL data; service usage policy |
| Offline geocode fallback | GeoNames `cities5000` bundle (optional 3 MB download) | global cities | local prefix search | CC-BY 3.0 |
| Labels / admin names | Natural Earth (public domain) + OSM `name` tags | global | bundled / from PBF | public domain / ODbL |
| User LiDAR (stretch) | drag-drop LAS/LAZ/EPT or open portals (OpenTopography) | varies | local parse / EPT octree HTTP | user's responsibility, shown in UI |

**Policy notes.** Nominatim: identify via `User-Agent: HoloScope/x.y (contact@…)`, ≤ 1 req/s, cache every response forever locally. Geofabrik extracts are country/region files (10 MB–2 GB): for a city request we prefer Overpass (fast, bounded) and fall back to the regional extract when Overpass times out or the extent is large; extracts are cached and clipped with a bounding box so multiple cities in one country reuse one download. Overture parquet gives us a *uniform* global fallback where OSM density is poor (e.g. buildings in some regions) — merged, de-duplicated by centroid proximity.

### 5.2 Region request pipeline (what happens when the user presses "Load")

1. **Resolve** place → `(lat, lon, suggested_bbox)` (cache-first: SQLite `geocode_cache`).
2. **Plan extent**: user dims (W×H km) ∩ suggestion; compute tile lists: DEM 1° tiles, WorldCover tiles, OSM source (Overpass bbox query vs Geofabrik extract), Overture row-groups.
3. **Estimate & confirm**: show projected point counts per layer + download MB (from tile byte ranges & historic density stats) → user confirms or adjusts preset (Draft/Standard/Ultra).
4. **Fetch** (tokio, pooled, ranged, conditional GETs) → `cache/raw/` with SHA-256 names.
5. **Forge** (rayon): per-layer point synthesis (§6) into LOD tiers (§7).
6. **Publish**: region manifest (SQLite + JSON sidecar) lists nodes with AABBs, tiers, counts, byte offsets; LOD selector can now traverse; root tiers published **first** so the scene appears while finer tiers stream.
7. **Attribution set** assembled from actually-used sources → overlay string.

---

## 6. Point-cloud synthesis

### 6.1 GPU point record (12 bytes)

```
struct GpuPoint {            // SoA-friendly, 12 B, 16-byte node stride padding avoided
    pos:      [i16; 3],      // offset from node origin, scaled by node.quant_scale (f32, in push constant)
    color:    u8,            // palette LUT index (per-layer base + elevation band option)
    layer:    u8,            // LayerId (terrain/road/rail/water/building/veg/contour/label/furniture…)
    flags:    u8,            // bit0 emissive-boost (roads/beacons), bit1 fade-group, bits2-7 spare
    intensity:u8,            // per-point brightness jitter / return-strength look
}
```
Node sidecar (push constant / storage buffer): `origin_view: vec3<f32>`, `quant_scale: f32`, `tier_fade: f32`, `size_mul: f32`, `layer_lift: f32`.

12 B/point ⇒ **14 GB VRAM budget ≈ 1.16 billion resident points** on the Titan RTX. Disk (zstd): ~4–6 B/point.

### 6.2 Per-layer generators

| Layer | Input | Algorithm | Default spacing (Standard) |
|---|---|---|---|
| Terrain | DEM grid (resampled 30 m → 10 m bicubic) | sample grid → jitter ±0.35·spacing (Poisson-ish blue-noise via hashed jitter) → height + optional synthetic micro-relief (fBm, amplitude 0.15 m, **toggle**, default ON at ≤ 0.2 m so head-height ground isn't glassy) | 2 m (tiers to 0.25 m near camera via slope-adaptive boost: ×2 density on slope > 25°) |
| Contours | DEM | marching-squares isolines every 10/20/50 m (auto by extent) → points along lines | 0.5 m along |
| Roads | OSM ways (`highway=*`) | centerline resample by class width (motorway 12 m → 4 lanes of points; residential 6 m → 2; path 1.2 m → 1); emissive boost; slight +0.15 m lift to avoid terrain z-fight | 0.3 m along, 0.4 m across |
| Rail | `railway=rail/tram/…` | twin rails (±0.72 m) + sleeper ticks every 0.6 m | 0.3 m |
| Water | `natural=water`, `waterway=*`, WorldCover class 80 | polygon interior blue-noise fill + shoreline ring densified; rivers as ribbon; optional slow shimmer (shader time phase) | 1 m fill, 0.3 m shore |
| Buildings | OSM building polygons + `height`/`levels` (fallback: class-based prior, e.g. residential 7 m) | wall shells: perimeter points stacked per floor height 3 m with window-gap modulation; roof: boundary + sparse interior; emissive window speckle (hashed) for night-holo look | 0.5 m shell |
| Vegetation | WorldCover classes 10/20/30 + OSM `leisure=park`, `natural=wood` | canopy: blue-noise scatter at 1.5 m, height = class prior (trees 6–14 m domed, crops 0.5 m) + trunk sparseness; parks tinted | 1.5 m |
| Labels/POI | OSM `place=*, name=*`, Natural Earth | billboard quads (SDF glyphs), not points; LOD by admin rank | n/a |
| Skirt / cage / walls / outline / beacons | region bbox | deterministic generators (§1.1 items 3–6, 8) | fixed counts |
| Imported LiDAR | LAS/LAZ/EPT | classify passthrough; octree/EPT nodes wrapped as `LodNode`s | as source |

Color: per-layer base hue from active palette × optional elevation ramp; `color` byte indexes a 256-entry LUT uploaded per palette change (palettes: **HoloGreen** (reference), CyanIce, AmberOps, MagentaScan, ElevationViridis, ClassTrue, MonoWhite).

### 6.3 Density presets & projected counts (worked example, 10 × 10 km urban)

| Layer | Draft | Standard | Ultra |
|---|---|---|---|
| Terrain | 12 M | 48 M | 190 M |
| Roads+rail | 3 M | 8 M | 22 M |
| Buildings | 6 M | 18 M | 55 M |
| Water+veg | 4 M | 12 M | 40 M |
| Contours+furniture | 1 M | 2 M | 4 M |
| **Total** | **26 M** | **88 M** | **311 M** |
| Disk (zstd) | ~130 MB | ~420 MB | ~1.5 GB |
| VRAM resident (all tiers ≈ ×1.33) | ~0.45 GB | ~1.5 GB | ~5.4 GB |

(UI shows these live as the user drags the extent/preset sliders; a 25 km region warns and suggests Standard.)

---

## 7. LOD, streaming & memory management

### 7.1 Model: chunked density mips (primary) + octree adapter (imports)

Because we *synthesize* points, LOD is exact and cheap:

- Region is tiled into **256 m × 256 m columns** (quadtree in 2D; height unbounded).
- Each column × layer holds **tiers T0…Tn**, spacing doubling per tier (e.g. terrain 0.25/0.5/1/2/4/8/16/32/64 m). Tier point sets are **nested-subsampled** (T_{k} ⊂ T_{k-1}) so cross-fades never double-density.
- A `LodNode` = (column, layer, tier) → AABB + GPU slab range + count + spacing.
- **Imported LiDAR/EPT** arrives as an octree; an adapter wraps each octree node as a `LodNode` with `spacing = node.extent / 2^depth · src_density` — identical downstream treatment. (Design decision **DD-1**: one mechanism, two providers; spike in M3.)

### 7.2 Selection: screen-space error (SSE)

Per frame, for each candidate node:
```
screen_px = (point_spacing * focal_px) / distance(camera, node_center)
load/render node iff  screen_px >= pixel_threshold      // pixel_threshold ≈ 1.0 (Clarity) / 1.4 (Holo, denser look)
```
plus frustum test (column AABB vs 6 planes, near-plane guard) and a **hysteresis band** (upgrade at 1.4 px, downgrade at 0.8 px) with a 180 ms alpha cross-fade between adjacent tiers → **no popping**.

Traversal cost: columns in a 50 km region ≈ 40 k; tier check is 2 floats per column-layer → ~0.3 ms single-threaded; we cache per-column distance buckets and re-traverse fully only when the camera moves > 1 m or rotates > 0.5°, else reuse.

### 7.3 Point budget & residency

- **Draw budget** (points/frame): default 20 M @1440p, adaptive (§12). Traversal sorts selected nodes by SSE desc and truncates at budget; truncated nodes fall back to their next-coarser resident tier (always keep T_coarse resident per column → *guaranteed no holes*).
- **VRAM residency**: slab pool (default 14 GB cap). On exhaustion: evict LRU *finest* tiers first (never the column's coarse guarantee). LRU ticks from last-drawn frame.
- **Disk cache**: node blobs zstd; GC to configurable cap (default 120 GB) LRU; raw source tiles kept separately (re-forge without re-download).
- **Upload pacing**: inbound queue drained ≤ 48 MB/frame; tier upgrades requested in SSE order so perceived refinement is front-to-center first.

### 7.4 Progressive reveal (aesthetic + functional)

On region load: publish coarse tiers (64/32/16 m) first → the city materializes as a ghostly coarse hologram, then tiers cascade in — this matches the reference "scan" feel and satisfies NFR-04 (first pixels ≤ 4 s). Optional **scan-sweep**: an animated vertical plane that forces tier upgrades + emissive spike as it passes (pure shader uniform + traversal hint).

---

## 8. Rendering system

### 8.1 Frame graph (Holo mode)

```
[0] PointPass      → HDR color target (R16F) + (Clarity: D32F depth)
                     instanced quads, additive (ONE, ONE), depth test ON / write OFF (Holo)
                     depth test+write ON (Clarity)
[1] EdlPass        (Clarity only) 8-neighbor log-depth compare → shading term → composite input
[2] BloomDown      6-mip dual-filter Kawase downsample chain (½…1/64)
[3] BloomUp        tent upsample combine → bloom texture
[4] Composite      HDR + bloom·intensity → tone map (ACES-ish or reinhard ext) → exposure
                     + vignette + optional scanlines/noise + distance-fade fog applied in [0]
[5] LinePass       cage/outline/beacon beams (line list + beam quads), additive
[6] LabelPass      SDF billboard text, alpha blend, depth test ON
[7] UiPass         egui (panels, FPS graph, attribution overlay)
[8] Present        (vsync per settings; fps cap option)
```

GPU frame cost target on Titan RTX @1440p: [0] ≤ 7 ms, [2–4] ≤ 2 ms, rest ≤ 1.5 ms ⇒ ≤ 11 ms GPU, leaving CPU/headroom.

### 8.2 Point sprite technique

- **Instanced quads** (4 verts / 2 tris per point from a shared 4-vertex vertex buffer; per-instance `GpuPoint`): portable (wgpu has no reliable gl_PointSize > 1 path), allows round soft sprites, per-point size, and future GPU compaction.
- Vertex shader: `world = node_origin_view + pos * quant_scale`; view-space → clip; sprite size = `clamp(base_px * size_mul * (focal / -view.z), min_px, max_px)` with mode overrides (fixed / attenuated / adaptive-from-budget).
- Fragment: radial falloff `exp(-4·r²)` core + faint halo; `intensity` & layer emissive scale; round discard outside r > 1; **no sorting needed** (additive). Clarity mode: hard-ish disc + EDL.
- Distance fade: alpha ·= smoothstep(fade_far, fade_near, dist) to dissolve far clutter (visibility aid, §10).
- Jitter mode: hashed sub-pixel temporal jitter (CRT-scan shimmer), amplitude setting 0–1 px.

### 8.3 Shaders (WGSL) — core excerpt (full sources live in `shaders/`)

```wgsl
struct NodeU { origin_view: vec3<f32>, quant_scale: f32,
               fade_near: f32, fade_far: f32, size_mul: f32, tier_fade: f32 };
@group(0) @binding(0) var<uniform> G : Globals;      // view_proj, focal_px, time, point_size_px, ...
@group(0) @binding(1) var<storage, read> NODES : array<NodeU>;
@group(1) @binding(0) var<storage, read> PTS : array<GpuPoint>;

struct VOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32>,
              @location(1) col: vec3<f32>, @location(2) alpha: f32 };

@vertex fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32,
              @location(0) node_id: u32) -> VOut {
    let corner = QUAD[vi];                       // 4-vertex buffer: (-1,-1),(1,-1),(1,1),(-1,1)
    let p  = PTS[ii];
    let n  = NODES[node_id];
    let lp = vec3<f32>(p.pos) * n.quant_scale;
    let view = vec4<f32>(n.origin_view + lp, 1.0);
    let clip = G.view_proj * view;
    let dist = -view.z;
    let px   = clamp(G.point_size_px * n.size_mul * (G.focal_px / max(dist, 0.05)), G.min_px, G.max_px);
    // billboard in view space by offsetting clip.xy by corner * px (screen-space sprite)
    var o : VOut;
    o.clip = clip + vec4<f32>(corner * px * G.pixel_to_clip, 0.0, 0.0);
    o.uv   = corner;
    o.col  = G.lut[u32(p.color)];
    o.alpha = n.tier_fade * f32(p.intensity) * smoothstep(n.fade_far, n.fade_near, dist);
    return o;
}
@fragment fn fs(i: VOut) -> @location(0) vec4<f32> {
    let r2 = dot(i.uv, i.uv);
    if (r2 > 1.0) { discard; }
    let w = exp(-4.0 * r2) + 0.15 * (1.0 - r2);      // core + halo
    return vec4<f32>(i.col * w * i.alpha, w * i.alpha);
}
```
*(instance buffer carries `node_id` per point-slab; indirect draws batch hundreds of nodes per submit — see DD-2 spike: `draw_indirect` with per-node instance ranges vs push-constant-per-draw; both prototyped in M0/M3, pick by measured CPU encode time.)*

### 8.4 Post & visibility aids

Bloom (dual-filter Kawase, 6 mips, threshold + knee), exposure, ACES-ish tonemap, vignette, optional scanline/noise overlay, **EDL** (Clarity), distance fade, layer solo/isolate, high-contrast palette swap, label scale, "dim terrain" quick key to make vector layers pop.

---

## 9. Camera & navigation

### 9.1 Modes

| Mode | Controls | Notes |
|---|---|---|
| **Fly (default)** | WASD move, mouse-look (capture), Q/E down/up, Shift boost, Ctrl brake, wheel = speed log-slider (0.5 m/s … 2 km/s), Space = brake-hold | 6-DoF, exponential speed curve, damping/inertia setting |
| **Orbit** | LMB drag orbit, RMB pan, wheel dolly | pivot = ray-picked point or region center; auto-clamp below terrain optional |
| **Walk / head height** | WASD on ground plane, mouse-look, head height clamp (default 1.7 m, setting 1.2–2.2 m), terrain-follow with step-smoothing | ground clamp samples DEM (CPU bilinear) + building roof check off by default |
| **Cinematic fly-to** | animated ease (position spline + slerp look), duration setting | used for search results, saved viewpoints, beacon jumps |

### 9.2 Precision & comfort

- f64 ENU internally; floating origin rebase (hysteresis 1 km) → **zero swim/jitter at head height** (NFR validation test: 10 min hover at 1.7 m, measure vertex swim < 0.5 mm).
- Reversed-Z float depth; near plane auto = `max(0.05 m, 0.001·speed)`; far 100 km.
- Motion: per-axis damping (setting 0–0.9), optional head-bob off, FOV 30–110 (default 65; auto-widen +6° at boost speeds, toggleable).
- Collision option: soft terrain clamp in Fly (never sub-terrain by default in Walk).
- Ray-pick (CPU ray-march vs current DEM grid + building AABB list, ≤ 1 ms) drives: orbit pivot, beacon placement, "measure point" readout (lat/lon/elev), context menu.

### 9.3 Camera settings (panel)

mode, FOV, sensitivity, speed min/max, wheel curve, damping, head height, ground clamp on/off, terrain clamp on/off, fly-to duration, near-plane mode (auto/manual), orbit pivot lock, invert-Y, raw-input on/off.

---

## 10. Layers, display & camera settings

### 10.1 Layer registry (default ON unless noted)

| Layer | Sub-toggles | Per-layer controls |
|---|---|---|
| Terrain (topology) | micro-relief, slope-density | color mode (palette/elevation/slope), size ×, intensity, lift |
| Contours | interval auto/10/20/50 m | …same set… |
| Roads | motorway/trunk, primary, secondary, residential, path/cycle | width-class density ×, emissive × |
| Rail | rail, tram, subway (off) | |
| Water | rivers, lakes, coast, shimmer anim | |
| Buildings | shells, roofs, window-speckle | height-color ramp option |
| Vegetation | trees, crops, parks | |
| Labels & POI | admin, city, suburb, poi (off) | scale, fade range |
| Imported LiDAR | per-file | classification color mode |
| Holo furniture | cage, grid walls, skirt, base outline, beacons, scan-sweep | cage size margin, wall density, skirt depth |

Quick keys: `1…9` toggle layers, `S` solo hovered layer, `X` isolate vectors (dim terrain), `H` holo furniture.

### 10.2 Display settings (panel, persisted TOML)

point size mode (attenuated/fixed/adaptive) + base px + min/max px · sprite hardness · blend mode (Holo additive / Clarity opaque+EDL) · EDL strength/radius · bloom intensity/threshold · exposure · tone map · palette (7) · elevation exaggeration 1–5× · distance fade near/far · fog color-less (alpha) · background (void/grid/starfield/gradient) · grid floor size · jitter shimmer 0–1 · scan-sweep speed/off · label scale · high-contrast mode · supersample screenshots 1–4× · vsync / fps cap (30/60/120/unlocked) · adaptive quality on/off + target fps.

### 10.3 Example persisted settings (`%APPDATA%\HoloScope\settings.toml`)

```toml
[render]
mode = "holo"                # "holo" | "clarity"
palette = "hologreen"
point_size_mode = "adaptive"
point_size_px = 2.2
point_min_px = 1.0
point_max_px = 14.0
bloom = 0.85
exposure = 1.0
elevation_exaggeration = 1.6
fade_near_m = 2000.0
fade_far_m = 12000.0
background = "void"
jitter = 0.25
scan_sweep = false

[perf]
target_fps = 60
adaptive = true
point_budget_m = 20          # adaptive trims around this
vram_budget_gb = 14
disk_cache_gb = 120
upload_mb_per_frame = 48

[camera]
mode = "fly"
fov_deg = 65
sensitivity = 1.0
speed_min = 0.5
speed_max = 2000.0
damping = 0.35
head_height_m = 1.7
ground_clamp = true

[layers.roads]
visible = true
size_mul = 1.15
intensity = 1.3
lift_m = 0.15
classes = ["motorway", "trunk", "primary", "secondary", "residential"]
```

---

## 11. UI / UX

### 11.1 Layout (egui, dockable panels, dark theme tuned to the holo look)

```
┌───────────────────────────────────────────────────────────────────────────┐
│ ⌕ [ Kyoto, Japan            ▾]  Extent [ 10 × 10 km ▾] Preset [Standard ▾] │  ← command bar
│   est. 88 M pts · ~420 MB · sources: OSM+Copernicus+WorldCover   [LOAD]    │
├───────────────┬───────────────────────────────────────────┬───────────────┤
│ LAYERS        │                                           │ STYLE / PERF  │
│ ☑ Terrain     │                                           │ mode  holo ▾  │
│   ☑ contours  │              3D VIEWPORT                  │ palette ▾     │
│ ☑ Roads  ▸    │         (wgpu surface, egui overlay)      │ bloom ──●──   │
│ ☑ Rail        │                                           │ exagger ─●─   │
│ ☑ Water       │   [attribution: © OpenStreetMap …]        │ fade  ──●──   │
│ ☑ Buildings   │                                           │ ── FPS 60 ──  │
│ ☐ Vegetation  │                                           │ graph 16.6ms  │
│ ☑ Labels      │                                           │ pts 18.4M     │
│ ▸ Holo furn.  │                                           │ vram 6.1/14GB │
├───────────────┴───────────────────────────────────────────┴───────────────┤
│ status: region kyoto_10km · nodes 4 212 · streaming T2… ▓▓▓▓▓░ 78%  [?]   │
└───────────────────────────────────────────────────────────────────────────┘
```

### 11.2 Search flow

1. Type ≥ 2 chars → 250 ms debounce → Photon autocomplete (5 rows: name, context, flag icon via emoji-free text) — cached prefixes served offline.
2. `Enter` on raw `lat, lon` parses directly (DD/MMSS accepted).
3. Selection → Nominatim resolve (cache-first) → bbox suggestion pre-fills extent (user can override) → estimator line updates live → `LOAD` (or `Ctrl+Enter` auto-loads with Standard).
4. Progress: staged bar (resolve → fetch DEM → fetch vectors → forge → stream tiers) with cancel; coarse hologram appears at ~2–4 s; status bar shows tier cascade %.
5. Recent places list (thumbnails = 256 px offscreen renders, free via existing frame buffer).

### 11.3 Other UX

- `F1` help overlay (key map), `F3` perf graph, `F9` screenshot, `F5` reload region, `Ctrl+S` viewpoint, `Ctrl+O` open viewpoint, `Ctrl+I` import LAS/GeoTIFF.
- Beacon workflow: `Ctrl+click` place beacon (beam + diamond + optional label), list panel, jump-to, export GPX/JSON.
- Measure tool: `Ctrl+click` two points → distance/Δh readout.
- Empty state: rotating demo region (procedural terrain, zero download) so first launch is never a blank screen.
- Accessibility: UI scale 0.75–2×, high-contrast UI theme, all toggles keyboard-reachable.

---

## 12. Performance engineering plan

### 12.1 Budgets (Titan RTX, 2560×1440, Holo)

| Stage | Budget |
|---|---|
| CPU: input+UI+camera+traversal+encode | ≤ 4.0 ms |
| GPU: point pass (20 M instanced quads, additive) | ≤ 7.0 ms |
| GPU: bloom+composite+lines+labels | ≤ 3.0 ms |
| Headroom / present slack | 2.6 ms |
| **Total** | **16.6 ms** |

Sanity math (validate in M0 spike): 20 M quads = 80 M verts / 40 M tris per frame (geometry-trivial for Titan-class); vertex fetch 20 M × 12 B = 240 MB/frame = 14.4 GB/s at 60 fps (vs 672 GB/s HBM); fill = 20 M × ~9 px avg sprite = 180 Mpx blended/frame = 10.8 GPix/s (within Titan RTX blend headroom at 1440p; at 4K the adaptive controller trims budget/size automatically).

### 12.2 Technique checklist

- Persistent GPU slab pool + sub-allocation; zero `Buffer` create/destroy at runtime; zero per-frame heap allocs in render path (scratch arena).
- Indirect/instanced batching; node uniforms in one storage buffer; push constants for per-draw scalars.
- No sorting (additive); Clarity uses depth buffer, not sorts.
- Traversal cached across frames (§7.2); column buckets SIMD'd (glam vec4 batches).
- Uploads ring-buffered, ≤ 48 MB/frame; zstd decode on workers; synthesis rayon-parallel per column.
- Reversed-Z, no MSAA (points don't need it; UI/text SDF are analytic).
- Bloom at ½-res seed; composite single pass.
- `tracing` spans → CPU flamegraph in debug; wgpu timestamp queries → GPU pass table in perf panel.

### 12.3 Adaptive quality controller (the 60 fps governor)

- Input: EMA of GPU frame time (timestamp queries) + present latency; 30-frame windows.
- Actuators (in order): (1) point budget multiplier 0.4–1.5×, (2) sprite min-px 1.0→0.75 (fill), (3) bloom mips −1, (4) tier pixel_threshold 1.4→2.0 (coarser LOD).
- Hysteresis: raise only after 90 consecutive good frames; cut within 3 bad frames (fast-cut, slow-restore).
- Manual override locks actuators; panel shows which actuator is engaged ("governor: budget 0.8×").
- Bench harness (`bench/`) replays scripted flythroughs (sky→street, 60 s) and asserts p99 ≤ 16.6 ms + emits CSV/HTML report; runs in CI on the reference machine (manual gate) and on lavapipe (functional only).

---

## 13. Error handling, offline mode, persistence

- **Offline:** cached regions fully browsable; search serves geocache; banner "offline — cached data only"; queued downloads auto-resume on reconnect (detect via lightweight HEAD).
- **Partial source failure:** layer-level degradation (e.g. WorldCover 404 → vegetation layer empty with warning chip, not a failed load); retry with exponential backoff ×3 then skip.
- **Corrupt cache:** SHA-256 verify on read; single re-fetch; second failure quarantines blob.
- **Antimeridian / polar extents:** ENU origin handles poles; region wrap splits into two sub-origins (test case included).
- **Logs:** `tracing` ring (10 MB × 3) in `%LOCALAPPDATA%\HoloScope\logs`; panic hook writes `crash-<ts>.rpt` + guidance dialog.
- **Settings/regions/viewpoints** persisted atomically (write-temp + rename).

---

## 14. Testing & QA

| Level | What |
|---|---|
| Unit | geo transforms (round-trip ≤ 1 mm over ±85°), tile math, quantization error bounds, SSE selector (synthetic scenes), cache LRU, PBF/COG parsers vs golden fixtures |
| Integration | region pipeline against recorded HTTP fixtures (mock server, byte-identical tiles) → deterministic node manifests |
| Golden renders | offscreen 512² renders of 3 fixed regions/modes; SSIM ≥ 0.98 gate (software adapter for determinism in CI) |
| Perf | bench flythroughs on Titan RTX: p99 frame, VRAM peak, upload spikes; regression ±5 % |
| Soak | 8 h idle-orbit on cached region: leak check (RAM/VRAM/handles), fps drift |
| Field | 12-city matrix (dense/flat/hilly/coastal/desert/polar/antimeridian) load-success + attribution correctness |
| Legal | attribution string snapshot test per source combination |

---

## 15. Packaging & distribution

- `cargo wix` → **HoloScope-x.y.z-x64.msi** (per-user install option) + **portable ZIP** (settings in `./profile/` when `portable.flag` present).
- VC++ redist not required (MSVC static CRT); Vulkan loader assumed present (Win10+ ships it; installer warns otherwise).
- Signed binary recommended (cert budget line item); SmartScreen note in docs until reputation builds.
- Auto-update: optional, manifest-JSON on our bucket, delta = full MSI (v1 keeps it simple).
- Bundled: fonts (OFL), palettes, GeoNames cities5000 (optional component checkbox), demo region.
- About dialog: full license & attribution list (ODbL, CC-BY, CDLA), telemetry = **none by default** (opt-in crash reports only).

---

## 16. Roadmap & milestones

| MS | Weeks | Scope | Exit criteria |
|----|----|---|---|
| **M0 Foundation & render spike** | 1–2 | workspace, CI, wgpu window + egui shell, fly camera, synthetic 20 M-pt terrain, point/bloom/composite passes, GPU timers, DD-2 batching spike | 20 M pts @ ≥ 60 fps 1440p on Titan; encode CPU ≤ 4 ms |
| **M1 Geo ingest + terrain** | 2–3 | COG DEM reader (ranged), SRTM fallback, geocoder clients + cache, region planner/estimator, terrain+contour forge, disk cache, progressive publish | type "Kyoto" → terrain hologram ≤ 4 s first pixels; offline re-open instant |
| **M2 Vector layers** | 2–3 | Overpass + Geofabrik + Overture fallback, roads/rail/water/buildings/veg/labels synthesis, layer panel + per-layer controls, attribution overlay | 10 km city all layers Standard ≤ 20 s stream; toggles < 1 frame latency |
| **M3 LOD & streaming** | 3 | tier mips + SSE traversal + cross-fade, budget truncation, VRAM slab pool + LRU, upload pacing, import adapter (EPT read) | no popping in bench flythrough; VRAM cap honored in 8 h soak |
| **M4 Look-dev** | 2 | palettes, EDL/clarity, bloom tuning, holo furniture (cage/walls/skirt/outline/beacons), scan-sweep, jitter, backgrounds, screenshot supersample | side-by-side vs `map1.jpg` reviewed & signed off |
| **M5 Camera & UX polish** | 2 | orbit/walk/head-height, fly-to, ray-pick measure/beacons, viewpoints, help, empty-state demo, accessibility pass | head-height jitter test pass; UX review checklist green |
| **M6 Perf & hardening** | 2 | adaptive governor tuning, 4K profile, error/offline matrix, antimeridian/polar cases, soak, crash reporting, log UX | p99 ≤ 16.6 ms @1440p & ≤ 16.6 ms @4K-with-governor on 12-city matrix |
| **M7 Packaging & docs** | 1 | MSI + portable, signing, About/licenses, manual, bench report template | install/uninstall clean on fresh Win11 VM; docs ship |

**Total ≈ 15–18 dev-weeks.** Two-developer split: (M0→M3 engine track) ∥ (M1→M5 content/UX track) after shared M0 ⇒ ~9–11 weeks.

---

## 17. Risks & mitigations

| # | Risk | P | I | Mitigation |
|---|---|---|---|---|
| 1 | Additive fill-rate wall at 4K with huge budgets | M | M | adaptive governor (size+budget), ½-res bloom, min-px floor; 4K profile ships with budget 8–12 M |
| 2 | Overpass rate limits / timeouts on big extents | M | M | Geofabrik extract fallback + caching; Overture parquet ranged fallback; per-host limiter; resume |
| 3 | Nominatim policy changes / blocks | L | H | Photon primary for AC, Nominatim cached; offline GeoNames bundle; user can run own Nominatim (settings URL) |
| 4 | 30 m DEM too coarse at head height | H | M | micro-relief toggle, slope-density, vector layers carry street-scale detail; OpenTopography 1–3 m where available; LAS import escape hatch |
| 5 | OSM building heights sparse | H | L | class priors + block-level interpolation; height-ramp coloring hides variance; Overture/Microsoft footprints complement |
| 6 | Precision swim at head height | M | H | f64 ENU + node-relative quantization + floating origin; explicit jitter test in CI-of-hands (bench) |
| 7 | wgpu/driver quirks (indirect draws, storage limits) | M | M | M0 spike decides DD-2; fallback path = per-draw push constants; storage-buffer > 128 MB split via bindless-free chunk arrays |
| 8 | VRAM fragmentation from slab churn | M | M | power-of-two slab classes, defrag pass on idle, coarse-tier pinning |
| 9 | License/attribution mistakes (ODbL) | L | H | attribution snapshot tests; legal review of source matrix before M7; per-source credit chips |
| 10 | Scope creep on import formats | H | M | LAS/LAZ/EPT only in v1; GeoTIFF DEM import; everything else v2 backlog |

---

## 18. Appendices

### A. Coordinate math (CPU, f64)

```
geodetic(lat,lon,h) → ECEF (WGS84 a=6378137, f=1/298.257223563)
ECEF → ENU(origin):  R = Ry(90°+lon0)·Rx(90°-lat0) convention;  enu = R·(ecef - ecef0)
ENU → GPU:  p_gpu_f32 = (enu - floating_origin) ; node points stored int16·quant_scale relative to node origin
tile addressing: DEM/WorldCover = EPSG:4326 1° tiles; Overture = bbox-filtered parquet row groups;
                 internal columns = 256 m grid in region-local ENU (axis-aligned, rotation option v2)
vertical: ellipsoidal → orthometric via bundled EGM96 15' geoid grid (±0.5 m ok for viz; setting: show WGS84 vs MSL)
```

### B. LayerId / palette LUT conventions

`LayerId`: 0 terrain, 1 contour, 2 road-motorway … 6 road-path, 7 rail, 8 water, 9 building-wall, 10 building-roof, 11 veg-tree, 12 veg-low, 13 label-anchor, 14 furniture, 15 beacon, 16 imported. LUT built at palette switch: `lut[i] = palette.base[layer(i)] ⊗ ramp(elev_band(i))`.

### C. Disk layout

```
%LOCALAPPDATA%\HoloScope\
  cache\raw\<sha256>.bin            (COG tiles, pbf, parquet fragments)
  cache\nodes\<region_id>\<col>_<layer>_<tier>.znode
  cache\db\cache.db                 (regions, nodes(index+LRU), geocodes, sources_used)
%APPDATA%\HoloScope\
  settings.toml  viewpoints.json  sessions\<region_id>.json  logs\  crash\
```
`.znode` = `[magic u32][ver u16][count u32][spacing f32][aabb 6×f32][quant_scale f32][layer u8][flags u8] + zstd(GpuPoint[])`.

### D. Bench flythrough script (excerpt, `bench/fly_kyoto.toml`)

```toml
[[segments]]
name = "sky_to_street"
from = { enu = [0, 0, 4200], look = [0, 0, 0] }
to   = { enu = [350, -120, 1.7], look = [400, -90, 1.6] }
duration_s = 40
easing = "smoothstep"
assert = { p99_frame_ms = 16.6, vram_peak_gb = 14.0 }
```

### E. Source endpoint cheatsheet

- Copernicus DEM: `https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N<lat>_00_E<lon>_DEM/Copernicus_DSM_COG_10_N<lat>_00_E<lon>_DEM.tif` (ranged GETs; registry: registry.opendata.aws/copernicus-dem)
- WorldCover: AWS open-data COGs (`esa-worldcover` bucket), class raster 10 m
- OSM: `https://download.geofabrik.de/…-latest.osm.pbf`, Overpass `https://overpass-api.de/api/interpreter`
- Overture: `s3://overturemaps-us-west-2/release/<latest>/theme=*/…parquet` (HTTPS ranged)
- Photon: `https://photon.komoot.io/api/?q=…&limit=5` · Nominatim: `https://nominatim.openstreetmap.org/search?format=jsonv2&q=…`
- OpenTopography (regional hi-res): `https://portal.opentopography.org/apibeam?…` (OT DEM/LIDAR APIs)

### F. Crate dependency list (workspace-root excerpt)

```toml
wgpu = "25"            winit = "0.30"        egui = "0.32"      egui-wgpu = "0.32"
egui-winit = "0.32"    glam = "0.30"         rayon = "1.10"     tokio = { version = "1", features = ["rt-multi-thread","net","time"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls","stream"] }
serde = "1"            serde_json = "1"      toml = "0.8"       zstd = "0.13"
rusqlite = { version = "0.32", features = ["bundled"] }
tiff = "0.9"           osmpbf = "0.3"        arrow = "53"       parquet = "53"
fontdue = "0.9"        image = "0.25"        tracing = "0.1"    tracing-subscriber = "0.3"
anyhow = "1"           thiserror = "2"       bytemuck = "1"     dirs = "6"
```
*(versions pinned at M0 start against then-current stable; all MIT/Apache-2.0-compatible — audited in M7.)*

### G. Glossary

**COG** Cloud-Optimized GeoTIFF · **EDL** Eye-Dome Lighting · **ENU** East-North-Up · **SSE** Screen-Space Error · **LodNode** our universal render unit · **tier** density mip of a column-layer · **column** 256 m quadtree cell · **governor** adaptive quality controller · **holo furniture** cage/skirt/walls/outline/beacons.

---
*End of plan. Next step on approval: M0 kickoff — workspace scaffold + render spike (DD-2 decision memo due end of week 2).*

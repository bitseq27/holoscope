# HOLOSCOPE — BUILD LOG

> **Purpose:** single source of truth for what was built, what was learned, and
> what comes next. Append a dated session block per work session; never rewrite
> history. If a future session wakes up in a fresh sandbox, **read this file
> first**, then run `scripts/bootstrap.sh` and `scripts/verify.sh`.

Conventions:
- `[DD-x]` = design decision · `[BUG-x]` = bug root-caiced (kept so we never re-debug it) · `[ENV-x]` = environment fact
- Renders live in `renders/` (never in `out/` or `target/` — see ENV-2).
- Toolchain lives outside the workspace (ENV-1).

---

## Session 1 — 2026-10-12 — environment + M0 "first working version"

**Goal:** bootstrap a rust + wgpu + egui ("rust-egpu") environment in the sandbox,
build the first working vertical slice of HoloScope per `docs/holoscope_plan.md`
(M0 foundation/render spike + first look-dev pass against the reference images),
and stand up this build log.

### Environment profile (this sandbox)

| Item | Value |
|---|---|
| OS | Debian 12 (bookworm), headless (no X/Wayland, no display) |
| CPU / RAM | 2 cores / **1 GiB cgroup limit** (rustc OOMs at -j2 on `naga`) |
| GPU | none → software rendering: wgpu picks **llvmpipe (OpenGL, LLVM 15)**; mesa Vulkan ICDs (lavapipe) also installed |
| Rust | stable 1.99.0 (rustup, minimal profile) |
| Pinned deps | wgpu 25.0.2 · egui/egui-wgpu/egui-winit 0.32.3 · winit 0.30.13 · glam 0.30.10 · image 0.25 (png) |

- **[ENV-1]** Toolchain + cargo caches + build cache live in **`/opt/hs-tools`**
  (`rustup/`, `cargo/`, `target/`, `env.sh`). Rationale: an earlier attempt put
  `.rustup/.cargo` inside `/home/user`; the ~1.5 GB tree blew the workspace
  snapshot cap and the *entire* workspace delta of that session was dropped
  (toolchain, `env.sh`, everything). `/opt` and `/usr` (apt) persist via the
  container layer. `scripts/bootstrap.sh` is idempotent; override root with
  `HS_TOOLROOT`.
- **[ENV-2]** Workspace snapshot excludes dir names `out/`, `target/`, `build/`,
  `dist/`, `node_modules/`, … → **renders go to `renders/`**, never `out/`.
- **[ENV-3]** 1 GiB RAM ⇒ always build with `scripts/build.sh` (=`cargo build -j1`).
- **[ENV-5]** The workspace snapshot excludes `.git/config` (credential safety),
  which drops the whole `.git` dir across some turn boundaries ⇒ **git history
  is NOT a persistence guarantee here; BUILD_LOG.md is**. If `.git` is missing
  at session start: `git init && git add -A && git commit -m snapshot` and move
  on; the log carries the real history.
- **[ENV-4]** No display ⇒ the app has a first-class **headless offscreen mode**
  (`--headless`) that renders the full graph + egui HUD into a texture and writes
  a PNG. This is our CI/golden-render path in this sandbox; the windowed path
  compiles but is untested here (needs a real machine).

### What was built (M0 vertical slice)

Workspace per plan §4.2 (subset; `hs-ingest/hs-lod/hs-cache` arrive with M1–M3):

```
holoscope/
├─ Cargo.toml                 workspace
├─ crates/
│  ├─ hs-geo/                 WGS84⇄ECEF⇄ENU (f64), value-noise fBm/ridged, HeightField,
│  │                          procedural demo region heightfield (plan App. A)
│  ├─ hs-forge/               GpuPoint 12 B record, ImplVertex 16 B, NodeU/Globals/PostU,
│  │                          palette LUTs (5 palettes), generators: terrain, contours
│  │                          (marching squares), road grid, river, buildings (shells+roof+
│  │                          window speckle), veg, skirt, grid wall, cage, base outline,
│  │                          beacon beams+diamonds
│  ├─ hs-camera/              fly (6-DoF, damping, boost) + orbit controllers, reversed-Z
│  │                          infinite-far projection, view/proj export
│  ├─ hs-render/              wgpu device/pipelines/render graph + WGSL shaders:
│  │                          points (instanced quads, additive/premul), lines, world-quads,
│  │                          bloom down/up ×4, composite (ACES+vignette+scanlines+shimmer),
│  │                          egui-wgpu integration, RGBA8 readback
│  ├─ hs-ui/                  egui HUD styled after reference ui.jpg (dark navy panels,
│  │                          monospace, cyan/signal-red accents): command bar, layer list,
│  │                          style sliders, perf block, status/attribution bar
│  └─ hs-app/                 binary `holoscope`: windowed (winit ApplicationHandler) +
│                             headless offscreen PNG mode, CLI flags
├─ scripts/  bootstrap.sh · build.sh · verify.sh
├─ docs/     holoscope_plan.md · reference/{map1_3d.jpg,map_2d.jpg,ui.jpg}
└─ renders/  golden renders (see below)
```

Render graph (M0): `[0] point/line/quad → HDR R16F (additive) → [1] bloom ↓4 ↑4 →
[2] composite (ACES, vignette, scanlines, noise shimmer) → sRGB target → [3] egui`.

CLI: `holoscope [--headless] [--out P] [--width W] [--height H] [--points N]
[--palette NAME] [--no-ui] [--frames N] [--hide csv]`
(`--hide` = layer-isolation diagnostics: terrain,contours,roads,water,buildings,veg,furniture)

### Verification & artifacts

- `cargo test`: 2/2 pass (hs-geo ENU round-trip ≤ 1 µm, noise bounds). 0 warnings.
- Headless renders (llvmpipe, ~2.5–3.3 s/frame at 1.2–1.5 M pts):
  - `renders/holoscope_m0.png` — **hero frame**, 1280×960, HUD on, hologreen, 1.35 M pts
  - `renders/holoscope_m0_noui.png` — viewport only
  - `renders/holoscope_m0_magentascan.png` — MagentaScan palette (nods to `map_2d.jpg`)
  - `renders/m0_iter6.png`, `out/m0_first.png` — earlier look-dev iterations (first is in snapshot-excluded `out/`, kept for local history only)
- Forge: 1.35 M pts in ~0.4 s (8 nodes).

### Decisions

- **[DD-1]** Dep versions pinned to plan Appendix F as resolved on 2026-10-12
  (wgpu 25 / egui 0.32). winit built `default-features=false, features=[x11,rwh_06]`
  (wayland system libs not needed in sandbox; `rwh_06` required for surfaces).
- **[DD-2]** GPU instance record is 16 B (`Sint16x4 + Uint8x4 + Uint32`): wgpu has no
  16-bit ×3 vertex format. The authoritative 12 B `GpuPoint` (plan §6.1) is kept for
  synthesis/disk; repacking happens at upload. DD-2 batching spike (indirect draws)
  deferred to M3 — M0 draws one `draw_indexed` per node (8 nodes).
- **[DD-3]** WGSL/CPU layout rule: every shared struct carries **explicit pad fields**
  so `size_of::<T>` == WGSL size (NodeU 48 B align-16, Globals 176 B, PostU 40 B with
  vec2 8-aligned). No `repr(C, packed)` across the GPU boundary.
- **[DD-4]** Bloom/fullscreen UV convention: fullscreen VS emits **texture-space uv**
  `(u, v-from-top)`; all post fragments use it. (Root cause of BUG-5.)
- **[DD-5]** egui-wgpu 0.32 requires `RenderPass<'static>` → use
  `encoder.begin_render_pass(..).forget_lifetime()` (same as eframe 0.32).
- **[DD-6]** Holo furniture rules learned from look-dev: skirt only on low edges
  (`edge = 1-smooth01((h/max_h-0.22)/0.30)`), single west grid wall (a second wall at
  grazing angle reads as solid stripes), beacon = crossed beam quads + horizontal
  diamond quad.
- **[DD-7]** Headless mode renders egui into the same offscreen target so screenshots
  show the real HUD (doubles as UI golden render).
- **[DD-8]** Clarity mode in M0 = premul-alpha blend + reduced bloom only. Depth buffer,
  EDL, reversed-Z arrive in M4 (plan §8). Holo mode needs no depth (additive).

### Bugs root-caused (do not re-debug)

- **[BUG-1]** `naga` SIGKILL → OOM at 1 GiB with -j2. Build with `-j1` (build.sh).
- **[BUG-2]** `bytemuck Pod` derive fails on `repr(C, align(16))` structs with implicit
  trailing padding → always pad explicitly (DD-3).
- **[BUG-3]** egui 0.32 API drift vs muscle memory: `Margin::same(i8)`,
  `Frame.corner_radius` (not `rounding`), `CornerRadius(u8)`, `RawInput` has no
  `pixels_per_point`, egui-wgpu takes `ScreenDescriptor` (not `Rect`),
  `egui_winit::State::new` has 6 args, `Renderer::new` has 5 (dithering bool).
- **[BUG-4]** WGSL: no assignment to swizzles (`clip.xy = …` illegal) → rebuild vec4.
- **[BUG-5]** *The teal-bands mystery* (cost 3 look-dev iterations): horizontal band /
  vertical-thread artifacts were **bloom chain UV errors** — down/up passes derived uv
  from `@builtin(position)/src_dims` while position is in *target* pixels ⇒ quarter-
  sampled mips + clamp-to-edge smears. Diagnosed by layer-isolation renders
  (`--hide`), fixed by DD-4. Symptom to remember: "ghost bands aligned to screen axes,
  surviving layer toggles" ⇒ check post-chain UVs first.
- **[BUG-6]** Near-field river rendered as a bright bar: 0.5 m station spacing ⇒
  additive overdraw saturation. Water now ~4 m stations (≈6 k pts).
- **[BUG-7]** wgpu 25 renames: `Maintain→PollType`, `ImageCopy*→TexelCopy*`;
  `RenderPass` is invariant (`PhantomDrop`) hence DD-5.

### Known gaps / deferred (honest list)

- Windowed path compile-only (no display here). Needs a smoke test on real HW.
- Headless HUD shows fps 60.0 placeholder; real EMA only in windowed mode.
- Clarity mode shallow (DD-8); no GPU timers yet (plan M0 exit item, deferred M6).
- Settings not persisted (serde/toml deferred, plan §10.3); palette via CLI/UI only.
- Contours faint at hero distance; beacon diamonds read as bars at shallow pitch
  (acceptable; revisit in M4 look-dev).
- `hs-ingest/hs-lod/hs-cache` crates not created yet (M1–M3).

### Next session checklist (M1 — geo ingest + terrain from real data)

1. `scripts/bootstrap.sh && scripts/verify.sh` (expect: tests pass, verify.png renders).
2. Scaffold `hs-cache` (SQLite index + blob store, plan §13/App. C) and `hs-ingest`
   (Copernicus DEM COG ranged reader + Photon/Nominatim clients with ≤1 req/s limiter,
   plan §5). Start with COG reader + golden fixtures (no network in CI).
3. Wire `LOAD` button: region planner + estimator line (plan §5.2) with cancellation.
4. Replace demo heightfield with DEM-driven terrain forge; keep demo region as
   offline empty-state (plan §11.3).
5. Add `scripts/bench.sh` stub + GPU timestamp queries (plan §12).
6. Housekeeping: move hero renders under git-lfs-like discipline? (plain git ok ≤ 10 MB),
   smoke-test windowed on real hardware, add `.gitignore` for `out/`.

### Addendum 1 (same session) — `launch.bat`

- Added Windows launcher `launch.bat` (plan §15 target platform is Win10/11 x64):
  locates cargo (PATH → `%USERPROFILE%\.cargo\bin`), incremental build
  (`debug` default / `release` opt-in), passes extra CLI args through
  (e.g. `launch.bat release --headless --out shot.png`), pauses with guidance on
  build failure or non-zero exit (e.g. missing GPU adapter → suggests headless mode).
- Not executable in this Linux sandbox; syntax is plain cmd.exe, no PowerShell
  dependency. Smoke-test on real Windows hardware remains in the M1 checklist.

## Session 2 — 2026-10-12 — M1: geo ingest + real terrain

**Goal (plan §16 M1):** type a place → real elevation hologram from open data,
cache-first, offline re-open instant. **Exit criteria met** (see verification).

**User report carried in:** `launch.bat` + windowed M0 ran successfully on real
Windows hardware (closes session-1 checklist item "smoke-test windowed").

### What was built

- **`hs-cache`** (new crate): SQLite index (`geocode`, `blob`, `kv` tables) +
  SHA-256 content-addressed blob store with atomic writes and logical-key→content-id
  mapping; `blob_fetch` = cache-first fetch-through (plan §13/App. C, NFR-07).
- **`hs-ingest`** (new crate):
  - `http.rs` — sync HTTPS client, UA `HoloScope/0.2 (…)`, **≤1 req/s per host**
    token-less limiter, ranged GETs. [DD-M1-1] ureq+native-tls instead of plan's
    reqwest+tokio: schannel on Windows (zero C build), openssl on Linux; sync is
    sufficient for M1's single-stream COG reads; revisit at M2 concurrency.
  - `cog.rs` — pure COG/TIFF parser (IFD, tile arrays, ModelPixelScale/Tiepoint),
    DEFLATE tile decode, **predictor-3 undo**, `read_window` (bbox → geodetic grid).
  - `geocode.rs` — Photon autocomplete + Nominatim resolve parsers, raw `lat, lon`,
    both cache-first forever.
  - `lib.rs` — `plan_region` (1° tile list + download estimator), `load_dem`
    (per-tile progress + cancellation), `merge_grids`, ENU resample → `HeightField`.
- **hs-forge**: `ForgeParams.vectors` flag — DEM-loaded regions forge
  terrain+contours+furniture only (synthetic street grid stays demo-only).
- **hs-ui**: command bar gains extent slider, LOAD/CANCEL + staged progress bar,
  autocomplete dropdown (overlay `Area`, so panel sizing stays stable), live
  estimate line (0.88 M pts/km² Standard), real attribution string in status bar.
- **hs-app**: worker-thread load job with mpsc `JobMsg` stream (autocomplete
  debounce 250 ms, resolve, plan, DEM, forge), cancellation token, region hot-swap
  (`upload_region` + camera reframe), cache dir `%LOCALAPPDATA%\HoloScope\cache` /
  `$XDG_CACHE_HOME/holoscope`, headless `--place "…" --extent N` for CI renders.

### Verification (M1 exit criteria)

- `cargo test`: **15 targets ok** incl. new: cache roundtrip, COG parse+window
  against a synthetic in-test COG, photon/nominatim fixtures, rate limiter.
- **Live load, Kyoto 10 km**: Nominatim resolve → 2 COG tiles (ranged) → forge
  1.02 M pts → render. First pixels well under plan's 4 s budget on this 2-core box.
- **Offline re-open**: second Kyoto run = **1.5 s end-to-end, zero network**
  (cache-only; FR-13 satisfied at M1 scope).
- **Hero renders**: `renders/m1_interlaken.png` (real alpine relief, 14 km),
  `renders/m1_kyoto.png` (flat basin + flanking hills — verified against grid
  column stats: 26–49 m basin, 339/470 m flanks), both 1280×960 with HUD.
- Cache footprint after both loads: ~9 MB.

### Decisions

- **[DD-M1-1]** ureq+native-tls (see above); plan §3 reqwest/tokio deferred to M2.
- **[DD-M1-2]** M1 merges 1° tiles into one geodetic grid, then resamples to ENU
  once (480² cells); per-column LOD mips remain M3 territory.
- **[DD-M1-3]** Region label sanitized to ASCII for the HUD (egui default fonts
  have no CJK); full unicode labels deferred to a bundled-font pass (M5).

### Bugs root-caused

- **[BUG-M1-1]** `blob_fetch` stored under content hash but looked up by logical-key
  hash → cache miss every time (test caught it). Lookup now goes through the
  `blobkey:` kv mapping.
- **[BUG-M1-2]** *The garbage-elevation saga*: Copernicus COGs use TIFF
  **predictor 3** (floating-point horizontal predictor, Tech Note 3): inflate →
  per-row byte-wise accumulation → de-plane 4 byte-planes → samples are MSB-first
  (read as BE even though the file magic is `II`). Diagnosed by brute-forcing
  decode orders against an alpine tile and picking the one whose range is
  544–3647 m (mean 1626 m). Without the undo, elevations decode as ~1e36 (pre-clamp)
  or flat+spike sheets (post-clamp). Symptom to remember: "plausible-ish flat slab
  with needle spikes" ⇒ check TIFF predictor before blaming anything else.
- **[BUG-M1-3]** Multi-tile merge with nearest-row assignment → half-pixel seams
  (double-density stripes + tone step at 1° boundaries). Merge now bilinearly
  resamples each output cell from the containing tile.
- **[BUG-M1-4]** ureq 2.12 does not auto-attach a TLS backend with
  `default-features=false` + `native-tls`: must build the agent via
  `AgentBuilder::tls_connector(Arc<native_tls::TlsConnector>)`.
- **[BUG-M1-5]** egui: a second `ui.horizontal` row inside `TopBottomPanel::top`
  mis-measured and half-clipped in this build; command bar returned to a single
  row (dropdown is an overlay `Area`). Revisit if a 2-row bar is ever needed.

### Known gaps / deferred

- Vector layers (roads/buildings/water from OSM) = **M2**; loaded regions are
  terrain+contours+furniture only.
- No LOD mips/streaming yet (M3): one node per layer, whole region resident.
- Clarity/EDL, GPU timers, settings persistence still deferred (M4/M6/M5).
- 1° tile head fetch is 256 KB cached per tile; fine, but a 16 KB two-pass parse
  would be leaner (cosmetic).

### Next session checklist (M2 — vector layers)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. `hs-ingest`: Overpass bbox query (highway/railway/waterway/building/leisure)
   with the existing limiter + cache; OSM PBF path deferred.
3. `hs-forge`: real vector generators per plan §6.2 (road ribbons by class,
   twin-rail, water shoreline+fill, building shells w/ height priors, veg from
   leisure/natural) replacing `vectors:true` synthetics when a loaded region has them.
4. Layer panel: per-layer intensity/size sliders (FR-06).
5. Attribution: append ODbL credit when OSM layers on (FR-15 snapshot test).
6. Housekeeping: consider 16 KB COG head two-pass; bundle a CJK-capable font? (M5).

---

## Session 3 — 2026-10-12 — M2: real vector layers (OSM/Overpass)

**Goal (plan §16 M2):** loaded regions get real roads/rail/water/buildings/veg
from OpenStreetMap; per-layer controls; ODbL attribution. **Done** for extents
≤ 10 km (Overpass); Geofabrik/Overture fallbacks remain backlog (plan §5.1).

### What was built

- **`hs-ingest::osm`**: single bounded Overpass QL query per load
  (`way[highway|railway|natural=water|waterway|building|leisure|natural=wood]`,
  `out geom`), cached forever in hs-cache; polite via the existing 1 req/s client.
  `MAX_OVERPASS_KM2 = 100` guard.
- **`hs-app::osm_to_vector`**: ways → `hs_forge::VectorData` in region-local ENU:
  road class ranks (motorway→path, plan §6.2 widths/lanes), rail gauge ±0.72 m,
  water polys (shoreline ring + jittered interior fill) & river/canal/stream
  ribbons, building shells w/ `height`/`levels`/class priors + window speckle,
  park/wood canopy scatter. **Extent clipping** (runs-split for lines,
  all-inside for polygons).
- **`hs-forge`**: `ForgeParams.osm: Option<VectorData>`; real generators replace
  synthetics when present; `point_in_poly` ray-cast; new unit tests
  (pip square, osm-forge smoke).
- **FR-06 per-layer controls**: `hs_render::LayerTune { size_mul, intensity }`
  per LayerId, applied by rewriting the small NodeU buffer only on change;
  HUD gains a LAYER TUNE combo + two sliders.
- **Attribution**: `hs_ingest::attribution(with_osm)` appends
  "© OpenStreetMap contributors (ODbL)" when vectors were used; snapshot test
  (plan §14 legal-light). Status bar hides key-help when attribution is long
  (overlap fix).
- Windowed LOAD now requests vectors automatically; headless gains `--vectors`.

### Verification

- Live load **Kyoto 4 km + vectors**: Overpass (cached 556 KB) → 1.33 M pts,
  8 nodes; render shows the real street grid, Kamo river ribbon, building
  speckle, Higashiyama slopes — `renders/m2_kyoto_vec.png` (hero, HUD on).
- `renders/m1_kyoto.png` re-rendered post-fix (10 km, terrain-only, M1 scope).
- `cargo test`: 13/13 pass · 0 warnings.

### Bugs root-caused

- **[BUG-M2-1]** *The flat-slab saga*: extent window was centered on the
  **Nominatim bbox center** (for "Kyoto" = 35.098, rural north — admin boundary
  bbox!) while the ENU origin stayed at the resolved point (35.021). 8.5 km
  mismatch ⇒ heightfield sampled outside its DEM window (clamp → flat sheet)
  and every OSM vertex left the i16 quantization range (saturated ±32768).
  Rule now: **one center = the resolved point**; bbox is a suggestion only.
  (M1's Kyoto render had silently sampled north Kyoto hills; Interlaken was
  unaffected because its bbox ≈ point.)
- **[BUG-M2-2]** Overpass `out geom` returns **unclipped** way geometry →
  streaks far outside the slab; fixed with runs-split clipping in the converter.
  Boundary-straddling *polygons* are dropped for now (proper clipper = M2.5).
- **[BUG-M2-3]** `merge_grids` seeded `lat_min` fold with `f64::MIN` (never
  updates) → inf extent → `usize` overflow panic; latent since M1, surfaced by
  the first boundary-straddling window. Seed is `f64::INFINITY` now.
- **[BUG-M2-4]** (process) a greedy debug-strip regex deleted `fn tentry` +
  half of `resize()`; recovered via `git checkout` + clean re-apply. Lesson:
  strip debug traces with anchored edits, never `.*?` across function bodies.

### Known gaps / deferred

- Polygons crossing the extent border dropped (need Sutherland–Hodgman).
- Geofabrik PBF + Overture parquet fallbacks (plan §5.1) not started.
- Labels/POI layer (SDF billboards) still M4/M5 scope.
- Overpass JSON can reach tens of MB for 10 km cities; parse is sync on the
  job thread (fine), but a PBF path would be 10× leaner (backlog).

### Next session checklist (M3 — LOD & streaming)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. Column quadtree (256 m) + density tiers per column-layer (nested subsample),
   `LodNode` AABBs, SSE traversal w/ hysteresis + cross-fade (plan §7).
3. VRAM slab pool + LRU eviction honouring `vram_budget_gb`; upload pacing
   ≤ 48 MB/frame (plan §7.3).
4. Progressive reveal: coarse tiers first (plan §7.4) + scan-sweep uniform.
5. Polygon clipper (Sutherland–Hodgman) to close BUG-M2-2's remainder.
6. Bench harness stub `bench/fly_kyoto.toml` + GPU timestamp queries (plan §12).

---

## Session 4 — 2026-10-12 — M3: LOD, streaming, bench harness

**Goal (plan §16 M3):** chunked density mips + SSE traversal with cross-fade
(no popping), persistent GPU slab pool with ≤48 MB/frame progressive upload,
VRAM budget enforcement, scan-sweep, polygon clipper, bench harness + GPU
timers. **All checklist items done.**

### What was built

- **`hs-lod`** (new crate, plan §7):
  - `build_lod`: region → 256 m columns × layer × **nested density chunks**
    (level set `{i % 2^t == 0}` stored exclusively, so level L = chunks L..=TMAX
    and cross-fades never double density); per-column origins + mm-scale
    int16 quantization (plan §6.1 node-relative idea).
  - Pool layout ordered **coarse-chunk-first** ⇒ the progressive uploader
    reveals coarse→fine and "is node resident" is a contiguous byte check.
  - `Traversal::select`: per-column-layer SSE (`spacing·focal/dist`) with
    hysteresis (upgrade ≥1.4 px, downgrade <0.8 px), 180 ms chunk cross-fade,
    sphere-vs-frustum cull, draw-budget truncation that coarsens the
    lowest-SSE columns first (plan §7.2–7.3).
  - `enforce_budget`: drops finest chunks globally until the pool fits
    `vram_budget_gb`; coarsest chunk per column always kept (no holes).
- **hs-render**: `upload_lod` (one persistent instance pool = plan §4.3 slab
  pool; NodeU array rewritten per frame with fades+tune), `write_pool`
  (≤48 MB/frame), `node_uploaded`, sel-based draws; **GPU timestamp queries**
  (wgpu 25: `Features::TIMESTAMP_QUERY`, resolve+staging readback,
  `read_gpu_ms()`); scan-sweep uniforms (animated vertical emissive plane,
  HUD "sweep" toggle).
- **hs-app**: bench runner `--bench bench/fly_kyoto.toml` (plan App. D):
  scripted ease segments, per-frame CPU+GPU ms + drawn points →
  `bench/out.csv`, p50/p99 summary vs `assert.p99_frame_ms`;
  `scripts/bench.sh` wrapper.
- **hs-forge**: `clip_poly_to_extent` (Sutherland–Hodgman) — boundary-
  straddling OSM polygons now clipped instead of dropped (closes BUG-M2-2).
- **hs-ui**: PERF panel gains POOL MB/upload % + LOD traversal ms.

### Verification

- `cargo test`: **16/16** (hs-lod: chunk-halving, SSE near/fine vs far/coarse,
  budget shrink + coarse guarantee).
- Live Kyoto 4 km + vectors through the LOD path: pool 21.5 MB / 5713 nodes;
  hero view draws 949 k of 1.35 M pts (5386 sels) — culling visibly working;
  `renders/m3_kyoto_lod.png`.
- Bench smoke (`bench/smoke.toml`, 320×240 llvmpipe): cpu p50 72 ms,
  **gpu p50 57.9 ms via real timestamp queries**, CSV per-frame point counts
  rise as the camera dives (LOD adapting). On the reference GPU the same
  toml asserts p99 ≤ 16.6 ms (plan §12).
- Progressive upload: `--frames N` shows POOL % climbing in the HUD; single
  frame drains fully.

### Bugs root-caused

- **[BUG-M3-1]** Frustum planes extracted from glam `to_cols_array_2d()` as if
  rows — culled visible columns in staircase patterns. glam returns
  **columns**; rows must be transposed out.
- **[BUG-M3-2]** Column index overflow: furniture points sit a few metres
  outside the slab → `cx = -1` → `as usize` → multiply overflow panic in
  `build_lod`. Columns now clamped.
- **[BUG-M3-3]** wgpu 25 buffer-usage rules for timestamp readback:
  MAP_READCOPY_SRC and MAP_READ⊕QUERY_RESOLVE are both illegal ⇒
  resolve buffer (QUERY_RESOLVE|COPY_SRC) + staging buffer (COPY_DST|MAP_READ)
  + `copy_buffer_to_buffer` before mapping.
- **[BUG-M3-4]** Terminal LOD chunk equals its predecessor in size by
  construction (remainder of the nested set) — test expectation corrected,
  not the code.

### Known gaps / deferred

- Quadtree is a uniform column grid (plan says quadtree; uniform is exact for
  axis-aligned extents — revisit only for polar/antimeridian cases, M6).
- LRU eviction across *multiple* regions deferred (single-region pool;
  budget enforced at upload). Cross-fade is per chunk, not per column pair.
- Geofabrik/Overture fallbacks, labels/SDF billboards: still backlog.

### Next session checklist (M4 — look-dev sign-off)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. Clarity mode real: depth buffer + eye-dome lighting pass (plan §8.1/8.4).
3. Palettes/look side-by-side vs `docs/reference/map1_3d.jpg`; tune bloom/
   exposure/sprite hardness; background modes (void/grid/starfield/gradient).
4. Labels & POI billboards (fontdue SDF) — admin/city tiers (plan §6.2).
5. Beacons: click-to-place ray-pick + list panel (plan §11.3).
6. Screenshot F9 supersample 1–4× (FR-14).

---

## Session 5 — 2026-10-12 — M4: look-dev sign-off pass

**Goal (plan §16 M4):** clarity mode real (depth+EDL), background modes,
sprite hardness, labels/POI, click-to-place beacons, supersample screenshots.
**All six checklist items done** (labels screen-space — see DD-M4-1).

### What was built

- **Clarity mode**: scene pass now always carries a Depth32Float attachment;
  clarity point pipeline = depth test Less + write, premul-alpha blend;
  **EDL** (8-neighbour log-depth contrast, `edl`/`edl_r` sliders) applied in
  composite; holo pipelines use compare-Always/no-write so one pass serves both
  modes (plan §8.1/8.4).
- **Backgrounds** (plan §10.2): void / **grid** (3D floor line lattice built at
  upload from LOD AABBs) / **stars** (hash starfield + twinkle) / **grad**
  (dusk gradient) — sky pixels detected via depth==1 in composite.
- **Sprite hardness** slider: mix(soft gaussian+halo, crisp LiDAR disc).
- **Scan-sweep**: animated vertical emissive plane (Globals sweep_pos/on),
  HUD "sweep" toggle; `--sweep` headless.
- **Labels & POI**: Overpass query gained `node[place=city|town|village|
  suburb|quarter]`; names prefer `name:en` (ASCII-safe for egui default fonts);
  rank-gated distance culling; screen-space projection with dark-halo double
  pass in the viewport painter. **DD-M4-1**: screen-space egui labels for M4;
  GPU SDF billboards (fontdue, plan §6.2) deferred to M5.
- **Beacons**: ctrl+click ray-march pick vs heightfield (400 steps) →
  `beacon_quads()` (extracted from forge) re-uploaded to a dynamic buffer;
  BEACONS panel section (count, clear, hint). F9 = supersampled screenshot
  (1–4×, `screenshots/` dir) via temp resize+render+readback (FR-14).
- **Traversal cache** (plan §7.2): re-traverse only when camera moves >1 m or
  >0.5°, or fades active → Kyoto 4 km traversal 43 ms → **3.6 ms** warm.
- Headless flags: `--clarity --bg N --sweep` for look-dev golden renders.

### Verification

- 16/16 tests, 0 warnings.
- `renders/m4_kyoto_clarity.png` — clarity+EDL+grad bg+labels (sign-off sheet
  vs `docs/reference/map1_3d.jpg`: holo additive glow ↔ clarity readability both
  present; palette/hardness/bloom tunable live).
- `renders/m4_kyoto_labels.png` — holo + place labels ("Kyoto", wards).
- `renders/m4_kyoto_grid_sweep.png` — grid floor + scan-sweep mid-pass.
- Bench harness from M3 still green (`scripts/bench.sh`).

### Bugs root-caused

- **[BUG-M4-1]** Clarity crash: wgpu requires pipeline depth-stencil FORMAT to
  match the pass attachment (Some vs None incompatible) → all scene pipelines
  now declare Depth32Float (holo: compare Always, write off).
- **[BUG-M4-2]** Stale Overpass cache: query schema gained place-nodes but the
  blob key was bbox-only → 0 labels from cache. Key now `overpass#v2#…`.
  Rule: version cache keys with query schema.
- **[BUG-M4-3]** Japanese place names: `name` non-ASCII → labels silently
  dropped by the glyph-safety filter; `name:en` preferred (Kyoto/Japan OSM
  data carries it consistently).
- **[BUG-M4-4]** Labels drawn but invisible: unmultiplied dark-teal over bright
  additive glow ≈ zero contrast; fixed with halo double-pass + brighter ink +
  min fade 0.65.

### Known gaps / deferred

- Boundary-straddling polygons clipped (Sutherland–Hodgman ✓) but holes inside
  water polys (islands) ignored; multipolygons not merged.
- Labels: no depth-occlusion test (screen-space), no SDF billboards (M5).
- Measure tool, viewpoints, help overlay: M5 (plan §11.3).
- Look sign-off on reference GPU still pending (llvmpipe only here).

### Next session checklist (M5 — camera & UX polish)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. Orbit pivot ray-pick + walk/head-height mode w/ terrain clamp (plan §9).
3. Fly-to splines for search results/beacon jumps; saved viewpoints (Ctrl+S/O).
4. F1 help overlay (key map), F3 perf graph sparkline, empty-state rotating demo.
5. SDF billboard labels (fontdue) replacing screen-space painter (DD-M4-1).
6. Accessibility pass: UI scale, high-contrast theme (plan §11.3).

---

## Session 6 — 2026-10-12 — M5: camera & UX polish

**Goal (plan §16 M5):** orbit pivot + walk/head-height, fly-to, viewpoints,
help/perf overlays, empty-state turntable, accessibility. **Done**, plus the
plan's "dense up close" near-field tier. SDF billboard labels deferred
(DD-M4-1 stands; screen-space labels gained terrain-occlusion culling instead).

### What was built

- **hs-camera**: `Mode::Walk` (WASD on ground plane, mouse-look, head-height
  terrain-follow with step smoothing, 1.2–2.2 m setting), `clamp_ground` for
  fly, `sensitivity` field; tests: walk holds 1.7 m ±5 cm over 240 steps, and
  f32-vs-f64 view-space swim < 0.5 mm at head height (plan risk #6 / NFR).
- **Fly-to** (`FlyTo`): smoothstep position lerp + shortest-arc yaw/pitch;
  triggers: region LOAD (overview pose), beacon-list jump buttons, Ctrl+O
  viewpoint recall; cancelled by any user input.
- **Viewpoints**: Ctrl+S append-save / Ctrl+O cycle-load, persisted to
  `<data>/viewpoints.json` (serde), count in CAMERA row.
- **Orbit polish**: pivot = screen-centre ray-pick on mode entry (fallback
  ahead-point); RMB drag pans the pivot; wheel dollies (existing).
- **Empty state**: slow auto-turntable around the region until first input
  (plan §11.3 "first launch never blank" — demo region + turntable).
- **Near-field tier** (plan §1.2 "≤1 px spacing at head height"): dynamic
  0.6 m-jittered 120 m patch node (2 cm quantization) regenerated when the
  camera moves >15 m; distance-faded 90→300 m; drawn as an extra NodeU slot
  appended to the per-frame node array (`near_id`).
- **Overlays**: F1 help window (key map), F3 frame-ms sparkline (120-sample
  ring, 16.6 ms target line) drawn with the viewport painter.
- **Accessibility** (plan §11.3): UI zoom slider 0.75–2× (`set_zoom_factor`),
  high-contrast theme variant (`apply_theme_hc`).
- **Label occlusion**: camera→label ray-march vs heightfield hides labels
  behind ridges (cheap substitute for depth-tested billboards).
- Headless flags: `--eye x,y` (street-level pose), `--help`.

### Verification

- 18/18 tests (walk head-height, f32 swim bound), 0 warnings, verify green.
- `renders/m5_kyoto_street.png` — **street-level hero**: eye at 1.7 m inside
  the cloud, near-field floor solid, roads/buildings at eye height, beacon
  column overhead. This is the plan's head-height experience.
- `renders/m5_help.png` — F1 overlay sheet.

### Bugs root-caused

- **[BUG-M5-1]** *The buried camera*: `z_exag` scales world z in the shader but
  camera/eye/walk/clamp/ray-pick heights used raw DEM metres → at eye height
  the camera sat ~20 m under the exaggerated surface (black foreground, sky
  wash). Rule: every camera-side ground query multiplies by the active
  exaggeration.
- **[BUG-M5-2]** Near-field first attempt encoded z as `h - h` = 0 (copy-paste
  of the origin expression) and lived only in the windowed path; extracted to
  `build_nearfield()` shared by headless/windowed.
- **[BUG-M5-3]** egui 0.32 `allocate_painter` returns `(Response, Painter)`,
  not `(Rect, Painter)`.

### Known gaps / deferred

- SDF billboard labels (fontdue) — screen-space + occlusion covers M5; GPU
  text remains the M6+ item together with measure tool & help-overlay polish.
- Walk mode roof-collision check off (plan default) — unchanged.
- Near-field patch is terrain-only (vectors don't densify up close yet).

### Next session checklist (M6 — perf & hardening)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. Adaptive quality governor (plan §12.3): EMA GPU frame time → budget/size/
   bloom-mip/threshold actuators with fast-cut/slow-restore hysteresis;
   governor line in PERF panel.
3. Offline matrix: source-failure degradation chips (WorldCover/Overpass down),
   corrupt-cache quarantine path (plan §13).
4. Antimeridian + polar extent test cases (plan §13/§17 risk 6).
5. 8 h soak script (idle orbit, RSS/VRAM sampling) for the cached region.
6. SDF billboard labels (fontdue) if governor lands early.

---

## Session 15 — 2026-10-15 — Overpass outage hardening (user log: all mirrors failing)

**User log:** de = schannel `SEC_E_ILLEGAL_MESSAGE` handshake blip; kumi/coffee
= HTTP 500; ru = timeout. Sandbox reproduced the **same storm simultaneously**
(all mirrors 5xx) ⇒ public Overpass infrastructure outage, not a client bug.
Client-side response (outages can't be fixed, only survived):

- **Class-staged queries**: stage 1 = highway/rail/water/waterway/place-nodes
  (light, high value); stage 2 = buildings/leisure/wood (heavy). An overloaded
  server now degrades by class — roads+water+labels land even when the
  building query dies; separate cache keys per stage.
- **Recursive 4-way chunk split** on failure (min cell ~1.5 km): small queries
  are what overloaded instances still answer.
- **Retry ladder**: 3 attempts × 7 mirrors (added `www.overpass-api.de`,
  `overpass.osm.ch`) with 15 s/30 s backoff; TLS handshake blips, 5xx and
  timeouts all treated retryable.
- **Global 240 s deadline** for the whole vector phase: on expiry the app
  renders with whatever landed ("partial vectors" log + degraded chip) instead
  of hanging for ten minutes; per-chunk successes are cached, so **re-LOAD
  resumes** and accumulates coverage across attempts.
- Cache keys bumped to `overpass#v4#<stage>#<box>` (staging invalidates v3).

Tests: 25/25 (query-stage test updated: stage 1 must NOT contain buildings).
Live verification blocked by the ongoing outage itself — Kyoto 2 km probe
exercised the full retry/split/deadline path and terminated cleanly at the
deadline with partial data; cached Tokyo/Kyoto chunks still render.

**User guidance baked into logs:** "deadline reached; using partial vectors
(re-LOAD later to complete)".

---

## Session 14 — 2026-10-15 — auto-LiDAR from a free API + Tokyo vectors fix + 10 M default

**User report:** "no lidar … should load from a free api; no roads/buildings
for Tokyo ('vector layers degraded'); set 10m points as default."

### Auto-LiDAR (free, keyless) — OpenTopography chain

Probed live: the old `apibeam` clipping endpoint is **dead (404)**. The
current documented free path (OT tile-index tutorial, Nov 2025) is:
1. `GET /API/otCatalog?minx…&productFormat=pointcloud&detail=true` (keyless;
   footprints as geojson Polygon/**MultiPolygon** — first parse handled only
   Polygon and silently dropped every dataset, BUG-M6.5-6),
2. tile index `https://opentopography.s3.sdsc.edu/pc-bulk/{alt}/{alt}_TileIndex.zip`
   (ZIP → **.shp** tile bounds + **.dbf** URL column — both parsers added,
   ~120 lines, no geo deps),
3. LAZ tiles → decoded with the **`laz` crate** (Apache-2.0): VLR scan
   (54-byte VLR header — forgot the reserved u16 first, BUG-M6.5-7),
   LAS 1.4 64-bit point count (legacy field is 0 there, BUG-M6.5-8),
   decompress + clip in dataset CRS.
4. CRS: dataset EPSG from catalog, else **UTM zone inferred from MGRS tile
   names** (`11SGC…` → zone 11 N); WGS84→UTM forward (Snyder) added + tested.
   State-plane datasets: auto-skip with a chip (use `--las`).
Auto-attempt runs on every LOAD (central ≤3 km window, ≤4 tiles, cached);
opt-out `--no-lidar`. Attribution appends the dataset name. Coverage is
regional (US/UK/NL/NZ/…): **Japan has none** → clean "no open LiDAR
coverage" chip, not an error. Live-verified up to decompress; a 19.5 M-pt
tile needs ~1 GiB to inflate (sandbox limit; desktops fine) — per-tile cap
25 M, oversize tiles skip with a log line. `laz_roundtrip` synthetic-container
test dropped (writer-side chunk-table patching is a test-artifact problem;
decode path covered by negative test + live probes).

### Tokyo vectors ("degraded")

Overpass 502/500-storm on big city queries → resilience stack:
**~3 km chunking** (≤16 chunks, partial success accepted), **4 mirrors** in
failover (de → kumi → private.coffee → osm.ru), **1 retry with 12 s backoff**,
per-chunk cache keys (interrupted sessions resume), query timeout 180 s.
Live: Tokyo 10 km now loads 7.3 M pts via kumi after de 502'd.

### "Dashed/invisible roads at altitude" root cause (BUG-M6.5-5)

LOD base spacing was estimated as sqrt(area/count) — meaningless for linear
features (roads estimated 9 m apart → SSE picked ultra-coarse chunks → the
network vanished at aerial distance; checkerboard = mid-cross-fade columns).
Fix: generators now publish **true spacing hints** per node (roads 3 m/vd,
shells 1 m/vd, fills, lidar 1 m); `hs-lod` uses them per node. Tokyo aerial
now shows the full street grid + wards labels (`renders/m6_tokyo.png`).

### 10 M default

`--points` default and windowed LOAD target = **10 000 000** (user request).
Demo empty-state stays 3 M for the NFR-06 cold-start budget.

Verification: 25/25 tests (UTM forward, LAZ reject, + all prior), verify
green, Tokyo 10 km hero above, sandbox pool note: 117 MB pool exceeds the
1 GiB box's staging at 6 M pts (desktop-unaffected; tested at 3 M).

---

## Session 13 — 2026-10-12 — "lidar is not showing" (placement bug)

**Report:** imported LiDAR invisible (windowed, Windows).

**Root cause (BUG-M6.5-3):** the scan's vertical placement was baked at load
time against a ground height that, in windowed mode, did not exist yet
(`App.hf` is only created in `resumed()`), and was never re-aligned when a
place was LOADed afterwards → the scan sat at z≈0 while region terrain sat
tens of metres up (or vice-versa): buried or floating, i.e. invisible.

**Fix — place-at-merge via node origin:**
- Bake z as height above the scan's own ground (median of class-2 points,
  fallback min); ground ref stashed in `origin.w`.
- `attach_lidar(region, node, hf)` sets `origin.z = hf.sample(0,0)` at every
  merge site (startup demo, region swap, density re-forge, headless).
- `hs-lod::build_lod` now dequantizes **with node origin** (it previously
  assumed origin 0 — latent bug that would have bitten any future relocated
  node too).
- Diagnostics: `[lidar] N points …`, `ground_ref=…`, `merged into region at
  z=…` so a missing scan is traceable from logs.

Verified: eye-level render inside the demo scan shows buildings/roofs/crowns
sitting on the terrain (`renders/m6_lidar_street.png`); 23/23 tests; 0 warnings.

**Usage recap (for the report reply):** LiDAR is opt-in per launch:
`launch.bat --las scan.las` or `--lidar-url …` (+ `--las-center`, `--lidar-max`);
layer row "imported lidar" (id 16) toggles/tints it; CLARITY mode reads it best.

---

## Session 12 — 2026-10-12 — Windows crash fix: zero-size buffer slice (user report)

**Report (local Windows, Vulkan):** after `[region] loaded Greater London`,
panic `slice offset 0 is out of range for buffer of size 0`
(wgpu buffer.rs:875), then a teardown cascade
(`SurfaceSemaphores still in use` — cosmetic, consequence of the first panic).

**Root chain (reconstructed + partly reproduced):**
1. London at default 10 km extent → Overpass query too big/slow → vectors
   **skipped** (degradation path working as designed) →
2. `region.road_quads` empty → `create_buffer_init` made a **0-byte buffer** →
3. first frame bound `road_quad_buf.slice(..)` unconditionally → wgpu panic.
   (Sandbox never hit it because vectors always loaded here.)
   Locally this also surfaced a second latent issue: an **interrupted earlier
   session** had left a half-written overpass blob; the M6 quarantine path
   caught it (verify-on-read → quarantine → refetch), confirming §13 design.

**Fixes:**
- [FIX] all furniture/ribbon/grid buffers get a 16-byte minimum size; every
  such draw is now count-gated (`n_lines/n_quads/n_road_quads/n_grid > 0`);
  pool bind guarded by `pool.size() > 0`. Whole panic class eliminated
  regardless of which list is empty (BUG-M6.5-1).
- [FIX] `blob_fetch` direct writes are now atomic (tmp + rename), so an
  interrupted download can no longer poison the cache (BUG-M6.5-2).
- [DIAG] `[gpu-buf] lines=… quads=… road_quads=… verts` line at upload makes
  any future empty-list situation visible in logs instead of a panic.
- Verified: vectors-skipped path now renders (road_quads=0 case exercised);
  full Kyoto vector render shows road_quads=478 416; 23/23 tests; 0 warnings.

**User action:** update the working copy (this fix), re-run `launch.bat`.
If Overpass times out for big city extents, the app now degrades gracefully
(chip in the status bar) instead of crashing; retry with extent ≤ 6–8 km for
vectors, or re-LOAD later (cache-first).

---

## Session 11 — 2026-10-12 — `--software` / `--adapter` (remote testing request)

User tests remotely where no GPU is exposed; adapter selection was hardcoded
to HighPerformance with no fallback. Added:

- `Renderer::new_with_prefs(instance, surface, fmt, AdapterPrefs { software, name })`:
  1. `--adapter <substring>` (case-insensitive, e.g. `llvmpipe`, `lavapipe`,
     `Basic Render`, `WARP`, `SwiftShader`) picks from `enumerate_adapters`;
  2. `--software` / env `HOLOSCOPE_SOFTWARE=1` prefers known software
     rasterizer names, else wgpu `force_fallback_adapter`;
  3. default path unchanged, but a failed hardware request now **auto-retries
     once with the fallback (software) adapter** — remote sessions work with
     zero flags.
- CLI: `--software|-sw`, `--adapter NAME`; env `HOLOSCOPE_ADAPTER`.
- `launch.bat`: documented in header + failure hint suggests `--software`.
- Verified in sandbox: `--software` → "software adapter: llvmpipe";
  `--adapter llvmpipe` → name-filter pick; bogus filter → clean error.
- Note: on Windows the software options are typically **WARP / Microsoft Basic
  Render Driver** (DX12 fallback) or SwiftShader (Vulkan); on Linux
  llvmpipe/lavapipe.

---

## Session 10 — 2026-10-12 — LiDAR import path (user request: "lidar next")

**Goal:** measured-3D via LAS import + URL fetch (plan FR-12 / §5.1 elevation
boost; session-8 decision `hs-ingest::lidar`). **Done** for LAS 1.2–1.4
formats 0–3 & 6–8; LAZ rejected with a clear error (decompress-first, plan
lists LAZ later).

### What was built

- **`hs-ingest::lidar`**: header/point parser (scale-offset decode, 64-bit
  counts for 1.4, class offsets per format), `looks_geographic()` heuristic,
  minimal LAS 1.2 writer (tests + demo-scan example), round-trip test.
- **Placement rules** (documented pragmatism, no proj dep per plan §3):
  geographic coords → ENU via `--las-center lat,lon`; projected/local coords →
  centred on centroid horizontally and **sat on the region ground**
  (`z - zmin + ground_z`) so scans aren't buried under terrain.
- **Classification → colour**: ground(2)→elevation ramp, building(6/17)→
  building slot, veg(3/4/5)→veg, water(9/13)→water, else→imported amber
  (new LUT slot 81, `layer::IMPORTED = 16`, COUNT 17, layer row + tint slot).
- **Merge**: imported points become a normal `Node` → `build_lod` columnises
  them, so LOD traversal, per-layer tune, tint, visibility and the VRAM budget
  all apply to LiDAR for free. Survives density re-forge + region swaps.
- **CLI**: `--las PATH`, `--lidar-url URL` (cache-first blob), `--las-center`,
  `--lidar-max N` (stride-subsample cap, default 4 M).
- **`make_demo_las` example**: synthetic urban scan (ground 1 m, 20 buildings
  flat+gable roofs 0.7–0.8 m shells, 60 tree crowns) → `renders/demo_scan.las`
  (483 k pts) as the verification artifact.
- **[FIX] near-distance fade** in points.wgsl: additive blending saturates
  inside ~10 m of the camera at scan densities; alpha now fades in 1.5→12 m.
  Improves every street-level view, not just LiDAR.

### Verification

- 23/23 tests (LAS round-trip incl. classification), 0 warnings.
- `renders/m6_lidar_street_clarity.png` — **the LiDAR hero**: clarity mode
  (depth+EDL) reads measured structures cleanly: gable roofs, floor bands,
  crowns, ground. Holo variant (`m6_lidar_street.png`) keeps the glow look;
  additive saturation at scan density is why clarity is the analysis mode
  (plan §8.4 intent).
- Pool with lidar: 41.3 MB / 3669 nodes on the demo region.

### Known limits (honest)

- No CRS reprojection (proj rejected in plan §3): projected LAS is placed
  centroid-relative; axis-aligned north. Fine for viz, not for surveying.
- LAZ unsupported (error message says so); octree/EPT streaming still backlog
  (plan §7.1 adapter).
- OpenTopography is consumed as a plain URL (`--lidar-url …apibeam…`); no
  product-catalog browser yet.

### Real measured-3D sources (for users)

- OpenTopography apibeam: `https://portal.opentopography.org/apibeam?south=…&north=…&west=…&east=…&product=USGS_Lidar_Point_Cloud_…&outputFormat=las`
- USGS 3DEP (US 1 m), EA LiDAR (UK 1 m), AHN (NL 0.5 m) tile portals.
- Anything local: `--las scan.las`.

### Next session checklist (M7 — packaging & docs)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. About dialog (licenses/attribution/version/adapter), manual.md.
3. cargo-wix MSI recipe + portable ZIP (`portable.flag` → ./profile).
4. LAZ support decision (laz crate? external laszip binary?) — backlog review.
5. Final golden sheet vs all three references; tag v0.7.0.

---

## Session 9 — 2026-10-12 — density presets + continuous road ribbons (user feedback 2)

**Report:** "roads/buildings need more points; zoomed roads look dashed; add
low/med/high/ultra (1/5/20/50 m)". All done:

- **Continuous road & water ribbons**: additive triangle-ribbon cores under
  the point texture (width = class width × 0.35, alpha ~0.34), so roads read
  as solid glowing lines at any zoom — matching reference `map1_3d.jpg`, where
  the network is continuous, not dotted. Ribbons are **palette-aware**: the
  line/quad shader now samples the LUT via a new `slot` field on `LineVert`
  (32 B); furniture keeps baked colours through a white slot 80.
- **Density presets** Low/Med/High/Ultra (×0.5/1/2/4 vector density): scale
  vector point budgets and base spacings (roads 3 m/vd along, building shells
  1 m/vd, water/veg fills √vd). Default **High**. Combo replaces the static
  "PRESET Standard" label; changing it **re-forges from cached sources in a
  worker thread** (no network) and hot-swaps the region.
- Headless: `--density low|med|high|ultra|N`.
- Ultra Kyoto 4 km: 3.41 M pts, pool 54.6 MB — inside the 14 GB VRAM budget
  with room for ~200× more; `enforce_budget` remains the backstop.
- Renders: `renders/m6_roads_zoom.png` (street-level ultra: continuous roads,
  building shells at eye height), `renders/m6_kyoto_final.png` (aerial, High).
- 22/22 tests, 0 warnings.

Notes: ribbon geometry is forged per region (not per frame), so density
changes cost a re-forge (~1–3 s), amortised by caching. Dashed-road symptom
at extreme zoom (<5 m) can still appear for the *point* texture; the ribbon
guarantees continuity regardless.

---

## Session 8 — 2026-10-12 — user-feedback round (field report from Windows/iPad use)

Six field issues reported by the user; all addressed:

1. **"Roads don't fully load / buildings don't load"** → [BUG-M6-2] the M2
   generators capped each layer at its point budget and `break`-ed mid-loop, so
   only the *first N* ways/footprints rendered (dense nowhere-beats-everywhere
   inverted). Fix: every OSM generator now measures its demand first (road
   lengths × lanes, building perimeters × floors, polygon areas) and scales
   spacing/floor-step/building-stride so the **whole network fits the budget**.
   Kyoto 4 km now shows complete street grid + all blocks (see
   `renders/m6_kyoto_final.png` vs `m2_kyoto_vec.png`).
2. **"Source for true 3D buildings?"** → answered + logged as backlog M6.5:
   current shells use OSM `height`/`levels`/class priors (plan §6.2). Measured
   3D options ranked: (a) **OpenTopography LiDAR** (USGS 3DEP US 1 m, EA LiDAR
   UK, AHN NL — real roofs/trees, plan §5.1 "elevation boost"), (b) Microsoft
   Global ML Building Heights (per-building estimated height, CDLA),
   (c) Cesium OSM Buildings / Google Photorealistic 3D Tiles (proprietary,
   token-licensed — rejected for v1 open-license policy NFR-10),
   (d) user LAS/LAZ drag-drop import (plan FR-12). Next session: OT provider +
   LAS 1.x reader as `hs-ingest::lidar`.
3. **Search dropdown persisted after selection** → dropdown now hides when the
   box text equals the resolved label, and results clear on LOAD commit and on
   region swap.
4. **"Can't rotate on iPad"** → touch support: one-finger drag = look/rotate,
   two-finger pinch = dolly (winit `Touch` events). Also found+fixed
   [BUG-M6-3]: **orbit mode never rotated** — `Camera::orbit_drag` was never
   called from the event loop (mouse drag did nothing in orbit); drag deltas
   now route per mode (fly/walk look, orbit pivot drag).
5. **Point count visibility** → status bar shows "pts X.XXM drawn" always
   (PERF panel keeps the full breakdown).
6. **Per-layer colour box/picker** → each LAYERS row has an egui colour
   swatch; editing sets a per-layer tint override (terrain tints multiply the
   elevation ramp, flat layers replace their LUT slot, furniture also dims
   its dim-slot); "↺" resets to the palette default; palette switch resets
   all overrides. Renderer API `set_layer_tints`, LUT rebuilt on change.

Verification: 22/22 tests, 0 warnings, verify green, regression hero
`renders/m6_kyoto_final.png` (complete urban fabric + pickers + pts readout).

---

## Session 7 — 2026-10-12 — M6: perf governor & hardening

**Goal (plan §16 M6):** adaptive 60 fps governor, error/offline matrix,
antimeridian/polar cases, soak harness. **Done** (reference-GPU p99 gate and
the real 8 h soak remain hardware-pending, as plan §14 intends).

### What was built

- **Governor** (plan §12.3, `hs-app::Governor`): EMA of GPU frame time vs
  16.6 ms; actuators in order — point-budget ×(0.4–1.5), sprite min-px
  1.0→0.75, bloom mip −1, tier threshold 1.4→2.0; **fast cut** (3 bad frames)
  / **slow restore** (90 good); manual override via PERF GOVERNOR panel
  (auto checkbox + manual budget slider); governor line shows engaged
  actuators. [DD-M6-1] GPU time sampled every 10th frame (readback stalls the
  pipe); falls back to CPU EMA when timestamp queries are unavailable.
- **Bloom mip actuator**: second composite bind group sampling `ups[1]`
  (coarser bloom) swapped per frame — no pipeline churn.
- **Corrupt-cache quarantine** (plan §13): `blob_get` verifies sha256-on-read;
  mismatch → file moved to `raw/quarantine/`, index row + logical mapping
  dropped, single transparent re-fetch. Unit test covers corrupt→quarantine→
  recover.
- **Offline / degradation chips**: load failures classified (dns/connect/
  network/tls → "offline — cached data only"; Overpass → vectors-degraded
  chip); chips render in the status bar (max 3), vectors-skip progress
  messages also chip.
- **Antimeridian + polar** (plan §13 edge cases): `plan_region` iterates tile
  longitudes by span with wrap to [-180,180); per-tile DEM windows clamped to
  the tile's 1° span; `merge_grids` replaced by **`GridSet`** (per-cell
  contained-tile sampling) which makes seams and wrap-arounds non-special.
  Tests: antimeridian tile wrap, seam sampling across two synthetic grids,
  polar ENU round-trips (78.2°N, 89.9°N) < 1 µm.
- **Soak harness**: `--soak MIN` idle-orbit loop sampling frame CPU/GPU ms,
  RSS (VmRSS) and drawn points → `bench/soak.csv`; `scripts/soak.sh` wrapper
  (SOAK_MIN/SOAK_PLACE/… env). Sandbox smoke: 18 frames @480p.

### Verification

- Governor live on llvmpipe (guaranteed overload): gpu 136.7 ms → cut chain
  engaged (budget 0.41×, minpx 0.75, bloom-1) → 46.7 ms; label surfaced in HUD.
- Soak smoke: RSS 155.9 → 203.8 MB across warm-up (pool upload + cache pages);
  no monotone growth after steady state in the short run; 8 h reference run
  remains a release-gate item.
- 22/22 tests (quarantine, antimeridian plan, GridSet seam, polar ENU, walk,
  swim bound, SSE, budget, COG, geocode, cache), 0 warnings, verify green.
- `renders/m6_kyoto_final.png` — regression hero (unchanged look ⇒ governor
  idle at 1.0× on single-frame headless, as intended).

### Bugs / notes

- **[BUG-M6-1]** `merge_grids` (M3) was fundamentally seam-fragile (rounding,
  folds, inf extents); deleting it in favour of GridSet removed a whole bug
  class instead of patching it.
- Overpass bbox queries still assume non-wrapped longitudes (vector layers at
  the antimeridian deferred; DEM path is wrap-safe now).

### Known gaps / deferred

- 8 h soak + p99 gate on reference GPU (release gate, plan §14).
- WorldCover vegetation raster never landed (veg = OSM polys only); its
  degradation chip path exists but the provider is M7/backlog.
- SDF billboard labels, measure tool: backlog (M5 carry-over).

### Next session checklist (M7 — packaging & docs)

1. `scripts/bootstrap.sh && scripts/verify.sh`.
2. About dialog: licenses & attribution list (ODbL/CC-BY/CDLA), version,
   adapter string (plan §15).
3. `cargo-wix` MSI recipe + portable ZIP layout (`portable.flag` → ./profile
   settings dir, plan §13/App. C); Windows CI notes in README.
4. User manual markdown (quickstart, controls, data sources, troubleshooting)
   shipped as `docs/manual.md`.
5. Release profile check: `launch.bat release` path, icon/meta (winres).
6. Final golden-render sheet vs all three references + tag v0.7.0-m7.

---

---
*Sessions 1–7 authored by Qwen agentic build · rustc 1.99.0 · wgpu 25.0.2 (llvmpipe GL) · egui 0.32.3 · ureq 2.12 (native-tls)*

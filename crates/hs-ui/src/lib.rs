//! hs-ui — egui HUD in the holo-ops style of reference `ui.jpg`
//! (dark navy panels, monospace type, cyan + signal-red accents).

use egui::{Color32, Frame, RichText};
use hs_forge::layer;

pub const CYAN: Color32 = Color32::from_rgb(0x59, 0xd8, 0xe6);
pub const RED: Color32 = Color32::from_rgb(0xe0, 0x47, 0x3b);
pub const LIME: Color32 = Color32::from_rgb(0xa8, 0xe6, 0x3c);
pub const DIM: Color32 = Color32::from_rgb(0x6b, 0x7a, 0x8a);
pub const TEXT: Color32 = Color32::from_rgb(0xc2, 0xce, 0xda);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(9, 13, 21, 232);
pub const PANEL_EDGE: Color32 = Color32::from_rgb(0x24, 0x34, 0x44);

#[derive(Clone, Debug)]
pub struct UiState {
    pub palette: String,
    pub point_size: f32,
    pub bloom: f32,
    pub exposure: f32,
    pub z_exag: f32,
    pub intensity: f32,
    pub holo: bool,
    pub scanlines: bool,
    pub scan_sweep: bool,
    pub bg_mode: u32,
    pub hardness: f32,
    pub edl_strength: f32,
    pub edl_radius: f32,
    pub fov: f32,
    pub layers: [bool; layer::COUNT],
    pub fps: f32,
    pub frame_ms: f32,
    pub points: u64,
    pub nodes_drawn: u32,
    pub region: String,
    pub adapter: String,
    pub cam_mode: String,
    pub search: String,
    // ---- M1 ingest flow ----
    pub search_results: Vec<(String, String, f64, f64)>, // label, sub, lat, lon
    pub resolved: Option<(String, f64, f64)>,
    pub extent_km: f32,
    pub density_idx: usize,
    pub load_requested: bool,
    pub cancel_requested: bool,
    pub job_active: bool,
    pub job_stage: String,
    pub job_pct: f32,
    pub job_error: String,
    pub attribution: String,
    pub est_points_m: f32,
    pub est_mb: f32,
    pub beacon_count: usize,
    pub clear_beacons: bool,
    pub beacon_list: Vec<(String, f32, f32)>,
    pub jump_beacon: Option<usize>,
    pub cam_sens: f32,
    pub cam_damp: f32,
    pub head_height: f32,
    pub ground_clamp: bool,
    pub fly_dur: f32,
    pub help_open: bool,
    pub perf_open: bool,
    pub frame_hist: Vec<f32>,
    pub vp_count: usize,
    pub ui_scale: f32,
    pub high_contrast: bool,
    pub gov_on: bool,
    pub gov_label: String,
    pub gpu_ema_ms: f32,
    pub point_budget_m: f32,
    pub warn_chips: Vec<String>,
    pub labels_screen: Vec<(String, f32, f32, f32)>,
    pub supersample: u32,
    pub pool_mb: f32,
    pub uploaded_pct: f32,
    pub trav_ms: f32,
    pub lidar_pts: u64,
    pub layer_tint: [[f32; 3]; hs_forge::layer::COUNT],
    pub layer_tint_on: [bool; hs_forge::layer::COUNT],
    pub layer_size: [f32; hs_forge::layer::COUNT],
    pub layer_inten: [f32; hs_forge::layer::COUNT],
    pub tune_idx: usize,
}

impl Default for UiState {
    fn default() -> Self {
        let mut layers = [true; layer::COUNT];
        layers[layer::VEG as usize] = true;
        Self {
            palette: "hologreen".into(),
            point_size: 1.7,
            bloom: 0.5,
            exposure: 0.95,
            z_exag: 1.6,
            intensity: 0.5,
            holo: true,
            scanlines: true,
            scan_sweep: false,
            bg_mode: 0,
            hardness: 0.25,
            edl_strength: 0.6,
            edl_radius: 1.4,
            fov: 40.0,
            layers,
            fps: 0.0,
            frame_ms: 0.0,
            points: 0,
            nodes_drawn: 0,
            region: "demo_valley_4km".into(),
            adapter: "-".into(),
            cam_mode: "fly".into(),
            search: String::new(),
            search_results: Vec::new(),
            resolved: None,
            extent_km: 10.0,
            density_idx: 2,
            load_requested: false,
            cancel_requested: false,
            job_active: false,
            job_stage: String::new(),
            job_pct: 0.0,
            job_error: String::new(),
            attribution: String::new(),
            est_points_m: 0.0,
            est_mb: 0.0,
            beacon_count: 0,
            clear_beacons: false,
            beacon_list: Vec::new(),
            jump_beacon: None,
            cam_sens: 1.0,
            cam_damp: 0.35,
            head_height: 1.7,
            ground_clamp: true,
            fly_dur: 2.5,
            help_open: false,
            perf_open: false,
            frame_hist: Vec::new(),
            vp_count: 0,
            ui_scale: 1.0,
            high_contrast: false,
            gov_on: true,
            gov_label: "auto".into(),
            gpu_ema_ms: 0.0,
            point_budget_m: 20.0,
            warn_chips: Vec::new(),
            labels_screen: Vec::new(),
            supersample: 2,
            pool_mb: 0.0,
            uploaded_pct: 100.0,
            trav_ms: 0.0,
            lidar_pts: 0,
            layer_tint: [[0.5, 0.9, 0.7]; hs_forge::layer::COUNT],
            layer_tint_on: [false; hs_forge::layer::COUNT],
            layer_size: [1.0; hs_forge::layer::COUNT],
            layer_inten: [1.0; hs_forge::layer::COUNT],
            tune_idx: 0,
        }
    }
}

/// High-contrast variant (plan §11.3 accessibility).
pub fn apply_theme_hc(ctx: &egui::Context) {
    apply_theme(ctx);
    let mut v = ctx.style().visuals.clone();
    v.override_text_color = Some(egui::Color32::WHITE);
    v.window_stroke = egui::Stroke::new(2.0_f32, egui::Color32::WHITE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0xdd, 0xdd, 0xdd));
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
    v.widgets.active.fg_stroke = egui::Stroke::new(2.0_f32, egui::Color32::WHITE);
    v.panel_fill = egui::Color32::BLACK;
    ctx.set_visuals(v);
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x05, 0x08, 0x0d);
    v.faint_bg_color = Color32::from_rgb(0x12, 0x1a, 0x26);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(0x14, 0x4a, 0x55);
    v.selection.stroke = egui::Stroke::new(1.0_f32, CYAN);
    let stroke = egui::Stroke::new(1.0_f32, PANEL_EDGE);
    let fg = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = stroke;
    v.widgets.noninteractive.fg_stroke = fg;
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x10, 0x18, 0x24);
    v.widgets.inactive.bg_stroke = stroke;
    v.widgets.inactive.fg_stroke = fg;
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x16, 0x26, 0x36);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, CYAN);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.active.bg_fill = Color32::from_rgb(0x1a, 0x3a, 0x46);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, CYAN);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.window_stroke = stroke;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.override_font_id = Some(egui::FontId::monospace(12.0));
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.slider_width = 108.0;
        s.spacing.combo_width = 108.0;
    });
}

fn panel_frame() -> Frame {
    Frame {
        inner_margin: egui::Margin::same(10),
        outer_margin: egui::Margin::same(0),
        corner_radius: egui::CornerRadius::same(2),
        shadow: egui::epaint::Shadow::NONE,
        fill: PANEL,
        stroke: egui::Stroke::new(1.0_f32, PANEL_EDGE),
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).color(CYAN).strong().size(11.0));
    ui.add_space(2.0);
}

fn slider_row(ui: &mut egui::Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, dec: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(DIM).size(10.5));
        ui.add(egui::Slider::new(value, range).show_value(false));
        ui.label(RichText::new(format!("{:.*}", dec, value)).color(RED).size(10.5));
    });
}

pub fn density_names() -> [&'static str; 4] {
    ["low", "med", "high", "ultra"]
}
pub fn density_name(i: usize) -> &'static str {
    density_names()[i.min(3)]
}
/// preset → vector spacing multiplier (plan-adjacent; user-requested M6.5)
pub fn density_value(i: usize) -> f32 {
    [0.5, 1.0, 2.0, 4.0][i.min(3)]
}

fn bg_name(m: u32) -> &'static str {
    match m {
        1 => "grid",
        2 => "stars",
        3 => "grad",
        _ => "void",
    }
}

pub fn hs_ingest_coords(q: &str) -> Option<(f64, f64)> {
    hs_ingest::geocode::parse_raw_coords(q)
}

pub fn draw(ctx: &egui::Context, s: &mut UiState) {
    // ---- command bar (top, single row — proven layout) ----
    egui::TopBottomPanel::top("cmd")
        .frame(panel_frame())
        .show(ctx, |ui| {
            let (search_resp, pressed_enter) = ui
                .horizontal(|ui| {
                    ui.label(RichText::new("⌕").color(CYAN).size(14.0));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut s.search)
                            .desired_width(200.0)
                            .hint_text(egui::RichText::new("search place / lat, lon").color(DIM)),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.separator();
                    ui.label(RichText::new("EXTENT").color(DIM).size(10.5));
                    ui.add(
                        egui::Slider::new(&mut s.extent_km, 1.0..=30.0)
                            .suffix(" km")
                            .show_value(false),
                    );
                    ui.label(
                        RichText::new(format!("{:.0}×{:.0}", s.extent_km, s.extent_km))
                            .color(RED)
                            .size(10.5),
                    );
                    ui.separator();
                    if s.job_active {
                        if ui.button(RichText::new("CANCEL").color(RED)).clicked() {
                            s.cancel_requested = true;
                        }
                        ui.add(
                            egui::ProgressBar::new(s.job_pct)
                                .desired_width(120.0)
                                .text(s.job_stage.clone()),
                        );
                    } else {
                        let can_load =
                            s.resolved.is_some() || hs_ingest_coords(&s.search).is_some();
                        if ui
                            .add_enabled(can_load, egui::Button::new(RichText::new("LOAD").color(LIME)))
                            .clicked()
                        {
                            s.load_requested = true;
                        }
                        if !s.job_error.is_empty() {
                            ui.label(
                                RichText::new(format!("⚠ {}", s.job_error)).color(RED).size(10.0),
                            );
                        }
                    }
                    ui.label(
                        RichText::new(format!("est {:.0} M pts · ~{:.1} MB", s.est_points_m, s.est_mb))
                            .color(DIM)
                            .size(10.0),
                    );
                    (resp, enter)
                })
                .inner;
            if pressed_enter {
                s.load_requested = true;
            }
            // autocomplete dropdown (overlay, does not affect panel sizing)
            let dropdown_wanted = !s.search_results.is_empty()
                && !s.job_active
                && s.resolved
                    .as_ref()
                    .map(|(l, _, _)| l != &s.search)
                    .unwrap_or(true);
            if dropdown_wanted {
                let items = std::mem::take(&mut s.search_results);
                let mut picked: Option<usize> = None;
                egui::Area::new(egui::Id::new("ac-dropdown"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(search_resp.rect.left_bottom() + egui::vec2(0.0, 2.0))
                    .show(ctx, |ui| {
                        egui::Frame {
                            fill: egui::Color32::from_rgb(0x0b, 0x12, 0x1c),
                            stroke: egui::Stroke::new(1.0_f32, PANEL_EDGE),
                            inner_margin: egui::Margin::same(4),
                            ..Default::default()
                        }
                        .show(ui, |ui| {
                            ui.set_width(340.0);
                            for (i, (label, sub, _, _)) in items.iter().enumerate() {
                                let txt = if sub.is_empty() {
                                    label.clone()
                                } else {
                                    format!("{label}  — {sub}")
                                };
                                if ui.button(RichText::new(txt).size(11.0)).clicked() {
                                    picked = Some(i);
                                }
                            }
                        });
                    });
                if let Some(i) = picked {
                    let (label, _, lat, lon) = items[i].clone();
                    s.search = label.clone();
                    s.resolved = Some((label, lat, lon));
                    s.load_requested = true;
                } else {
                    s.search_results = items;
                }
            }
        });

    // ---- status bar (bottom) ----
    egui::TopBottomPanel::bottom("status")
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("region {}", s.region)).color(LIME).size(10.5));
                ui.separator();
                ui.label(
                    RichText::new(format!("pts {:.2}M drawn", s.points as f64 / 1e6))
                        .color(TEXT)
                        .size(10.5),
                );
                if s.lidar_pts > 0 {
                    ui.label(
                        RichText::new(format!("· lidar {:.2}M", s.lidar_pts as f64 / 1e6))
                            .color(LIME)
                            .size(10.5),
                    );
                }
                ui.separator();
                for chip in s.warn_chips.iter().take(2) {
                    ui.label(RichText::new(format!("⚠ {chip}")).color(RED).size(10.0));
                    ui.separator();
                }
                let attr = if s.attribution.is_empty() {
                    "© synthetic demo region — real geodata attribution appears after LOAD".to_string()
                } else {
                    s.attribution.clone()
                };
                ui.label(RichText::new(attr).color(DIM).size(10.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if s.attribution.is_empty() {
                        ui.label(
                            RichText::new("drag=look · wheel=speed · WASD+QE fly · esc quit")
                                .color(DIM)
                                .size(10.0),
                        );
                    }
                });
            });
        });

    // ---- HUD side panel (left), styled after ui.jpg ----
    egui::SidePanel::left("hud")
        .width_range(250.0..=250.0)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("HOLO|SCOPE").color(CYAN).strong().size(15.0));
                ui.label(RichText::new("v0.2-m1").color(DIM).size(10.0));
            });
            ui.label(RichText::new("rust · wgpu/vulkan · egui").color(DIM).size(9.5));
            ui.separator();

            section(ui, "FOV_ANGLE");
            egui::Frame::default()
                .stroke(egui::Stroke::new(1.0_f32, PANEL_EDGE))
                .fill(Color32::from_rgb(0x0b, 0x12, 0x1c))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("FOV_ANGLE").color(TEXT).size(10.5));
                        ui.add(egui::Slider::new(&mut s.fov, 30.0..=110.0).show_value(false));
                        ui.label(RichText::new(format!("{:.0}", s.fov)).color(RED).size(11.0));
                    });
                });

            section(ui, "CAMERA");
            ui.horizontal(|ui| {
                ui.label(RichText::new("mode").color(DIM).size(10.5));
                egui::ComboBox::from_id_salt("cammode")
                    .selected_text(s.cam_mode.as_str())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.cam_mode, "fly".to_string(), "fly");
                        ui.selectable_value(&mut s.cam_mode, "orbit".to_string(), "orbit");
                        ui.selectable_value(&mut s.cam_mode, "walk".to_string(), "walk");
                    });
                ui.label(RichText::new(format!("vp {}", s.vp_count)).color(DIM).size(9.0));
            });
            slider_row(ui, "sens   ", &mut s.cam_sens, 0.2..=3.0, 2);
            slider_row(ui, "damp   ", &mut s.cam_damp, 0.0..=0.9, 2);
            slider_row(ui, "head h ", &mut s.head_height, 1.2..=2.2, 2);
            slider_row(ui, "fly-to ", &mut s.fly_dur, 0.5..=6.0, 1);
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.ground_clamp, RichText::new("ground clamp").size(10.5));
            });

            section(ui, "LAYERS");
            let names: [(usize, &str); 8] = [
                (layer::TERRAIN as usize, "terrain"),
                (layer::CONTOUR as usize, "contours"),
                (layer::ROAD as usize, "roads"),
                (layer::WATER as usize, "water"),
                (layer::BUILDING as usize, "buildings"),
                (layer::VEG as usize, "vegetation"),
                (layer::FURNITURE as usize, "holo furniture"),
                (layer::IMPORTED as usize, "imported lidar"),
            ];
            for (id, name) in names {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.layers[id], RichText::new(name).size(11.0));
                    // per-layer colour box (user request): edit = override on
                    let mut col = s.layer_tint[id];
                    if egui::color_picker::color_edit_button_rgb(ui, &mut col).changed() {
                        s.layer_tint[id] = col;
                        s.layer_tint_on[id] = true;
                    }
                    if s.layer_tint_on[id]
                        && ui.button(RichText::new("↺").color(DIM).size(9.0)).clicked()
                    {
                        s.layer_tint_on[id] = false;
                        s.layer_tint[id] =
                            hs_forge::palette::layer_default_color(&s.palette, id as u8);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}", id)).color(DIM).size(9.0));
                    });
                });
            }

            section(ui, "STYLE");
            ui.horizontal(|ui| {
                ui.label(RichText::new("palette").color(DIM).size(10.5));
                egui::ComboBox::from_id_salt("palette")
                    .selected_text(s.palette.as_str())
                    .show_ui(ui, |ui| {
                        for p in hs_forge::palette::PALETTES {
                            ui.selectable_value(&mut s.palette, p.to_string(), *p);
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.holo, true, RichText::new("HOLO").color(CYAN));
                ui.selectable_value(&mut s.holo, false, RichText::new("CLARITY").color(DIM));
                ui.checkbox(&mut s.scanlines, RichText::new("scan").size(10.5));
                ui.checkbox(&mut s.scan_sweep, RichText::new("sweep").size(10.5));
            });
            slider_row(ui, "pt_size", &mut s.point_size, 0.5..=6.0, 1);
            slider_row(ui, "bloom  ", &mut s.bloom, 0.0..=2.5, 2);
            slider_row(ui, "expos  ", &mut s.exposure, 0.2..=3.0, 2);
            slider_row(ui, "exagg  ", &mut s.z_exag, 1.0..=4.0, 1);
            slider_row(ui, "intens ", &mut s.intensity, 0.2..=2.5, 2);
            slider_row(ui, "hard   ", &mut s.hardness, 0.0..=1.0, 2);
            ui.horizontal(|ui| {
                ui.label(RichText::new("bg").color(DIM).size(10.5));
                egui::ComboBox::from_id_salt("bgmode")
                    .selected_text(bg_name(s.bg_mode))
                    .show_ui(ui, |ui| {
                        for (i, n) in ["void", "grid", "stars", "grad"].iter().enumerate() {
                            ui.selectable_value(&mut s.bg_mode, i as u32, *n);
                        }
                    });
            });
            if !s.holo {
                slider_row(ui, "edl    ", &mut s.edl_strength, 0.0..=2.0, 2);
                slider_row(ui, "edl_r  ", &mut s.edl_radius, 0.5..=4.0, 1);
            }

            section(ui, "LAYER TUNE");
            let tune_names: [(usize, &str); 7] = [
                (layer::TERRAIN as usize, "terrain"),
                (layer::CONTOUR as usize, "contours"),
                (layer::ROAD as usize, "roads"),
                (layer::WATER as usize, "water"),
                (layer::BUILDING as usize, "buildings"),
                (layer::VEG as usize, "vegetation"),
                (layer::FURNITURE as usize, "furniture"),
            ];
            ui.horizontal(|ui| {
                ui.label(RichText::new("layer").color(DIM).size(10.5));
                egui::ComboBox::from_id_salt("tune-layer")
                    .selected_text(tune_names[s.tune_idx.min(6)].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, name)) in tune_names.iter().enumerate() {
                            ui.selectable_value(&mut s.tune_idx, i, *name);
                        }
                    });
            });
            let tid = tune_names[s.tune_idx.min(6)].0;
            slider_row(ui, "l_size ", &mut s.layer_size[tid], 0.2..=3.0, 2);
            slider_row(ui, "l_inten", &mut s.layer_inten[tid], 0.0..=3.0, 2);

            section(ui, "BEACONS");
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("placed {}", s.beacon_count)).color(LIME).size(10.5));
                if ui.button(RichText::new("clear").color(RED).size(10.5)).clicked() {
                    s.clear_beacons = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("ctrl+click").color(DIM).size(9.0));
                });
            });
            for (i, (name, bx, by)) in s.beacon_list.iter().enumerate().take(6) {
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(format!("▶ {name}")).color(CYAN).size(10.0)).clicked() {
                        s.jump_beacon = Some(i);
                    }
                    ui.label(RichText::new(format!("({bx:.0},{by:.0})")).color(DIM).size(9.0));
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("shot ×").color(DIM).size(10.5));
                egui::ComboBox::from_id_salt("ss")
                    .selected_text(format!("{}", s.supersample))
                    .show_ui(ui, |ui| {
                        for v in [1u32, 2, 3, 4] {
                            ui.selectable_value(&mut s.supersample, v, format!("{v}"));
                        }
                    });
                ui.label(RichText::new("F9").color(DIM).size(9.0));
            });

            section(ui, "A11Y");
            slider_row(ui, "ui ×   ", &mut s.ui_scale, 0.75..=2.0, 2);
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.high_contrast, RichText::new("high contrast").size(10.5));
                ui.label(RichText::new("F1 help · F3 perf").color(DIM).size(9.0));
            });

            section(ui, "PERF GOVERNOR");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.gov_on, RichText::new("auto 60fps").size(10.5));
                ui.label(RichText::new(format!("budget {:.0}M", s.point_budget_m)).color(DIM).size(9.5));
            });
            if !s.gov_on {
                slider_row(ui, "pts M  ", &mut s.point_budget_m, 4.0..=40.0, 0);
            }
            ui.label(RichText::new(format!("gpu ema {:.1}ms", s.gpu_ema_ms)).color(TEXT).size(10.0));
            ui.label(RichText::new(s.gov_label.clone()).color(LIME).size(9.5));

            section(ui, "PERF");
            egui::Frame::default()
                .stroke(egui::Stroke::new(1.0_f32, PANEL_EDGE))
                .fill(Color32::from_rgb(0x0b, 0x12, 0x1c))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("FPS   {:>7.1}", s.fps)).color(LIME).size(10.5));
                    ui.label(RichText::new(format!("FRAME {:>6.2}ms", s.frame_ms)).color(TEXT).size(10.5));
                    ui.label(RichText::new(format!("PTS   {:>5.1}M", s.points as f64 / 1e6)).color(TEXT).size(10.5));
                    ui.label(RichText::new(format!("NODES {:>7}", s.nodes_drawn)).color(TEXT).size(10.5));
                    ui.label(
                        RichText::new(format!("POOL  {:>5.1}MB {:>3.0}%", s.pool_mb, s.uploaded_pct))
                            .color(if s.uploaded_pct < 100.0 { LIME } else { TEXT })
                            .size(10.5),
                    );
                    ui.label(RichText::new(format!("LOD   {:>6.2}ms", s.trav_ms)).color(TEXT).size(10.5));
                    ui.label(RichText::new(format!("GPU   {}", s.adapter)).color(DIM).size(9.0));
                });
        });

    // ---- F1 help overlay ----
    if s.help_open {
        egui::Window::new(RichText::new("HELP — key map").color(CYAN).strong())
            .default_pos([380.0, 140.0])
            .frame(panel_frame())
            .show(ctx, |ui| {
                ui.label(RichText::new("WASD move · Q/E down/up · Shift boost").size(11.0));
                ui.label(RichText::new("drag = look (fly/walk) · RMB drag = pan (orbit)").size(11.0));
                ui.label(RichText::new("wheel = speed / dolly · ctrl+click = place beacon").size(11.0));
                ui.label(RichText::new("F1 help · F3 perf graph · F9 supersample shot").size(11.0));
                ui.label(RichText::new("Ctrl+S save viewpoint · Ctrl+O cycle viewpoints").size(11.0));
                ui.label(RichText::new("Esc quit · 1-7 toggle layers via LAYERS panel").size(11.0));
                if ui.button("close").clicked() {
                    s.help_open = false;
                }
            });
    }
    // ---- F3 perf sparkline ----
    if s.perf_open {
        egui::Area::new(egui::Id::new("perf-graph"))
            .order(egui::Order::Foreground)
            .fixed_pos([ctx.screen_rect().right() - 260.0, ctx.screen_rect().bottom() - 120.0])
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_rgba_premultiplied(9, 13, 21, 220))
                    .stroke(egui::Stroke::new(1.0_f32, PANEL_EDGE))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.label(RichText::new("frame ms (16.6 target)").color(DIM).size(9.0));
                        let (resp, painter) = ui.allocate_painter(egui::vec2(230.0, 70.0), egui::Sense::hover());
                        let rect = resp.rect;
                        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, PANEL_EDGE), egui::StrokeKind::Inside);
                        let target_y = rect.max.y - (16.6 / 50.0) * rect.height();
                        painter.line_segment(
                            [egui::pos2(rect.left(), target_y), egui::pos2(rect.right(), target_y)],
                            egui::Stroke::new(1.0_f32, RED),
                        );
                        let n = s.frame_hist.len();
                        if n >= 2 {
                            let pts: Vec<egui::Pos2> = s
                                .frame_hist
                                .iter()
                                .enumerate()
                                .map(|(i, ms)| {
                                    let x = rect.left() + (i as f32 / 119.0) * rect.width();
                                    let y = rect.max.y - (ms.min(50.0) / 50.0) * rect.height();
                                    egui::pos2(x, y)
                                })
                                .collect();
                            painter.add(egui::Shape::line(pts, egui::Stroke::new(1.0_f32, LIME)));
                        }
                        let _ = n;
                    });
            });
    }

    // ---- viewport: fully transparent so the 3D pass shows through ----
    egui::CentralPanel::default()
        .frame(egui::Frame {
            inner_margin: egui::Margin::same(0),
            outer_margin: egui::Margin::same(0),
            corner_radius: egui::CornerRadius::same(0),
            shadow: egui::epaint::Shadow::NONE,
            fill: Color32::TRANSPARENT,
            stroke: egui::Stroke::NONE,
        })
        .show(ctx, |ui| {
            // screen-space place labels (M4; GPU SDF billboards remain backlog)
            let painter = ui.painter();
            for (name, x, y, fade) in s.labels_screen.clone() {
                let pos = egui::pos2(x, y);
                let a = (235.0 * fade) as u8;
                // dark halo pass for legibility over the glow, then bright pass
                for dx in [-1.0f32, 0.0, 1.0] {
                    for dy in [-1.0, 0.0, 1.0] {
                        if dx == 0.0 && dy == 0.0 {
                            continue;
                        }
                        painter.text(
                            pos + egui::vec2(dx, dy),
                            egui::Align2::CENTER_BOTTOM,
                            name.clone(),
                            egui::FontId::monospace(11.0),
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, a / 2),
                        );
                    }
                }
                painter.text(
                    pos,
                    egui::Align2::CENTER_BOTTOM,
                    name.clone(),
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgba_unmultiplied((0.55 * 255.0) as u8, 255, (0.85 * 255.0) as u8, a),
                );
                painter.circle_filled(pos + egui::vec2(0.0, 3.0), 2.0, egui::Color32::from_rgba_unmultiplied((0.8 * 255.0) as u8, 255, (0.2 * 255.0) as u8, a));
            }
        });
}

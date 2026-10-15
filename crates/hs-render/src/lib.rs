//! hs-render — wgpu device management, pipelines, render graph (plan §8, M0 slice).
//!
//! Frame graph (M0):
//!   [0] PointPass (instanced quads, additive / premul-alpha)  → HDR R16F
//!   [0b] LinePass + QuadPass (cage, outline, beams, diamonds)  → HDR
//!   [1] BloomDown ×4 → BloomUp ×4 (dual-filter-ish)
//!   [2] Composite (HDR + bloom → ACES → vignette/scanlines)   → sRGB target
//!   [3] UiPass (egui, painted by the app on the same target)
//!
//! Deferred to M3/M4: depth/EDL clarity path, GPU timers, slab pool.

use anyhow::{Context, Result};
use hs_camera::Camera;
use hs_forge::record::{Globals, ImplVertex, NodeU, PostU};
use hs_forge::{layer, Region as _R};
use hs_lod::{LodNode, LodRegion, Sel};
use std::mem::size_of;
use wgpu::util::DeviceExt;

pub const BLOOM_MIPS: usize = 4;

/// Adapter selection (remote/GPU-less testing, user request):
/// `software` prefers known software rasterizers; `name` is a case-insensitive
/// substring filter; without either, a failed hardware request automatically
/// retries with wgpu's fallback (software) adapter.
#[derive(Clone, Debug, Default)]
pub struct AdapterPrefs {
    pub software: bool,
    pub name: Option<String>,
}

const SOFTWARE_ADAPTER_HINTS: &[&str] = &[
    "llvmpipe",
    "lavapipe",
    "swiftshader",
    "softpipe",
    "software",
    "basic render",
    "warp",
];

#[derive(Clone, Debug)]
pub struct Settings {
    pub point_size_px: f32,
    pub min_px: f32,
    pub max_px: f32,
    pub bloom: f32,
    pub exposure: f32,
    pub intensity: f32,
    pub emissive: f32,
    pub z_exag: f32,
    pub holo: bool,
    pub scanlines: bool,
    pub vignette: f32,
    pub scan_sweep: bool,
    pub hardness: f32,
    pub bloom_skip: u32, // governor actuator: sample bloom one mip coarser
    pub edl_strength: f32,
    pub edl_radius: f32,
    pub bg_mode: u32, // 0 void, 1 grid floor, 2 starfield, 3 gradient
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            point_size_px: 1.7,
            min_px: 0.75,
            max_px: 26.0,
            bloom: 0.5,
            exposure: 0.95,
            intensity: 0.5,
            emissive: 1.6,
            z_exag: 1.6,
            holo: true,
            scanlines: true,
            vignette: 0.85,
            scan_sweep: false,
            hardness: 0.25,
            bloom_skip: 0,
            edl_strength: 0.6,
            edl_radius: 1.4,
            bg_mode: 0,
        }
    }
}

/// Per-layer size/intensity multipliers (FR-06), applied on top of node bases.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerTune {
    pub size_mul: [f32; hs_forge::layer::COUNT],
    pub intensity: [f32; hs_forge::layer::COUNT],
}

impl Default for LayerTune {
    fn default() -> Self {
        Self {
            size_mul: [1.0; hs_forge::layer::COUNT],
            intensity: [1.0; hs_forge::layer::COUNT],
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub points_drawn: u64,
    pub nodes_drawn: u32,
}

struct Tex {
    #[allow(dead_code)]
    tex: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    pub adapter_name: String,
    pub format: wgpu::TextureFormat,
    palette_name: String,
    pub size: (u32, u32),

    pt_holo: wgpu::RenderPipeline,
    pt_clarity: wgpu::RenderPipeline,
    ln_pipe: wgpu::RenderPipeline,
    qd_pipe: wgpu::RenderPipeline,
    down_pipe: wgpu::RenderPipeline,
    up_pipe: wgpu::RenderPipeline,
    comp_pipe: wgpu::RenderPipeline,

    bgl_scene: wgpu::BindGroupLayout,
    bgl_post: wgpu::BindGroupLayout,

    globals_buf: wgpu::Buffer,
    post_buf: wgpu::Buffer,
    inst_buf: Option<wgpu::Buffer>,
    nodes_buf: Option<wgpu::Buffer>,
    pool_total: u64,
    pool_uploaded: u64,
    sweep_size: f32,
    lod_nodes: Vec<LodNode>,
    base_node_u: Vec<NodeU>,
    lut_buf: wgpu::Buffer,
    lines_buf: Option<wgpu::Buffer>,
    quads_buf: Option<wgpu::Buffer>,
    road_quad_buf: Option<wgpu::Buffer>,
    n_road_quads: u32,
    beacon_buf: Option<wgpu::Buffer>,
    n_beacons: u32,
    near_buf: Option<wgpu::Buffer>,
    near_count: u32,
    near_id: u32,
    near_origin: [f32; 4],
    near_q: f32,
    idx_buf: wgpu::Buffer,

    scene_bg: wgpu::BindGroup,
    node_ranges: Vec<(u32, u32)>,
    node_layers: Vec<u8>,
    last_tune: LayerTune,
    n_lines: u32,
    n_quads: u32,

    hdr: Option<Tex>,
    depth: Option<Tex>,
    grid_buf: Option<wgpu::Buffer>,
    n_grid: u32,
    downs: Vec<Tex>,
    ups: Vec<Tex>,
    down_bgs: Vec<wgpu::BindGroup>,
    up_bgs: Vec<wgpu::BindGroup>,
    comp_bg: Option<wgpu::BindGroup>,
    comp_bg_coarse: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    dummy_tex: Tex,

    pub egui_renderer: egui_wgpu::Renderer,
    last_stats: FrameStats,
    ts_query: Option<wgpu::QuerySet>,
    ts_buf: Option<wgpu::Buffer>,
    ts_read: Option<wgpu::Buffer>,
    pub ts_supported: bool,
}

const HDR_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn additive() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn premul_alpha() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

impl Renderer {
    async fn pick_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        prefs: &AdapterPrefs,
    ) -> Result<wgpu::Adapter> {
        let mut cands = instance.enumerate_adapters(wgpu::Backends::all());
        // explicit name filter wins
        if let Some(f) = &prefs.name {
            let f = f.to_lowercase();
            cands.retain(|a| a.get_info().name.to_lowercase().contains(&f));
            if let Some(a) = cands.into_iter().next() {
                eprintln!("[gpu] adapter chosen by name filter: {}", a.get_info().name);
                return Ok(a);
            }
            anyhow::bail!("no adapter matching name filter '{f}'");
        }
        if prefs.software {
            if let Some(a) = cands.iter().find(|a| {
                let n = a.get_info().name.to_lowercase();
                SOFTWARE_ADAPTER_HINTS.iter().any(|h| n.contains(h))
            }) {
                eprintln!("[gpu] software adapter: {}", a.get_info().name);
                return Ok(a.clone());
            }
            eprintln!("[gpu] no named software adapter found; using wgpu fallback adapter");
        }
        match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface,
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: prefs.software,
            })
            .await
        {
            Ok(a) => Ok(a),
            Err(e) => {
                // remote/GPU-less boxes: retry once with the software fallback
                eprintln!("[gpu] hardware adapter unavailable ({e}); retrying with software fallback");
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        compatible_surface: surface,
                        power_preference: wgpu::PowerPreference::LowPower,
                        force_fallback_adapter: true,
                    })
                    .await
                    .context("no GPU adapter and no software fallback available")
            }
        }
    }

    pub fn create_instance() -> wgpu::Instance {
        wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        })
    }

    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        fallback_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        Self::new_with_prefs(instance, surface, fallback_format, &AdapterPrefs::default()).await
    }

    pub async fn new_with_prefs(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        fallback_format: wgpu::TextureFormat,
        prefs: &AdapterPrefs,
    ) -> Result<Self> {
        let adapter = Self::pick_adapter(instance, surface, prefs).await?;
        let format = match surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                caps.formats
                    .iter()
                    .find(|f| f.is_srgb())
                    .copied()
                    .unwrap_or_else(|| caps.formats.first().copied().unwrap_or(fallback_format))
            }
            None => fallback_format,
        };
        let adapter_name = adapter.get_info().name.clone();
        // wgpu 25 renamed the feature to TIMESTAMP_QUERY; software GL lacks it
        let ts_supported = adapter
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("holoscope-device"),
                required_features: if ts_supported {
                    wgpu::Features::TIMESTAMP_QUERY
                } else {
                    wgpu::Features::empty()
                },
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await?;

        let bgl_scene = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl-scene"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<NodeU>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });

        let bgl_post = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl-post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<PostU>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let sm_points = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("points.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/points.wgsl").into()),
        });
        let sm_lines = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lines.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/lines.wgsl").into()),
        });
        let sm_post = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/post.wgsl").into()),
        });

        let pl_scene = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl-scene"),
            bind_group_layouts: &[&bgl_scene],
            push_constant_ranges: &[],
        });
        let pl_post = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl-post"),
            bind_group_layouts: &[&bgl_post],
            push_constant_ranges: &[],
        });

        let inst_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<ImplVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Sint16x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint8x4,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 12,
                    shader_location: 2,
                },
            ],
        };

        let mk_points_depth = |blend: wgpu::BlendState, depth: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("points"),
                layout: Some(&pl_scene),
                vertex: wgpu::VertexState {
                    module: &sm_points,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[inst_layout.clone()],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: depth,
                    depth_compare: if depth {
                        wgpu::CompareFunction::Less
                    } else {
                        wgpu::CompareFunction::Always
                    },
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sm_points,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FMT,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let mk_points = |blend: wgpu::BlendState| mk_points_depth(blend, false);
        let pt_holo = mk_points(additive());
        let pt_clarity = mk_points_depth(premul_alpha(), true);

        let ln_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<hs_forge::LineVert>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 12,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 28,
                    shader_location: 2,
                },
            ],
        };
        let mk_lines = |topo: wgpu::PrimitiveTopology, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl_scene),
                vertex: wgpu::VertexState {
                    module: &sm_lines,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[ln_layout.clone()],
                },
                primitive: wgpu::PrimitiveState {
                    topology: topo,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Always,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sm_lines,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FMT,
                        blend: Some(additive()),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let ln_pipe = mk_lines(wgpu::PrimitiveTopology::LineList, "lines");
        let qd_pipe = mk_lines(wgpu::PrimitiveTopology::TriangleList, "quads");

        let mk_post = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl_post),
                vertex: wgpu::VertexState {
                    module: &sm_post,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sm_post,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FMT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let down_pipe = mk_post("fs_down");
        let up_pipe = mk_post("fs_up");

        let comp_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&pl_post),
            vertex: wgpu::VertexState {
                module: &sm_post,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &sm_post,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: size_of::<PostU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lut_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lut"),
            size: (256 * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let idx_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad-idx"),
            contents: bytemuck::cast_slice(&[0u16, 1, 2, 0, 2, 3]),
            usage: wgpu::BufferUsages::INDEX,
        });

        // placeholder scene bind group (buffers replaced on upload_region)
        let dummy_nodes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nodes-dummy"),
            contents: bytemuck::cast_slice(&[NodeU::default()]),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let dummy_inst = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("inst-dummy"),
            contents: bytemuck::cast_slice(&[ImplVertex::default()]),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let scene_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene-bg"),
            layout: &bgl_scene,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: dummy_nodes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: lut_buf.as_entire_binding() },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let (ts_query, ts_buf, ts_read) = if ts_supported {
            (
                Some(device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("gpu-ts"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                })),
                Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu-ts-resolve"),
                    size: 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })),
                Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu-ts-read"),
                    size: 16,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })),
            )
        } else {
            (None, None, None)
        };

        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FMT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_tex = Tex {
            view: dummy.create_view(&wgpu::TextureViewDescriptor::default()),
            tex: dummy,
        };

        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

        Ok(Self {
            device,
            queue,
            adapter: adapter.clone(),
            adapter_name,
            palette_name: "hologreen".into(),
            format,
            size: (0, 0),
            pt_holo,
            pt_clarity,
            ln_pipe,
            qd_pipe,
            down_pipe,
            up_pipe,
            comp_pipe,
            bgl_scene,
            bgl_post,
            globals_buf,
            post_buf,
            inst_buf: Some(dummy_inst),
            nodes_buf: Some(dummy_nodes),
            pool_total: 0,
            pool_uploaded: 0,
            sweep_size: 4000.0,
            lod_nodes: Vec::new(),
            base_node_u: Vec::new(),
            lut_buf,
            lines_buf: None,
            quads_buf: None,
            road_quad_buf: None,
            n_road_quads: 0,
            beacon_buf: None,
            n_beacons: 0,
            near_buf: None,
            near_count: 0,
            near_id: 0,
            near_origin: [0.0; 4],
            near_q: 0.01,
            idx_buf,
            scene_bg,
            node_ranges: Vec::new(),
            node_layers: Vec::new(),
            last_tune: LayerTune::default(),
            n_lines: 0,
            n_quads: 0,
            hdr: None,
            depth: None,
            grid_buf: None,
            n_grid: 0,
            downs: Vec::new(),
            ups: Vec::new(),
            down_bgs: Vec::new(),
            up_bgs: Vec::new(),
            comp_bg: None,
            comp_bg_coarse: None,
            sampler,
            dummy_tex,
            egui_renderer,
            last_stats: FrameStats::default(),
            ts_query,
            ts_read,
            ts_buf,
            ts_supported,
        })
    }

    pub fn set_palette(&mut self, name: &str) {
        self.palette_name = name.to_string();
        let lut = hs_forge::palette::build_lut(name);
        self.queue.write_buffer(&self.lut_buf, 0, bytemuck::cast_slice(&lut));
    }

    /// Per-layer colour overrides (user request): rebuild the LUT with tints.
    pub fn set_layer_tints(
        &mut self,
        tints: &[[f32; 3]; hs_forge::layer::COUNT],
        on: &[bool; hs_forge::layer::COUNT],
    ) {
        let mut lut = hs_forge::palette::build_lut(&self.palette_name);
        hs_forge::palette::apply_tints(&mut lut, tints, on);
        self.queue.write_buffer(&self.lut_buf, 0, bytemuck::cast_slice(&lut));
    }

    /// Upload the LOD instance pool (created empty-filled; bytes stream in
    /// via `pump_upload` coarse-first, ≤48 MB/frame — plan §7.3/§7.4).
    pub fn upload_lod(&mut self, lod: &LodRegion) {
        let pool = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance-pool"),
            size: lod.pool_bytes.max(16),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut base = Vec::with_capacity(lod.nodes.len());
        for n in &lod.nodes {
            base.push(NodeU {
                origin_view: n.origin,
                quant_scale: n.quant_scale,
                fade_near: n.fade_near,
                fade_far: n.fade_far,
                size_mul: n.size_mul,
                tier_fade: 0.0,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            });
        }
        let nodes_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nodes-u"),
            size: (base.len() + 1).max(2) as u64 * size_of::<NodeU>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.scene_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene-bg"),
            layout: &self.bgl_scene,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: nodes_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lut_buf.as_entire_binding() },
            ],
        });
        self.inst_buf = Some(pool);
        self.nodes_buf = Some(nodes_buf);
        self.base_node_u = base;
        self.lod_nodes = lod.nodes.clone();
        self.pool_total = lod.pool_bytes;
        self.pool_uploaded = 0;
        self.sweep_size = lod
            .nodes
            .iter()
            .map(|n| n.aabb_max[0])
            .fold(f32::MIN, f32::max)
            .max(100.0);
        self.node_layers = lod.nodes.iter().map(|n| n.layer).collect();
        self.node_ranges = lod
            .nodes
            .iter()
            .map(|n| (n.inst_start, n.inst_start + n.inst_count))
            .collect();
        self.n_lines = 0;
        self.n_quads = 0;
        self.near_id = lod.nodes.len() as u32;
        self.near_buf = None;
        self.near_count = 0;
        // grid-floor lines for background mode 1
        let size = self.sweep_size * 2.2;
        let zmin = lod
            .nodes
            .iter()
            .map(|n| n.aabb_min[2])
            .fold(f32::INFINITY, f32::min);
        let z = if zmin.is_finite() { zmin - 4.0 } else { -100.0 };
        let step = size / 24.0;
        let col = [0.05, 0.35, 0.40, 0.30];
        let mut gl: Vec<hs_forge::LineVert> = Vec::new();
        let mut x = -size * 0.5;
        while x <= size * 0.5 {
            gl.push(hs_forge::LineVert { pos: [x, -size * 0.5, z], col, slot: 80 });
            gl.push(hs_forge::LineVert { pos: [x, size * 0.5, z], col, slot: 80 });
            gl.push(hs_forge::LineVert { pos: [-size * 0.5, x, z], col, slot: 80 });
            gl.push(hs_forge::LineVert { pos: [size * 0.5, x, z], col, slot: 80 });
            x += step;
        }
        let gl_bytes = bytemuck::cast_slice(&gl);
        let gl_bytes: &[u8] = if gl_bytes.is_empty() { &[0u8; 16][..] } else { gl_bytes };
        self.grid_buf = Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grid-floor"),
            contents: gl_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        }));
        self.n_grid = gl.len() as u32;
    }

    /// Dense near-field patch (plan §1.2 "dense up close"): a dynamic node
    /// regenerated around the camera when it moves > ~15 m.
    pub fn upload_nearfield(&mut self, verts: &[ImplVertex], origin: [f32; 3], quant: f32) {
        if verts.is_empty() {
            self.near_buf = None;
            self.near_count = 0;
            return;
        }
        self.near_buf = Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nearfield"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        }));
        self.near_count = verts.len() as u32;
        self.near_origin = [origin[0], origin[1], origin[2], 0.0];
        self.near_q = quant;
    }

    /// Replace the dynamic beacon beam/diamond quads (M4 ray-pick placement).
    pub fn upload_beacons(&mut self, quads: &[hs_forge::QuadVert]) {
        if quads.is_empty() {
            self.beacon_buf = None;
            self.n_beacons = 0;
            return;
        }
        self.beacon_buf = Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("beacons"),
            contents: bytemuck::cast_slice(quads),
            usage: wgpu::BufferUsages::VERTEX,
        })); // non-empty guaranteed above
        self.n_beacons = quads.len() as u32;
    }

    /// Lines/quads (holo furniture) are uploaded separately from the pool.
    /// Empty lists still get a 16-byte buffer: wgpu panics on slicing a
    /// zero-size buffer (BUG-M6.5-1, Windows crash after region load).
    pub fn upload_furniture(&mut self, region: &_R) {
        let mk = |label: &str, bytes: &[u8]| {
            let contents = if bytes.is_empty() { &[0u8; 16][..] } else { bytes };
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let lines_buf = mk("lines", bytemuck::cast_slice(&region.lines));
        let quads_buf = mk("quads", bytemuck::cast_slice(&region.quads));
        let road_buf = mk("road-ribbons", bytemuck::cast_slice(&region.road_quads));
        eprintln!(
            "[gpu-buf] lines={} quads={} road_quads={} verts",
            region.lines.len(),
            region.quads.len(),
            region.road_quads.len()
        );
        self.lines_buf = Some(lines_buf);
        self.quads_buf = Some(quads_buf);
        self.n_lines = region.lines.len() as u32;
        self.n_quads = region.quads.len() as u32;
        self.road_quad_buf = Some(road_buf);
        self.n_road_quads = region.road_quads.len() as u32;
    }

    /// Stream ≤ `limit` bytes of the pool to the GPU. Returns true when done.
    pub fn pump_upload(&mut self, limit: u64) -> bool {
        if self.pool_uploaded >= self.pool_total {
            return true;
        }
        let Some(pool) = &self.inst_buf else {
            return true;
        };
        let remaining = self.pool_total - self.pool_uploaded;
        let chunk = remaining.min(limit);
        // borrow-safe slice copy: instances live in the LodRegion on the app
        // side; the app passes them via upload_pool_bytes instead.
        let _ = (pool, chunk);
        true
    }

    /// App-driven pump: copies `bytes[off..off+len]` into the pool at `off`.
    pub fn write_pool(&mut self, off: u64, bytes: &[u8]) {
        if let Some(pool) = &self.inst_buf {
            self.queue.write_buffer(pool, off, bytes);
            self.pool_uploaded = self.pool_uploaded.max(off + bytes.len() as u64);
        }
    }

    pub fn near_node_id(&self) -> u32 {
        self.near_id
    }

    pub fn pool_progress(&self) -> f32 {
        if self.pool_total == 0 {
            1.0
        } else {
            self.pool_uploaded as f32 / self.pool_total as f32
        }
    }

    pub fn node_uploaded(&self, ni: u32) -> bool {
        match self.lod_nodes.get(ni as usize) {
            Some(n) => (n.inst_start as u64) * 16 < self.pool_uploaded,
            None => false,
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 || (w, h) == self.size {
            return;
        }
        self.size = (w, h);
        let mk = |mw: u32, mh: u32, fmt: wgpu::TextureFormat| -> Tex {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rt"),
                size: wgpu::Extent3d {
                    width: mw.max(1),
                    height: mh.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            Tex { tex, view }
        };
        self.hdr = Some(mk(w, h, HDR_FMT));
        let dtex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.depth = Some(Tex {
            view: dtex.create_view(&wgpu::TextureViewDescriptor::default()),
            tex: dtex,
        });
        self.downs.clear();
        self.ups.clear();
        for i in 0..BLOOM_MIPS {
            let s = 1 << (i + 1);
            self.downs.push(mk(w / s as u32, h / s as u32, HDR_FMT));
            self.ups.push(mk(w / s as u32, h / s as u32, HDR_FMT));
        }
        fn tentry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        self.down_bgs.clear();
        for i in 0..BLOOM_MIPS {
            let src = if i == 0 {
                &self.hdr.as_ref().unwrap().view
            } else {
                &self.downs[i - 1].view
            };
            self.down_bgs.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("down-bg"),
                layout: &self.bgl_post,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.post_buf.as_entire_binding() },
                    tentry(1, src),
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    tentry(3, &self.dummy_tex.view),
                    tentry(4, &self.depth.as_ref().unwrap().view),
                ],
            }));
        }
        self.up_bgs.clear();
        for i in 0..BLOOM_MIPS {
            let a = if i == BLOOM_MIPS - 1 {
                &self.downs[BLOOM_MIPS - 1].view
            } else {
                &self.ups[i + 1].view
            };
            self.up_bgs.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("up-bg"),
                layout: &self.bgl_post,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.post_buf.as_entire_binding() },
                    tentry(1, a),
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    tentry(3, &self.downs[i].view),
                    tentry(4, &self.depth.as_ref().unwrap().view),
                ],
            }));
        }
        let mk_comp = |bloom_view: &wgpu::TextureView| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("comp-bg"),
                layout: &self.bgl_post,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.post_buf.as_entire_binding() },
                    tentry(1, &self.hdr.as_ref().unwrap().view),
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    tentry(3, bloom_view),
                    tentry(4, &self.depth.as_ref().unwrap().view),
                ],
            })
        };
        self.comp_bg = Some(mk_comp(&self.ups[0].view));
        self.comp_bg_coarse = Some(mk_comp(&self.ups[1].view));
    }

    /// Encode the full 3D + post graph for one frame onto `target`.
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        camera: &Camera,
        s: &Settings,
        time: f32,
        layer_vis: &[bool; layer::COUNT],
        tune: &LayerTune,
        sels: &[Sel],
    ) -> FrameStats {
        if tune != &self.last_tune && !self.base_node_u.is_empty() {
            let tuned: Vec<NodeU> = self
                .base_node_u
                .iter()
                .enumerate()
                .map(|(i, nu)| {
                    let l = self.node_layers[i] as usize;
                    let mut t = *nu;
                    t.size_mul = nu.size_mul * tune.size_mul[l];
                    t.tier_fade = nu.tier_fade * tune.intensity[l];
                    t
                })
                .collect();
            if let Some(nb) = &self.nodes_buf {
                self.queue.write_buffer(nb, 0, bytemuck::cast_slice(&tuned));
            }
            self.last_tune = tune.clone();
        }
        let (w, h) = self.size;
        if w == 0 || self.hdr.is_none() || self.inst_buf.is_none() {
            return FrameStats::default();
        }
        let aspect = w as f32 / h as f32;
        let g = Globals {
            view_proj: camera.view_proj(aspect).to_cols_array_2d(),
            view: camera.view().to_cols_array_2d(),
            focal_px: (h as f32 * 0.5) / (camera.fov_deg.to_radians() * 0.5).tan(),
            time,
            point_size_px: s.point_size_px,
            min_px: s.min_px,
            max_px: s.max_px,
            pixel_to_clip: 2.0 / h as f32,
            intensity_mul: s.intensity,
            emissive_mul: s.emissive,
            z_exag: s.z_exag,
            sweep_pos: if s.scan_sweep {
                let span = self.sweep_size * 1.6;
                (time * self.sweep_size * 0.12) % span - span * 0.5
            } else {
                -1e9
            },
            sweep_on: if s.scan_sweep { 1.0 } else { 0.0 },
            hardness: s.hardness,
        };
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&g));
        let pu = PostU {
            bloom: if s.holo { s.bloom } else { s.bloom * 0.25 },
            exposure: s.exposure,
            time,
            scanlines: if s.scanlines { 1.0 } else { 0.0 },
            vignette: s.vignette,
            _pad0: 0.0,
            res: [w as f32, h as f32],
            edl_on: if s.holo { 0.0 } else { 1.0 },
            edl_strength: s.edl_strength,
            edl_radius: s.edl_radius,
            bg_mode: s.bg_mode as f32,
        };
        self.queue.write_buffer(&self.post_buf, 0, bytemuck::bytes_of(&pu));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // ---- [0] scene → HDR ----
        {
            let ts_writes = self.ts_query.as_ref().map(|qs| wgpu::RenderPassTimestampWrites {
                query_set: qs,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            });
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr.as_ref().unwrap().view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth.as_ref().unwrap().view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts_writes,
                occlusion_query_set: None,
            });
            // per-frame node uniforms: fade from selection, tune multipliers
            if !self.base_node_u.is_empty() {
                let mut nus = self.base_node_u.clone();
                for (i, nu) in nus.iter_mut().enumerate() {
                    nu.tier_fade = 0.0;
                    let l = self.node_layers[i] as usize;
                    nu.size_mul = self.base_node_u[i].size_mul * tune.size_mul[l];
                }
                for sel in sels {
                    if let Some(nu) = nus.get_mut(sel.node as usize) {
                        let l = self.node_layers[sel.node as usize] as usize;
                        nu.tier_fade = sel.fade * tune.intensity[l];
                    }
                }
                // near-field node entry (id = base count)
                let mut nf = NodeU {
                    origin_view: self.near_origin,
                    quant_scale: self.near_q,
                    fade_near: 300.0,
                    fade_far: 90.0, // fade OUT beyond 90 m: the patch is local
                    size_mul: 1.0,
                    tier_fade: if self.near_count > 0 {
                        tune.intensity[layer::TERRAIN as usize]
                    } else {
                        0.0
                    },
                    _pad0: 0.0,
                    _pad1: 0.0,
                    _pad2: 0.0,
                };
                // note: fade_near<fade_far inverts smoothstep => fade with distance
                std::mem::swap(&mut nf.fade_near, &mut nf.fade_far);
                nus.push(nf);
                if let Some(nb) = &self.nodes_buf {
                    self.queue.write_buffer(nb, 0, bytemuck::cast_slice(&nus));
                }
            }
            rp.set_bind_group(0, &self.scene_bg, &[]);
            let pt = if s.holo { &self.pt_holo } else { &self.pt_clarity };
            rp.set_pipeline(pt);
            let pool = self.inst_buf.as_ref().unwrap();
            if pool.size() > 0 {
                rp.set_vertex_buffer(0, pool.slice(..));
            }
            rp.set_index_buffer(self.idx_buf.slice(..), wgpu::IndexFormat::Uint16);
            let mut stats = FrameStats::default();
            for sel in sels {
                let ni = sel.node as usize;
                let Some(n) = self.lod_nodes.get(ni) else {
                    continue;
                };
                if !layer_vis[n.layer as usize] {
                    continue;
                }
                let start = n.inst_start;
                let end = start + n.inst_count;
                rp.draw_indexed(0..6, 0, start..end);
                stats.nodes_drawn += 1;
                stats.points_drawn += (end - start) as u64;
            }
            if self.near_count > 0 && layer_vis[layer::TERRAIN as usize] {
                if let Some(nb) = &self.near_buf {
                    rp.set_vertex_buffer(0, nb.slice(..));
                    rp.draw_indexed(0..6, 0, 0..self.near_count);
                    stats.nodes_drawn += 1;
                    stats.points_drawn += self.near_count as u64;
                }
            }
            if layer_vis[layer::FURNITURE as usize] {
                rp.set_pipeline(&self.ln_pipe);
                if self.n_lines > 0 {
                    if let Some(lb) = &self.lines_buf {
                        rp.set_vertex_buffer(0, lb.slice(..));
                        rp.draw(0..self.n_lines, 0..1);
                    }
                }
                rp.set_pipeline(&self.qd_pipe);
                if self.n_quads > 0 {
                    if let Some(qb) = &self.quads_buf {
                        rp.set_vertex_buffer(0, qb.slice(..));
                        rp.draw(0..self.n_quads, 0..1);
                    }
                }
                if let Some(bb) = &self.beacon_buf {
                    rp.set_vertex_buffer(0, bb.slice(..));
                    rp.draw(0..self.n_beacons, 0..1);
                }
            }
            // continuous road ribbons (palette-aware, always solid under the
            // point texture — fixes dashed roads when zoomed, user feedback)
            if layer_vis[layer::ROAD as usize] && self.n_road_quads > 0 {
                rp.set_pipeline(&self.qd_pipe);
                if let Some(rb) = &self.road_quad_buf {
                    rp.set_vertex_buffer(0, rb.slice(..));
                    rp.draw(0..self.n_road_quads, 0..1);
                }
            }
            if false {
                if let Some(bb) = &self.beacon_buf {
                    rp.set_vertex_buffer(0, bb.slice(..));
                    rp.draw(0..self.n_beacons, 0..1);
                }
            }
            if s.bg_mode == 1 && self.n_grid > 0 {
                if let Some(gb) = &self.grid_buf {
                    rp.set_pipeline(&self.ln_pipe);
                    rp.set_vertex_buffer(0, gb.slice(..));
                    rp.draw(0..self.n_grid, 0..1);
                }
            }
            self.last_stats = stats;
        }

        // ---- [1] bloom ----
        for i in 0..BLOOM_MIPS {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom-down"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.downs[i].view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.down_pipe);
            rp.set_bind_group(0, &self.down_bgs[i], &[]);
            rp.draw(0..3, 0..1);
        }
        for i in (0..BLOOM_MIPS).rev() {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom-up"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.ups[i].view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.up_pipe);
            rp.set_bind_group(0, &self.up_bgs[i], &[]);
            rp.draw(0..3, 0..1);
        }

        // ---- [2] composite → target ----
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.comp_pipe);
            let cbg = if s.bloom_skip > 0 {
                self.comp_bg_coarse.as_ref().unwrap()
            } else {
                self.comp_bg.as_ref().unwrap()
            };
            rp.set_bind_group(0, cbg, &[]);
            rp.draw(0..3, 0..1);
        }

        if let Some(qs) = &self.ts_query {
            encoder.resolve_query_set(qs, 0..2, self.ts_buf.as_ref().unwrap(), 0);
            self.queue.submit(Some(encoder.finish()));
        } else {
            self.queue.submit(Some(encoder.finish()));
        }
        self.last_stats
    }

    /// GPU ms for the last resolved scene pass (None if unsupported).
    pub fn read_gpu_ms(&self) -> Option<f32> {
        let buf = self.ts_buf.as_ref()?;
        let read = self.ts_read.as_ref()?;
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ts-copy") });
        enc.copy_buffer_to_buffer(buf, 0, read, 0, 16);
        self.queue.submit(Some(enc.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        rx.recv().ok()?.ok()?;
        let mut vals = [0u64; 2];
        {
            let mapped = read.slice(..).get_mapped_range();
            vals[0] = u64::from_le_bytes(mapped[0..8].try_into().ok()?);
            vals[1] = u64::from_le_bytes(mapped[8..16].try_into().ok()?);
        }
        read.unmap();
        let period = self.queue.get_timestamp_period(); // ns per tick
        Some((vals[1].saturating_sub(vals[0]) as f32) * period / 1e6)
    }

    /// Download an RGBA8 texture to CPU (headless screenshots / golden renders).
    pub fn download_rgba8(&self, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
        let stride = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (stride * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("download") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        rx.recv().unwrap().unwrap();
        let mut out = vec![0u8; (w * h * 4) as usize];
        {
            let mapped = buf.slice(..).get_mapped_range();
            for row in 0..h as usize {
                out[row * (w as usize * 4)..(row + 1) * (w as usize * 4)]
                    .copy_from_slice(&mapped[row * stride as usize..row * stride as usize + w as usize * 4]);
            }
        }
        buf.unmap();
        out
    }

    pub fn last_stats(&self) -> FrameStats {
        self.last_stats
    }
}

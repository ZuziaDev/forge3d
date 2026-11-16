//! [`GpuRenderer`]: wgpu viewport renderer with per-node GPU mesh caching,
//! PBR shading, grid, wireframe, selection highlights, ID-buffer picking and
//! offscreen readback.

use std::collections::{HashMap, HashSet};

use forge3d_core::math::Aabb;
use forge3d_core::mesh::Mesh;
use forge3d_core::scene::{Material, Node, Scene};
use forge3d_core::selection::{SelectMode, Selection};
use glam::{Affine3A, Vec2, Vec4};
use uuid::Uuid;
use wgpu::util::DeviceExt;

use super::uniforms::{
    FrameUniform, LineInstance, MeshVertex, ObjectUniform, LINE_ATTRS, MESH_ATTRS,
};
use crate::camera::Camera;
use crate::error::{RenderError, RenderResult};
use crate::extract::{
    display_mesh, is_edited, node_crease_deg, object_highlight, EditHighlight, MeshGeometry,
    ObjectHighlight,
};
use crate::image::RgbaImage;
use crate::options::{style, RenderOptions, ViewOptions};
use crate::pick::{nearest_face_elements, PickHit};
use crate::shaders;
use crate::shading::{linear_to_srgb, srgb_to_linear};

/// Depth format of the renderer's own depth buffers.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const OBJECT_UNIFORM_SIZE: u64 = std::mem::size_of::<ObjectUniform>() as u64;

/// Construction parameters of a [`GpuRenderer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuRendererConfig {
    /// Format of the color targets passed to [`GpuRenderer::render`].
    pub color_format: wgpu::TextureFormat,
    /// MSAA sample count of the main pass: 1 or 4.
    pub sample_count: u32,
}

impl GpuRendererConfig {
    /// 4× MSAA with the given target format.
    pub fn new(color_format: wgpu::TextureFormat) -> Self {
        GpuRendererConfig {
            color_format,
            sample_count: 4,
        }
    }
}

/// What [`GpuRenderer::update_scene`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SceneUpdateStats {
    /// Meshes whose GPU buffers were (re)built.
    pub uploaded: usize,
    /// Meshes whose GPU buffers were kept.
    pub reused: usize,
    /// Cached meshes dropped (node deleted, hidden or mesh removed).
    pub removed: usize,
    /// Objects that will be drawn.
    pub objects: usize,
    /// True when nothing had to be checked (same revision, selection and options).
    pub skipped: bool,
}

/// Selection overlay buffers of one object (edit modes).
#[derive(Default)]
struct OverlayBuffers {
    faces: Option<(wgpu::Buffer, u32)>,
    edges: Option<(wgpu::Buffer, u32)>,
    points: Option<(wgpu::Buffer, u32)>,
}

/// Cached GPU data of one mesh node.
struct MeshEntry {
    fingerprint: u64,
    crease_deg: f32,
    /// Whether the geometry is the evaluated (modified) mesh, not the cage.
    evaluated: bool,
    /// Local-space geometry (kept for picking resolution and overlays).
    geometry: MeshGeometry,
    triangles: Option<(wgpu::Buffer, u32)>,
    wire: Option<(wgpu::Buffer, u32)>,
    vertices: Option<(wgpu::Buffer, u32)>,
    overlay: OverlayBuffers,
    overlay_key: Option<EditHighlight>,
}

/// One object drawn this frame.
struct FrameObject {
    node_id: Uuid,
    world: Affine3A,
    highlight: ObjectHighlight,
}

struct Targets {
    size: (u32, u32),
    depth: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    mask: wgpu::TextureView,
    mask_bind_group: wgpu::BindGroup,
}

struct PickTargets {
    size: (u32, u32),
    id: wgpu::Texture,
    id_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    readback: wgpu::Buffer,
}

/// 64-bit FNV-1a over 32-bit words.
struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    #[inline]
    fn word(&mut self, w: u32) {
        self.0 = (self.0 ^ w as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Fingerprint of what a node displays: its base mesh plus, when the
/// evaluated mesh is shown, the modifier stack.
fn node_fingerprint(node: &Node, mesh: &Mesh, modified: bool) -> u64 {
    let base = mesh_fingerprint(mesh);
    if !modified {
        return base;
    }
    let mut h = Fnv(base);
    for b in format!("{:?}", node.modifiers).bytes() {
        h.word(b as u32);
    }
    h.0
}

/// Fingerprint of a mesh's geometry and topology.
pub fn mesh_fingerprint(mesh: &Mesh) -> u64 {
    let mut h = Fnv::new();
    h.word(mesh.vertex_count() as u32);
    h.word(mesh.face_count() as u32);
    h.word(mesh.edge_count() as u32);
    for p in mesh.positions() {
        h.word(p.x.to_bits());
        h.word(p.y.to_bits());
        h.word(p.z.to_bits());
    }
    for f in mesh.face_ids() {
        h.word(u32::MAX);
        for v in mesh.face_vertices(f) {
            h.word(v.0);
        }
    }
    h.0
}

/// wgpu viewport renderer.
///
/// It does not own a window or surface: the caller passes a color target
/// view to [`GpuRenderer::render`] each frame (a swapchain image or an
/// offscreen texture, e.g. one registered as an egui texture).
///
/// Per frame:
/// 1. [`GpuRenderer::update_scene`] with the scene, selection, document
///    revision and view options (cheap when nothing changed),
/// 2. [`GpuRenderer::render`] into the caller's encoder and view,
/// 3. on click, [`GpuRenderer::pick`] (blocking ID-buffer readback).
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: GpuRendererConfig,
    encode_srgb: bool,
    object_layout: wgpu::BindGroupLayout,
    mask_layout: wgpu::BindGroupLayout,
    mesh_pipeline: wgpu::RenderPipeline,
    highlight_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    id_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    pick_frame_buffer: wgpu::Buffer,
    pick_frame_bind_group: wgpu::BindGroup,
    object_buffer: wgpu::Buffer,
    object_bind_group: wgpu::BindGroup,
    object_capacity: usize,
    object_stride: u64,
    meshes: HashMap<Uuid, MeshEntry>,
    objects: Vec<FrameObject>,
    bounds: Option<Aabb>,
    last_revision: Option<u64>,
    last_selection: Option<Selection>,
    last_view: Option<ViewOptions>,
    targets: Option<Targets>,
    pick_targets: Option<PickTargets>,
}

fn uniform_entry(binding: u32, dynamic: bool, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn alpha_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
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

/// Whether a target format can be used: 8-bit color formats for readback,
/// any renderable float/unorm color format for `render`.
fn readback_layout(format: wgpu::TextureFormat) -> Option<bool> {
    // Some(swap_rb)
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(false),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => Some(true),
        _ => None,
    }
}

struct PipelineSpec<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: &'a str,
    layout: &'a wgpu::PipelineLayout,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    depth: Option<(bool, wgpu::CompareFunction)>,
    samples: u32,
}

fn pipeline(device: &wgpu::Device, s: PipelineSpec<'_>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(s.label),
        layout: Some(s.layout),
        vertex: wgpu::VertexState {
            module: s.module,
            entry_point: Some(s.vs),
            compilation_options: Default::default(),
            buffers: s.buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: s.depth.map(|(write, cmp)| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(cmp),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: s.samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: s.module,
            entry_point: Some(s.fs),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: s.format,
                blend: s.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl GpuRenderer {
    /// Create a renderer for targets of `config.color_format`. The device
    /// and queue handles are cloned (cheap reference counts).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: GpuRendererConfig,
    ) -> RenderResult<GpuRenderer> {
        if config.sample_count != 1 && config.sample_count != 4 {
            return Err(RenderError::option("sample_count", "must be 1 or 4"));
        }
        let format = config.color_format;
        let encode_srgb = !format.is_srgb();
        let frame_size = std::mem::size_of::<FrameUniform>() as u64;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forge3d frame layout"),
            entries: &[uniform_entry(0, false, frame_size)],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forge3d object layout"),
            entries: &[uniform_entry(0, true, OBJECT_UNIFORM_SIZE)],
        });
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forge3d mask layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let object_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forge3d object pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&object_layout)],
            immediate_size: 0,
        });
        let frame_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forge3d frame pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let outline_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forge3d outline pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&mask_layout)],
            immediate_size: 0,
        });
        let module = |label: &str, src: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let mesh_sm = module("forge3d mesh.wgsl", shaders::mesh());
        let lines_sm = module("forge3d lines.wgsl", shaders::lines());
        let grid_sm = module("forge3d grid.wgsl", shaders::grid());
        let outline_sm = module("forge3d outline.wgsl", shaders::outline());

        let mesh_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &MESH_ATTRS,
        })];
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &LINE_ATTRS,
        })];
        let samples = config.sample_count;
        let less = wgpu::CompareFunction::LessEqual;
        let mesh_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d mesh",
                module: &mesh_sm,
                vs: "vs_main",
                fs: "fs_shade",
                layout: &object_pl,
                buffers: &mesh_buffers,
                format,
                blend: None,
                depth: Some((true, less)),
                samples,
            },
        );
        let highlight_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d face highlight",
                module: &mesh_sm,
                vs: "vs_overlay",
                fs: "fs_highlight",
                layout: &object_pl,
                buffers: &mesh_buffers,
                format,
                blend: Some(alpha_blend()),
                depth: Some((false, less)),
                samples,
            },
        );
        let line_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d lines",
                module: &lines_sm,
                vs: "vs_line",
                fs: "fs_line",
                layout: &object_pl,
                buffers: &line_buffers,
                format,
                blend: Some(alpha_blend()),
                depth: Some((false, less)),
                samples,
            },
        );
        let grid_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d grid",
                module: &grid_sm,
                vs: "vs_fullscreen",
                fs: "fs_grid",
                layout: &frame_pl,
                buffers: &[],
                format,
                blend: Some(alpha_blend()),
                depth: Some((false, wgpu::CompareFunction::Less)),
                samples,
            },
        );
        let id_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d id",
                module: &mesh_sm,
                vs: "vs_main",
                fs: "fs_id",
                layout: &object_pl,
                buffers: &mesh_buffers,
                format: ID_FORMAT,
                blend: None,
                depth: Some((true, less)),
                samples: 1,
            },
        );
        let mask_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d selection mask",
                module: &mesh_sm,
                vs: "vs_main",
                fs: "fs_mask",
                layout: &object_pl,
                buffers: &mesh_buffers,
                format: MASK_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Max,
                    },
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
                depth: None,
                samples: 1,
            },
        );
        let outline_pipeline = pipeline(
            device,
            PipelineSpec {
                label: "forge3d outline",
                module: &outline_sm,
                vs: "vs_fullscreen",
                fs: "fs_outline",
                layout: &outline_pl,
                buffers: &[],
                format,
                blend: Some(alpha_blend()),
                depth: None,
                samples: 1,
            },
        );

        let make_frame = |label: &str| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: frame_size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            (buf, bg)
        };
        let (frame_buffer, frame_bind_group) = make_frame("forge3d frame uniforms");
        let (pick_frame_buffer, pick_frame_bind_group) = make_frame("forge3d pick frame uniforms");
        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let object_stride = OBJECT_UNIFORM_SIZE.div_ceil(align) * align;
        let (object_buffer, object_bind_group) =
            Self::object_storage(device, &object_layout, 16, object_stride);
        Ok(GpuRenderer {
            device: device.clone(),
            queue: queue.clone(),
            config,
            encode_srgb,
            object_layout,
            mask_layout,
            mesh_pipeline,
            highlight_pipeline,
            line_pipeline,
            grid_pipeline,
            id_pipeline,
            mask_pipeline,
            outline_pipeline,
            frame_buffer,
            frame_bind_group,
            pick_frame_buffer,
            pick_frame_bind_group,
            object_buffer,
            object_bind_group,
            object_capacity: 16,
            object_stride,
            meshes: HashMap::new(),
            objects: Vec::new(),
            bounds: None,
            last_revision: None,
            last_selection: None,
            last_view: None,
            targets: None,
            pick_targets: None,
        })
    }

    fn object_storage(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: usize,
        stride: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge3d object uniforms"),
            size: stride * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge3d object uniforms"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(OBJECT_UNIFORM_SIZE),
                }),
            }],
        });
        (buf, bg)
    }

    /// The configuration this renderer was created with.
    pub fn config(&self) -> GpuRendererConfig {
        self.config
    }

    /// World bounds of the objects of the last [`GpuRenderer::update_scene`].
    pub fn bounds(&self) -> Option<Aabb> {
        self.bounds
    }

    /// Number of cached meshes.
    pub fn cached_mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Drop all cached GPU meshes (they are rebuilt on the next update).
    pub fn clear_cache(&mut self) {
        self.meshes.clear();
        self.last_revision = None;
        self.last_selection = None;
    }

    /// Force a rebuild of one node's GPU buffers on the next update.
    pub fn invalidate_node(&mut self, id: Uuid) {
        self.meshes.remove(&id);
        self.last_revision = None;
    }

    fn buffer_init<T: bytemuck::Pod>(
        &self,
        label: &str,
        data: &[T],
        usage: wgpu::BufferUsages,
    ) -> Option<(wgpu::Buffer, u32)> {
        if data.is_empty() {
            return None;
        }
        let buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage,
            });
        Some((buf, data.len() as u32))
    }

    fn build_entry(&self, mesh: &Mesh, fingerprint: u64, crease_deg: f32) -> MeshEntry {
        let geometry = MeshGeometry::from_mesh_shaded(mesh, crease_deg);
        let verts: Vec<MeshVertex> = (0..geometry.positions.len())
            .map(|i| MeshVertex {
                pos: geometry.positions[i].to_array(),
                normal: geometry.normals[i].to_array(),
                face: geometry.tri_faces[i / 3],
            })
            .collect();
        let wire: Vec<LineInstance> = geometry
            .edges
            .iter()
            .map(|&[a, b]| {
                LineInstance::line(
                    geometry.vertices[a as usize],
                    geometry.vertices[b as usize],
                    [1.0; 4],
                    style::WIRE_WIDTH,
                    true,
                )
            })
            .collect();
        let points: Vec<LineInstance> = geometry
            .vertices
            .iter()
            .map(|&p| LineInstance::point(p, style::VERTEX, style::VERTEX_SIZE))
            .collect();
        let vb = wgpu::BufferUsages::VERTEX;
        MeshEntry {
            fingerprint,
            crease_deg,
            evaluated: false,
            triangles: self.buffer_init("forge3d mesh triangles", &verts, vb),
            wire: self.buffer_init("forge3d mesh wire", &wire, vb),
            vertices: self.buffer_init("forge3d mesh vertices", &points, vb),
            geometry,
            overlay: OverlayBuffers::default(),
            overlay_key: None,
        }
    }

    fn build_overlay(&self, g: &MeshGeometry, e: &EditHighlight) -> OverlayBuffers {
        let vb = wgpu::BufferUsages::VERTEX;
        let mut faces = Vec::new();
        for t in 0..g.triangle_count() {
            if e.faces.contains(&g.tri_faces[t]) {
                for k in 0..3 {
                    faces.push(MeshVertex {
                        pos: g.positions[3 * t + k].to_array(),
                        normal: g.normals[3 * t + k].to_array(),
                        face: g.tri_faces[t],
                    });
                }
            }
        }
        let edges: Vec<LineInstance> = e
            .edges
            .iter()
            .filter_map(|&ei| g.edges.get(ei as usize))
            .map(|&[a, b]| {
                LineInstance::line(
                    g.vertices[a as usize],
                    g.vertices[b as usize],
                    style::ELEMENT_SELECTED,
                    style::SELECTED_EDGE_WIDTH,
                    false,
                )
            })
            .collect();
        let points: Vec<LineInstance> = if e.mode == SelectMode::Vertex {
            e.vertices
                .iter()
                .filter_map(|&v| g.vertices.get(v as usize))
                .map(|&p| {
                    LineInstance::point(p, style::ELEMENT_SELECTED, style::SELECTED_VERTEX_SIZE)
                })
                .collect()
        } else {
            Vec::new()
        };
        OverlayBuffers {
            faces: self.buffer_init("forge3d selected faces", &faces, vb),
            edges: self.buffer_init("forge3d selected edges", &edges, vb),
            points: self.buffer_init("forge3d selected vertices", &points, vb),
        }
    }

    /// Synchronize GPU buffers with the scene. Call every frame before
    /// [`GpuRenderer::render`]; pass the document revision
    /// ([`forge3d_core::document::Document::revision`]) so unchanged frames
    /// skip all work. With `revision: None` meshes are fingerprinted each
    /// call and only changed ones are re-uploaded. Transform and material
    /// changes never rebuild vertex buffers.
    pub fn update_scene(
        &mut self,
        scene: &Scene,
        selection: Option<&Selection>,
        revision: Option<u64>,
        view: &ViewOptions,
    ) -> SceneUpdateStats {
        let same_rev = revision.is_some() && revision == self.last_revision;
        let same_sel = self.last_selection.as_ref() == selection;
        let same_view = self.last_view.as_ref() == Some(view);
        if same_rev && same_sel && same_view {
            return SceneUpdateStats {
                reused: self.objects.len(),
                objects: self.objects.len(),
                skipped: true,
                ..Default::default()
            };
        }
        let mut stats = SceneUpdateStats::default();
        let smooth = view.smooth_shading;
        let show_sel = view.show_selection;
        self.objects.clear();
        self.bounds = None;
        let mut seen = HashSet::new();
        for id in scene.traverse() {
            let Some(node) = scene.get(id) else { continue };
            let Some(mesh) = node.mesh.as_ref() else {
                continue;
            };
            let edited = selection.is_some_and(|s| is_edited(s, id));
            let modified = !edited && node.has_active_modifiers();
            if !node.visible || (!modified && mesh.face_count() == 0 && mesh.edge_count() == 0) {
                continue;
            }
            let Ok(world) = scene.world_transform(id) else {
                continue;
            };
            let crease = node_crease_deg(node, smooth);
            let check_geometry = !same_rev
                || self
                    .meshes
                    .get(&id)
                    .is_none_or(|e| e.evaluated != modified || e.crease_deg != crease);
            if check_geometry {
                let fp = node_fingerprint(node, mesh, modified);
                let fresh = self.meshes.get(&id).is_some_and(|e| {
                    e.fingerprint == fp && e.crease_deg == crease && e.evaluated == modified
                });
                if fresh {
                    stats.reused += 1;
                } else {
                    let shown = display_mesh(node, selection);
                    let mut entry = self.build_entry(&shown, fp, crease);
                    entry.evaluated = modified;
                    self.meshes.insert(id, entry);
                    stats.uploaded += 1;
                }
            } else {
                stats.reused += 1;
            }
            seen.insert(id);
            let highlight = if show_sel {
                object_highlight(selection, id, mesh)
            } else {
                ObjectHighlight::default()
            };
            let entry = self.meshes.get(&id).expect("entry inserted above");
            if entry.overlay_key != highlight.edit {
                let overlay = match &highlight.edit {
                    Some(e) => self.build_overlay(&entry.geometry, e),
                    None => OverlayBuffers::default(),
                };
                let entry = self.meshes.get_mut(&id).expect("entry exists");
                entry.overlay = overlay;
                entry.overlay_key = highlight.edit.clone();
            }
            let entry = &self.meshes[&id];
            if let Some(b) = entry.geometry.bounds() {
                let b = b.transformed(&world);
                self.bounds = Some(self.bounds.map_or(b, |o| o.union(&b)));
            }
            self.objects.push(FrameObject {
                node_id: id,
                world,
                highlight,
            });
        }
        let before = self.meshes.len();
        self.meshes.retain(|id, _| seen.contains(id));
        stats.removed = before - self.meshes.len();
        stats.objects = self.objects.len();
        self.write_object_uniforms(scene);
        self.last_revision = revision;
        self.last_selection = selection.cloned();
        self.last_view = Some(*view);
        stats
    }

    fn write_object_uniforms(&mut self, scene: &Scene) {
        if self.objects.len() > self.object_capacity {
            let cap = self.objects.len().next_power_of_two();
            let (b, g) =
                Self::object_storage(&self.device, &self.object_layout, cap, self.object_stride);
            self.object_buffer = b;
            self.object_bind_group = g;
            self.object_capacity = cap;
        }
        if self.objects.is_empty() {
            return;
        }
        let stride = self.object_stride as usize;
        let mut data = vec![0u8; stride * self.objects.len()];
        let default_material = Material::default();
        for (i, o) in self.objects.iter().enumerate() {
            let material = scene
                .get(o.node_id)
                .and_then(|n| n.material.as_ref())
                .unwrap_or(&default_material);
            let hl = &o.highlight;
            let mask = if hl.selected {
                if hl.active {
                    2
                } else {
                    1
                }
            } else {
                0
            };
            let u = ObjectUniform::new(
                &o.world,
                material,
                i as u32 + 1,
                mask,
                hl.selected,
                hl.edit.is_some(),
            );
            data[i * stride..i * stride + OBJECT_UNIFORM_SIZE as usize]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        self.queue.write_buffer(&self.object_buffer, 0, &data);
    }

    fn ensure_targets(&mut self, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let tex = |label: &str, format, samples, usage| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let samples = self.config.sample_count;
        let depth = tex(
            "forge3d depth",
            DEPTH_FORMAT,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let msaa = (samples > 1).then(|| {
            tex(
                "forge3d msaa color",
                self.config.color_format,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let mask = tex(
            "forge3d selection mask",
            MASK_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let mask_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("forge3d mask"),
            layout: &self.mask_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&mask),
            }],
        });
        self.targets = Some(Targets {
            size,
            depth,
            msaa,
            mask,
            mask_bind_group,
        });
    }

    fn clear_color(&self, bg: [f32; 4]) -> wgpu::Color {
        let c = |x: f32| {
            if self.encode_srgb {
                x as f64
            } else {
                srgb_to_linear(x) as f64
            }
        };
        wgpu::Color {
            r: c(bg[0]),
            g: c(bg[1]),
            b: c(bg[2]),
            a: bg[3] as f64,
        }
    }

    fn object_offset(&self, i: usize) -> u32 {
        (i as u64 * self.object_stride) as u32
    }

    /// Record the viewport passes into `encoder`, drawing into `target`
    /// (format must equal the configured color format, size `size`).
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        camera: &Camera,
        view: &ViewOptions,
    ) {
        self.render_scaled(encoder, target, size, camera, view, 1.0);
    }

    fn render_scaled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        camera: &Camera,
        view: &ViewOptions,
        px_scale: f32,
    ) {
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_targets(size);
        let mut cam = *camera;
        if let Some(b) = &self.bounds {
            cam.fit_clip_to_bounds(b);
        }
        let fu = FrameUniform::new(&cam, size.0, size.1, px_scale, self.encode_srgb);
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&fu));
        let targets = self.targets.as_ref().expect("targets ensured");
        let show_sel = view.show_selection;
        {
            let (color_view, resolve) = match &targets.msaa {
                Some(m) => (m, Some(target)),
                None => (target, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forge3d main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color(view.background)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            // Opaque surfaces.
            pass.set_pipeline(&self.mesh_pipeline);
            for (i, o) in self.objects.iter().enumerate() {
                if let Some((buf, n)) = &self.meshes[&o.node_id].triangles {
                    pass.set_bind_group(1, &self.object_bind_group, &[self.object_offset(i)]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*n, 0..1);
                }
            }
            // Selected face tint.
            if show_sel {
                pass.set_pipeline(&self.highlight_pipeline);
                for (i, o) in self.objects.iter().enumerate() {
                    if let Some((buf, n)) = &self.meshes[&o.node_id].overlay.faces {
                        pass.set_bind_group(1, &self.object_bind_group, &[self.object_offset(i)]);
                        pass.set_vertex_buffer(0, buf.slice(..));
                        pass.draw(0..*n, 0..1);
                    }
                }
            }
            if view.grid {
                pass.set_pipeline(&self.grid_pipeline);
                pass.draw(0..3, 0..1);
            }
            // Lines and points.
            pass.set_pipeline(&self.line_pipeline);
            for (i, o) in self.objects.iter().enumerate() {
                let entry = &self.meshes[&o.node_id];
                let edit = o.highlight.edit.as_ref().filter(|_| show_sel);
                let mut draws: Vec<&(wgpu::Buffer, u32)> = Vec::new();
                if view.wireframe || edit.is_some() {
                    draws.extend(entry.wire.as_ref());
                }
                if let Some(e) = edit {
                    draws.extend(entry.overlay.edges.as_ref());
                    if e.mode == SelectMode::Vertex {
                        draws.extend(entry.vertices.as_ref());
                        draws.extend(entry.overlay.points.as_ref());
                    }
                }
                if draws.is_empty() {
                    continue;
                }
                pass.set_bind_group(1, &self.object_bind_group, &[self.object_offset(i)]);
                for (buf, n) in draws {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..*n);
                }
            }
        }
        let any_selected = show_sel && self.objects.iter().any(|o| o.highlight.selected);
        if !any_selected {
            return;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forge3d selection mask pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mask_pipeline);
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            for (i, o) in self.objects.iter().enumerate() {
                if !o.highlight.selected {
                    continue;
                }
                if let Some((buf, n)) = &self.meshes[&o.node_id].triangles {
                    pass.set_bind_group(1, &self.object_bind_group, &[self.object_offset(i)]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*n, 0..1);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forge3d outline pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.outline_pipeline);
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            pass.set_bind_group(1, &targets.mask_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn ensure_pick_targets(&mut self, size: (u32, u32)) {
        if self.pick_targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let desc = |label, format, usage| wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        let id = self.device.create_texture(&desc(
            "forge3d id buffer",
            ID_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        ));
        let depth = self
            .device
            .create_texture(&desc(
                "forge3d id depth",
                DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ))
            .create_view(&Default::default());
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge3d id readback"),
            size: wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.pick_targets = Some(PickTargets {
            size,
            id_view: id.create_view(&Default::default()),
            id,
            depth,
            readback,
        });
    }

    /// GPU pick: render the ID buffer (node id + face id + depth) for the
    /// current scene state and read back the pixel under (`x`, `y`) (pixels,
    /// top-left origin) of a `size` viewport. Blocks until the GPU is done.
    /// The nearest vertex and edge of the hit face are resolved on the CPU.
    pub fn pick(
        &mut self,
        camera: &Camera,
        size: (u32, u32),
        x: f32,
        y: f32,
    ) -> RenderResult<Option<PickHit>> {
        let size = (size.0.max(1), size.1.max(1));
        if x < 0.0 || y < 0.0 || x >= size.0 as f32 || y >= size.1 as f32 {
            return Ok(None);
        }
        self.ensure_pick_targets(size);
        let mut cam = *camera;
        if let Some(b) = &self.bounds {
            cam.fit_clip_to_bounds(b);
        }
        let fu = FrameUniform::new(&cam, size.0, size.1, 1.0, self.encode_srgb);
        self.queue
            .write_buffer(&self.pick_frame_buffer, 0, bytemuck::bytes_of(&fu));
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pt = self.pick_targets.as_ref().expect("pick targets ensured");
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge3d pick"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forge3d id pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pt.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &pt.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.id_pipeline);
            pass.set_bind_group(0, &self.pick_frame_bind_group, &[]);
            for (i, o) in self.objects.iter().enumerate() {
                if let Some((buf, n)) = &self.meshes[&o.node_id].triangles {
                    pass.set_bind_group(1, &self.object_bind_group, &[self.object_offset(i)]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*n, 0..1);
                }
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &pt.id,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: x as u32,
                    y: y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &pt.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let words = self.submit_and_read(encoder, &pt.readback, 16, scope)?;
        let px: [u32; 4] = [
            u32::from_le_bytes([words[0], words[1], words[2], words[3]]),
            u32::from_le_bytes([words[4], words[5], words[6], words[7]]),
            u32::from_le_bytes([words[8], words[9], words[10], words[11]]),
            u32::from_le_bytes([words[12], words[13], words[14], words[15]]),
        ];
        let (pick_id, face, depth_bits) = (px[0], px[1], px[2]);
        if pick_id == 0 {
            return Ok(None);
        }
        let Some(o) = self.objects.get(pick_id as usize - 1) else {
            return Ok(None);
        };
        let entry = &self.meshes[&o.node_id];
        let aspect = size.0 as f32 / size.1 as f32;
        let vp = cam.view_projection(aspect);
        let (vertex, edge) =
            nearest_face_elements(&entry.geometry, &o.world, &vp, size, Vec2::new(x, y), face);
        // Reconstruct the world position from the stored NDC depth.
        let px_center = (x.floor() + 0.5, y.floor() + 0.5);
        let ndc = Vec4::new(
            px_center.0 / size.0 as f32 * 2.0 - 1.0,
            1.0 - px_center.1 / size.1 as f32 * 2.0,
            f32::from_bits(depth_bits),
            1.0,
        );
        let wp = vp.inverse() * ndc;
        let position = wp.truncate() / wp.w;
        let ray = cam.ray_through_pixel(px_center.0, px_center.1, size.0, size.1);
        Ok(Some(PickHit {
            node_id: o.node_id,
            face,
            vertex,
            edge,
            position: position.to_array(),
            distance: (position - ray.origin).length(),
        }))
    }

    fn submit_and_read(
        &self,
        encoder: wgpu::CommandEncoder,
        buffer: &wgpu::Buffer,
        len: u64,
        scope: wgpu::ErrorScopeGuard,
    ) -> RenderResult<Vec<u8>> {
        let index = self.queue.submit([encoder.finish()]);
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(RenderError::Gpu(err.to_string()));
        }
        let slice = buffer.slice(..len);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .map_err(|e| RenderError::Gpu(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Gpu(e.to_string()))?
            .map_err(|e| RenderError::Gpu(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Gpu(format!("{e:?}")))?
            .to_vec();
        buffer.unmap();
        Ok(data)
    }

    /// Render the scene state of the last update into an offscreen texture
    /// and read it back. The renderer's color format must be an 8-bit
    /// RGBA/BGRA format. `options.supersample` renders larger and box-filters.
    pub fn render_to_rgba(
        &mut self,
        camera: &Camera,
        options: &RenderOptions,
    ) -> RenderResult<RgbaImage> {
        options.validate()?;
        camera.validate()?;
        let format = self.config.color_format;
        let swap_rb = readback_layout(format).ok_or_else(|| {
            RenderError::UnsupportedFormat(format!("{format:?} cannot be read back as RGBA8"))
        })?;
        let ss = options.supersample;
        let (w, h) = (options.width * ss, options.height * ss);
        let max = self.device.limits().max_texture_dimension_2d;
        if w > max || h > max {
            return Err(RenderError::option(
                "width",
                format!("{w}x{h} (with supersampling) exceeds the GPU texture limit {max}"),
            ));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge3d offscreen color"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let size = row as u64 * h as u64;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge3d offscreen readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge3d offscreen"),
            });
        self.render_scaled(
            &mut encoder,
            &view,
            (w, h),
            camera,
            &options.view,
            ss as f32,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let data = self.submit_and_read(encoder, &readback, size, scope)?;
        Ok(downsample(
            &data,
            row as usize,
            options.width,
            options.height,
            ss,
            swap_rb,
        ))
    }

    /// Update from a scene and render it offscreen (convenience for headless use).
    pub fn render_scene_to_rgba(
        &mut self,
        scene: &Scene,
        selection: Option<&Selection>,
        camera: &Camera,
        options: &RenderOptions,
    ) -> RenderResult<RgbaImage> {
        self.update_scene(scene, selection, None, &options.view);
        self.render_to_rgba(camera, options)
    }
}

/// Box-filter sRGB RGBA8 rows (in linear, premultiplied) into an image.
fn downsample(data: &[u8], row: usize, ow: u32, oh: u32, ss: u32, swap_rb: bool) -> RgbaImage {
    let (ow, oh, ss) = (ow as usize, oh as usize, ss as usize);
    let mut pixels = vec![0u8; ow * oh * 4];
    let lut: Vec<f32> = (0..256).map(|i| srgb_to_linear(i as f32 / 255.0)).collect();
    let n = (ss * ss) as f32;
    for y in 0..oh {
        for x in 0..ow {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let i = (y * ss + sy) * row + (x * ss + sx) * 4;
                    let a = data[i + 3] as f32 / 255.0;
                    let (r, b) = if swap_rb {
                        (data[i + 2], data[i])
                    } else {
                        (data[i], data[i + 2])
                    };
                    acc[0] += lut[r as usize] * a;
                    acc[1] += lut[data[i + 1] as usize] * a;
                    acc[2] += lut[b as usize] * a;
                    acc[3] += a;
                }
            }
            let a = acc[3] / n;
            let un = if a > 1e-6 { 1.0 / (a * n) } else { 0.0 };
            let o = (y * ow + x) * 4;
            for k in 0..3 {
                pixels[o + k] = (linear_to_srgb(acc[k] * un) * 255.0 + 0.5) as u8;
            }
            pixels[o + 3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    RgbaImage {
        width: ow as u32,
        height: oh as u32,
        pixels,
    }
}

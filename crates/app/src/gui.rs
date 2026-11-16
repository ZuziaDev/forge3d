//! GPU-side glue: the egui renderer, the 3D viewport renderer and the
//! offscreen viewport texture shown in the central panel.
//!
//! [`Gui`] is independent of winit: it takes an [`egui::RawInput`] and draws
//! into any texture view, so the same code serves the window, headless
//! screenshots and tests.

use std::time::Duration;

use egui::ViewportId;
use forge3d_core::document::Document;
use forge3d_render::gpu::{GpuRenderer, GpuRendererConfig};
use forge3d_render::wgpu;

use crate::error::AppError;
use crate::state::AppState;
use crate::ui;

/// Format of the offscreen viewport texture. Gamma-space, so egui can sample
/// it directly (the viewport renderer encodes sRGB itself for non-sRGB targets).
const VIEWPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct ViewportTexture {
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// Result of one frame.
pub struct FrameOutput {
    /// Cursor, clipboard, ... requests for the windowing layer.
    pub platform_output: egui::PlatformOutput,
    /// When the next frame should be drawn (`Duration::MAX` = on next input).
    pub repaint_after: Duration,
}

/// The application's renderer and UI.
pub struct Gui {
    pub state: AppState,
    ctx: egui::Context,
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: egui_wgpu::Renderer,
    gpu: GpuRenderer,
    viewport: Option<ViewportTexture>,
    viewport_id: egui::TextureId,
    adapter_info: String,
}

impl Gui {
    /// Create the renderers for targets of `target_format` (preferably a
    /// non-sRGB format such as `Bgra8Unorm` / `Rgba8Unorm`).
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        doc: Document,
        adapter_info: String,
    ) -> Result<Gui, AppError> {
        let ctx = egui::Context::default();
        ui::setup_style(&ctx);
        let mut egui_renderer =
            egui_wgpu::Renderer::new(&device, target_format, egui_wgpu::RendererOptions::default());
        let gpu = GpuRenderer::new(
            &device,
            &queue,
            GpuRendererConfig::new(VIEWPORT_FORMAT),
        )?;
        let placeholder = create_viewport_texture(&device, (1, 1));
        let viewport_id =
            egui_renderer.register_native_texture(&device, &placeholder.view, wgpu::FilterMode::Linear);
        Ok(Gui {
            state: AppState::new(doc),
            ctx,
            device,
            queue,
            egui_renderer,
            gpu,
            viewport: Some(placeholder),
            viewport_id,
            adapter_info,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Name of the graphics adapter, for diagnostics.
    pub fn adapter_info(&self) -> &str {
        &self.adapter_info
    }

    fn ensure_viewport(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if self.viewport.as_ref().is_none_or(|v| v.size != size) {
            let tex = create_viewport_texture(&self.device, size);
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &tex.view,
                wgpu::FilterMode::Linear,
                self.viewport_id,
            );
            self.viewport = Some(tex);
        }
    }

    fn sync_scene(&mut self) {
        let doc = self.state.display_doc();
        let rev = self.state.display_revision();
        self.gpu
            .update_scene(&doc.scene, Some(&doc.selection), Some(rev), &self.state.view);
    }

    /// Run the UI for `input`, resolve viewport picks, render the 3D view and
    /// draw everything into `target` (`size_px` large).
    pub fn frame(
        &mut self,
        input: egui::RawInput,
        target: &wgpu::TextureView,
        size_px: [u32; 2],
    ) -> FrameOutput {
        self.state.sync_external();
        let rev_before = self.state.doc.revision();
        let viewport_id = self.viewport_id;
        let mut out = ui::UiOutput::default();
        let mut full = {
            let state = &mut self.state;
            self.ctx.run_ui(input, |ui| {
                out = ui::draw(ui, state, viewport_id);
            })
        };
        let ppp = full.pixels_per_point;

        // egui textures
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }

        // viewport size in physical pixels
        let vp_px = out.viewport.map_or((1, 1), |r| {
            (
                (r.width() * ppp).round().max(1.0) as u32,
                (r.height() * ppp).round().max(1.0) as u32,
            )
        });
        self.ensure_viewport(vp_px);

        // picking against the current scene state
        self.sync_scene();
        let picks = self.state.take_picks();
        if !picks.is_empty() {
            for req in picks {
                let cam = self.state.camera.to_camera();
                match self.gpu.pick(&cam, vp_px, req.pos.x, req.pos.y) {
                    Ok(hit) => self.state.apply_pick(hit, req.shift),
                    Err(e) => self.state.report(&e.into()),
                }
                self.sync_scene();
            }
        }

        // render the 3D view, then the UI on top
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("forge3d frame"),
            });
        if let Some(vp) = &self.viewport {
            let cam = self.state.camera.to_camera();
            self.gpu
                .render(&mut encoder, &vp.view, vp.size, &cam, &self.state.view);
        }
        let paint_jobs = self.ctx.tessellate(full.shapes, ppp);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point: ppp,
        };
        let extra = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let bg = 0.11_f64;
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forge3d ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg,
                            g: bg,
                            b: bg,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }
        self.queue
            .submit(extra.into_iter().chain(Some(encoder.finish())));
        for id in &full.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        full.textures_delta.clear();

        let mut repaint_after = full
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);
        if self.state.doc.revision() != rev_before || self.state.is_dragging() {
            // The panels were drawn before the change: draw again.
            repaint_after = Duration::ZERO;
        }
        FrameOutput {
            platform_output: full.platform_output,
            repaint_after,
        }
    }

    /// Draw `frames` UI frames of a `size_px` window without any windowing
    /// system and return the last one as tightly packed RGBA8 bytes.
    pub fn render_offscreen(
        &mut self,
        size_px: [u32; 2],
        pixels_per_point: f32,
        frames: u32,
    ) -> Result<Vec<u8>, AppError> {
        let target = OffscreenTarget::new(&self.device, VIEWPORT_FORMAT, size_px);
        for i in 0..frames.max(1) {
            let input = headless_input(size_px, pixels_per_point, i as f64 / 60.0);
            self.frame(input, &target.view, size_px);
        }
        target.read_rgba(&self.device, &self.queue)
    }
}

/// `RawInput` for a window-less frame.
pub fn headless_input(size_px: [u32; 2], ppp: f32, time: f64) -> egui::RawInput {
    let rect = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(size_px[0] as f32 / ppp, size_px[1] as f32 / ppp),
    );
    let mut input = egui::RawInput {
        screen_rect: Some(rect),
        time: Some(time),
        focused: true,
        ..Default::default()
    };
    input.viewports.insert(
        ViewportId::ROOT,
        egui::ViewportInfo {
            native_pixels_per_point: Some(ppp),
            focused: Some(true),
            ..Default::default()
        },
    );
    input
}

fn create_viewport_texture(device: &wgpu::Device, size: (u32, u32)) -> ViewportTexture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("forge3d viewport"),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: VIEWPORT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    ViewportTexture {
        view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
        size,
    }
}

/// An offscreen color target that can be read back.
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: [u32; 2],
}

impl OffscreenTarget {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge3d offscreen target"),
            size: wgpu::Extent3d {
                width: size[0].max(1),
                height: size[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        OffscreenTarget {
            texture,
            view,
            format,
            size,
        }
    }

    /// Copy the texture to the CPU as RGBA8 (blocking).
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<u8>, AppError> {
        let swap_rb = match self.format {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            other => {
                return Err(AppError::Screenshot(format!(
                    "unsupported readback format {other:?}"
                )))
            }
        };
        let (w, h) = (self.size[0].max(1), self.size[1].max(1));
        let unpadded = w * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("forge3d readback"),
            size: padded as u64 * h as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("forge3d readback"),
        });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| AppError::Screenshot(e.to_string()))?;
        rx.recv()
            .map_err(|e| AppError::Screenshot(e.to_string()))?
            .map_err(|e| AppError::Screenshot(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| AppError::Screenshot(format!("{e:?}")))?;
        let mut out = Vec::with_capacity((unpadded * h) as usize);
        for row in data.chunks(padded as usize).take(h as usize) {
            out.extend_from_slice(&row[..unpadded as usize]);
        }
        drop(data);
        buffer.unmap();
        if swap_rb {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }
}

/// Encode RGBA8 pixels as a PNG file.
pub fn save_png(path: &std::path::Path, rgba: &[u8], size: [u32; 2]) -> Result<(), AppError> {
    let file = std::fs::File::create(path)
        .map_err(|e| AppError::Screenshot(format!("{}: {e}", path.display())))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size[0], size[1]);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc
        .write_header()
        .map_err(|e| AppError::Screenshot(e.to_string()))?;
    w.write_image_data(rgba)
        .map_err(|e| AppError::Screenshot(e.to_string()))?;
    Ok(())
}

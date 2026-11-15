//! wgpu renderer (feature `gpu`).
//!
//! * [`GpuRenderer`] – viewport renderer for an existing device/queue,
//! * [`try_create_headless_device`] – a device without a window (`None`
//!   when no adapter exists),
//! * [`render_png_gpu`] / [`render_png_auto`] – headless PNG rendering,
//!   the latter falling back to the software rasterizer.

mod renderer;
mod uniforms;

use std::sync::{Mutex, OnceLock};

use forge3d_core::scene::Scene;
use forge3d_core::selection::Selection;

pub use renderer::{
    mesh_fingerprint, GpuRenderer, GpuRendererConfig, SceneUpdateStats, DEPTH_FORMAT,
};

use crate::camera::CameraSpec;
use crate::error::{RenderError, RenderResult};
use crate::options::RenderOptions;
use crate::RenderBackend;

/// A device created without a window.
#[derive(Debug, Clone)]
pub struct HeadlessGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
}

/// Create a device without a surface. Tries a regular adapter first, then a
/// fallback (software) adapter. Returns `None` when no adapter exists
/// (e.g. no GPU and no software Vulkan/GL driver installed).
///
/// Blocks the calling thread; call it from a blocking context.
pub fn try_create_headless_device() -> Option<HeadlessGpu> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = [false, true].into_iter().find_map(|fallback| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()
    })?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("forge3d headless device"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    Some(HeadlessGpu {
        device,
        queue,
        adapter_info: adapter.get_info(),
    })
}

/// Render a scene to PNG with a GPU device (offscreen, 4× MSAA plus
/// `options.supersample`).
pub fn render_png_gpu(
    gpu: &HeadlessGpu,
    scene: &Scene,
    selection: Option<&Selection>,
    camera: &CameraSpec,
    options: &RenderOptions,
) -> RenderResult<Vec<u8>> {
    options.validate()?;
    let cam = camera.resolve(scene, options.aspect())?;
    let mut r = GpuRenderer::new(
        &gpu.device,
        &gpu.queue,
        GpuRendererConfig::new(wgpu::TextureFormat::Rgba8UnormSrgb),
    )?;
    r.render_scene_to_rgba(scene, selection, &cam, options)?
        .to_png()
}

struct SharedGpu {
    renderer: Mutex<GpuRenderer>,
}

fn shared_gpu() -> Option<&'static SharedGpu> {
    static GPU: OnceLock<Option<SharedGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let gpu = try_create_headless_device()?;
        let renderer = GpuRenderer::new(
            &gpu.device,
            &gpu.queue,
            GpuRendererConfig::new(wgpu::TextureFormat::Rgba8UnormSrgb),
        )
        .ok()?;
        Some(SharedGpu {
            renderer: Mutex::new(renderer),
        })
    })
    .as_ref()
}

/// Render to PNG on the GPU when an adapter is available (a process-wide
/// headless device and renderer are created on first use), otherwise — or
/// when the GPU path fails — with the software rasterizer. Returns the
/// backend that produced the image. Blocking.
pub fn render_png_auto(
    scene: &Scene,
    selection: Option<&Selection>,
    camera: &CameraSpec,
    options: &RenderOptions,
) -> RenderResult<(Vec<u8>, RenderBackend)> {
    options.validate()?;
    let cam = camera.resolve(scene, options.aspect())?;
    if let Some(shared) = shared_gpu() {
        let result = match shared.renderer.lock() {
            Ok(mut r) => r.render_scene_to_rgba(scene, selection, &cam, options),
            Err(_) => Err(RenderError::Gpu("renderer lock poisoned".into())),
        };
        if let Ok(img) = result {
            return Ok((img.to_png()?, RenderBackend::Gpu));
        }
    }
    let png = crate::soft::render_scene(scene, selection, &cam, options)?.to_png()?;
    Ok((png, RenderBackend::Software))
}

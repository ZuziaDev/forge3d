//! # forge3d-render
//!
//! Rendering for the forge3d modeler.
//!
//! Always available (no GPU needed):
//! * [`camera`] – [`Camera`], [`OrbitCamera`] (Z-up turntable) and the
//!   serializable [`CameraSpec`] for API layers,
//! * [`extract`] – [`SceneDrawData`]: scene + selection → triangles with
//!   normals / material / node id / face id, real mesh edges and highlights,
//! * [`soft`] – a parallel CPU software rasterizer producing [`RgbaImage`]s
//!   (PBR shading, ground grid, wireframe, selection highlight, supersampling),
//! * [`pick`] – CPU ray-cast picking returning node, face and the nearest
//!   vertex / edge of the face,
//! * [`render_png`] – one call from scene to PNG bytes.
//!
//! With the `gpu` feature (default): [`gpu`] – a wgpu viewport renderer
//! with ID-buffer picking and offscreen rendering; [`wgpu`] is re-exported.
//!
//! ```
//! use forge3d_core::prelude::*;
//! use forge3d_render::{render_png, CameraSpec, RenderOptions};
//!
//! let mut doc = Document::new();
//! doc.execute_json(r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#, Origin::Ai)
//!     .unwrap();
//! let opts = RenderOptions::with_size(160, 120);
//! let png = render_png(&doc.scene, Some(&doc.selection), &CameraSpec::default(), &opts).unwrap();
//! assert_eq!(&png[1..4], b"PNG");
//! ```

#![forbid(unsafe_code)]

pub mod camera;
pub mod error;
pub mod extract;
pub mod image;
pub mod options;
pub mod pick;
pub mod shaders;
pub mod shading;
pub mod soft;

#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
pub use wgpu;

pub use camera::{Camera, CameraSpec, OrbitCamera, Projection, Ray, ViewPreset};
pub use error::{RenderError, RenderResult};
pub use extract::{DrawObject, MeshGeometry, SceneDrawData};
pub use image::{encode_png, RgbaImage};
pub use options::{RenderOptions, ViewOptions};
pub use pick::PickHit;

use forge3d_core::scene::Scene;
use forge3d_core::selection::Selection;

/// Which renderer produced an image.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RenderBackend {
    /// The CPU software rasterizer.
    Software,
    /// A wgpu adapter (name in the backend info of the GPU module).
    Gpu,
}

/// Render a scene with the software rasterizer (works without any GPU).
pub fn render_image(
    scene: &Scene,
    selection: Option<&Selection>,
    camera: &CameraSpec,
    options: &RenderOptions,
) -> RenderResult<RgbaImage> {
    options.validate()?;
    let cam = camera.resolve(scene, options.aspect())?;
    soft::render_scene(scene, selection, &cam, options)
}

/// Render a scene to PNG bytes with the software rasterizer (works without any GPU).
pub fn render_png(
    scene: &Scene,
    selection: Option<&Selection>,
    camera: &CameraSpec,
    options: &RenderOptions,
) -> RenderResult<Vec<u8>> {
    render_image(scene, selection, camera, options)?.to_png()
}

/// Pick the object / face under a pixel of a `width` × `height` view
/// (CPU ray cast; `x`, `y` in pixels from the top-left).
pub fn pick_scene(
    scene: &Scene,
    camera: &Camera,
    width: u32,
    height: u32,
    x: f32,
    y: f32,
) -> Option<PickHit> {
    let draw = SceneDrawData::extract(scene, None, false);
    pick::pick(&draw, camera, width, height, x, y)
}

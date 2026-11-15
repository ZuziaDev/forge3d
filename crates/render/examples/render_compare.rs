//! Render the same demo scene with the software rasterizer and (when an
//! adapter exists) the GPU renderer, for visual comparison.
//!
//! `cargo run -p forge3d-render --example render_compare -- out_dir`

use forge3d_core::prelude::*;
use forge3d_core::primitives;
use forge3d_render::{soft, CameraSpec, RenderOptions};
use glam::{Affine3A, Vec3};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Ai,
    )?;
    let cube = doc.selection.active().ok_or("no cube")?;
    let mut n = Node::new("Torus").with_mesh(primitives::torus(1.0, 0.3, 32, 12)?);
    n.transform = Affine3A::from_translation(Vec3::new(2.8, 0.5, 0.3));
    n.material = Some(Material {
        base_color: [0.9, 0.6, 0.2, 1.0],
        metallic: 1.0,
        roughness: 0.35,
        ..Material::default()
    });
    doc.scene.add_node(n, None)?;
    let mut sel = doc.selection.clone();
    sel.mode = SelectMode::Vertex;
    sel.elements_mut(cube).vertices.extend([0, 1]);
    let mut opts = RenderOptions::with_size(640, 480);
    opts.view.wireframe = true;
    let cam = CameraSpec::default().resolve(&doc.scene, opts.aspect())?;
    for (name, s) in [("object", &doc.selection), ("vertex", &sel)] {
        let img = soft::render_scene(&doc.scene, Some(s), &cam, &opts)?;
        std::fs::write(out.join(format!("soft_{name}.png")), img.to_png()?)?;
        #[cfg(feature = "gpu")]
        if let Some(g) = forge3d_render::gpu::try_create_headless_device() {
            let mut r = forge3d_render::gpu::GpuRenderer::new(
                &g.device,
                &g.queue,
                forge3d_render::gpu::GpuRendererConfig::new(
                    forge3d_render::wgpu::TextureFormat::Rgba8UnormSrgb,
                ),
            )?;
            let img = r.render_scene_to_rgba(&doc.scene, Some(s), &cam, &opts)?;
            std::fs::write(out.join(format!("gpu_{name}.png")), img.to_png()?)?;
        }
    }
    println!("wrote images to {}", out.display());
    Ok(())
}

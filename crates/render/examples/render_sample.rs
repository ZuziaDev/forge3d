//! Render a small demo scene with the software rasterizer and write a PNG.
//!
//! `cargo run -p forge3d-render --example render_sample --release -- out.png [width height]`

use forge3d_core::prelude::*;
use forge3d_core::primitives;
use forge3d_render::{render_png, CameraSpec, RenderOptions};
use glam::{Affine3A, Quat, Vec3};

fn add(doc: &mut Scene, name: &str, mesh: Mesh, t: Affine3A, m: Material) -> uuid::Uuid {
    let mut n = Node::new(name).with_mesh(mesh).with_transform(t);
    n.material = Some(m);
    doc.add_node(n, None).expect("add node")
}

fn mat(name: &str, rgb: [f32; 3], metallic: f32, roughness: f32) -> Material {
    Material {
        name: name.into(),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic,
        roughness,
        emissive: [0.0; 3],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "render_sample.png".into());
    let width: u32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(1024);
    let height: u32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(768);

    let mut scene = Scene::new();
    let cube = add(
        &mut scene,
        "Cube",
        primitives::cube(1.6)?,
        Affine3A::from_rotation_translation(Quat::from_rotation_z(0.4), Vec3::new(-2.2, 0.0, 0.8)),
        mat("Red plastic", [0.75, 0.08, 0.06], 0.0, 0.45),
    );
    add(
        &mut scene,
        "Sphere",
        primitives::uv_sphere(1.0, 48, 24)?,
        Affine3A::from_translation(Vec3::new(0.3, 0.6, 1.0)),
        mat("Gold", [1.0, 0.76, 0.33], 1.0, 0.3),
    );
    add(
        &mut scene,
        "Torus",
        primitives::torus(0.9, 0.3, 48, 16)?,
        Affine3A::from_translation(Vec3::new(2.6, -0.4, 0.3)),
        mat("Blue", [0.1, 0.3, 0.8], 0.0, 0.6),
    );
    add(
        &mut scene,
        "Cylinder",
        primitives::cylinder(0.5, 2.0, 32)?,
        Affine3A::from_translation(Vec3::new(0.8, 3.0, 1.0)),
        mat("Steel", [0.6, 0.62, 0.65], 1.0, 0.5),
    );
    let mut selection = Selection::new();
    selection.set_objects([cube]);

    let mut opts = RenderOptions::with_size(width, height);
    opts.view.wireframe = false;
    let png = render_png(&scene, Some(&selection), &CameraSpec::default(), &opts)?;
    std::fs::write(&out, png)?;
    println!("wrote {out}");
    Ok(())
}

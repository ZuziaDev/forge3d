//! GPU renderer tests. They need a wgpu adapter (hardware or a software
//! driver such as lavapipe/llvmpipe); without one they print a notice and
//! return early.
#![cfg(feature = "gpu")]

use forge3d_core::prelude::*;
use forge3d_core::primitives;
use forge3d_render::camera::Camera;
use forge3d_render::gpu::{
    render_png_auto, render_png_gpu, try_create_headless_device, GpuRenderer, GpuRendererConfig,
    HeadlessGpu,
};
use forge3d_render::{
    soft, wgpu, CameraSpec, OrbitCamera, RenderBackend, RenderOptions, RgbaImage,
};
use glam::{Affine3A, Vec3};

fn gpu() -> Option<HeadlessGpu> {
    let g = try_create_headless_device();
    match &g {
        Some(g) => eprintln!(
            "GPU adapter: {} ({:?}, {:?})",
            g.adapter_info.name, g.adapter_info.backend, g.adapter_info.device_type
        ),
        None => eprintln!("NOTICE: no wgpu adapter available; skipping GPU test"),
    }
    g
}

fn add(scene: &mut Scene, name: &str, mesh: Mesh, at: Vec3, rgb: [f32; 3]) -> uuid::Uuid {
    let mut n = Node::new(name)
        .with_mesh(mesh)
        .with_transform(Affine3A::from_translation(at));
    n.material = Some(Material {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        ..Material::default()
    });
    scene.add_node(n, None).unwrap()
}

fn cube_scene() -> (Scene, uuid::Uuid) {
    let mut s = Scene::new();
    let id = add(
        &mut s,
        "Cube",
        primitives::cube(2.0).unwrap(),
        Vec3::ZERO,
        [0.8, 0.1, 0.1],
    );
    (s, id)
}

fn front_camera() -> Camera {
    Camera::look_at(Vec3::new(0.0, -8.0, 0.0), Vec3::ZERO, Vec3::Z, 40.0)
}

fn opts(w: u32, h: u32) -> RenderOptions {
    let mut o = RenderOptions::with_size(w, h);
    o.view.grid = false;
    o.supersample = 1;
    o.view.background = [0.0, 0.0, 1.0, 1.0];
    o
}

fn renderer(g: &HeadlessGpu, format: wgpu::TextureFormat) -> GpuRenderer {
    GpuRenderer::new(&g.device, &g.queue, GpuRendererConfig::new(format)).unwrap()
}

fn mean_abs_diff(a: &RgbaImage, b: &RgbaImage) -> f32 {
    let s: u64 = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    s as f32 / a.pixels.len() as f32
}

#[test]
fn offscreen_render_cube_and_parity_with_software() {
    let Some(g) = gpu() else { return };
    let (s, _) = cube_scene();
    let mut r = renderer(&g, wgpu::TextureFormat::Rgba8UnormSrgb);
    let o = opts(64, 48);
    let img = r
        .render_scene_to_rgba(&s, None, &front_camera(), &o)
        .unwrap();
    assert_eq!((img.width, img.height), (64, 48));
    let c = img.pixel(32, 24);
    assert!(
        c[0] > 60 && c[0] > c[1] * 2 && c[0] > c[2] * 2,
        "red-ish center {c:?}"
    );
    for (x, y) in [(0, 0), (63, 0), (0, 47), (63, 47)] {
        assert_eq!(img.pixel(x, y), [0, 0, 255, 255], "corner {x},{y}");
    }
    let cpu = soft::render_scene(&s, None, &front_camera(), &o).unwrap();
    let d = mean_abs_diff(&img, &cpu);
    eprintln!("GPU vs software mean abs diff per channel: {d:.2}");
    assert!(
        d < 6.0,
        "GPU and software renders should match closely: {d}"
    );
    // Shading of the center pixel matches the CPU model.
    let cc = cpu.pixel(32, 24);
    for k in 0..3 {
        assert!((c[k] as i32 - cc[k] as i32).abs() <= 4, "{c:?} vs {cc:?}");
    }
}

#[test]
fn id_buffer_picking() {
    let Some(g) = gpu() else { return };
    let mut s = Scene::new();
    let a = add(
        &mut s,
        "A",
        primitives::uv_sphere(1.0, 16, 8).unwrap(),
        Vec3::new(-2.0, 0.0, 0.0),
        [1.0, 0.0, 0.0],
    );
    let b = add(
        &mut s,
        "B",
        primitives::cube(1.5).unwrap(),
        Vec3::new(2.0, 0.0, 0.0),
        [0.0, 1.0, 0.0],
    );
    let cam = Camera::look_at(Vec3::new(0.0, -10.0, 0.0), Vec3::ZERO, Vec3::Z, 45.0);
    let mut r = renderer(&g, wgpu::TextureFormat::Rgba8UnormSrgb);
    r.update_scene(&s, None, Some(1), &Default::default());
    let size = (160, 100);
    let pa = cam
        .project_to_pixel(Vec3::new(-2.0, 0.0, 0.0), size.0, size.1)
        .unwrap();
    let pb = cam
        .project_to_pixel(Vec3::new(2.0, -0.75, 0.3), size.0, size.1)
        .unwrap();
    let ha = r.pick(&cam, size, pa.x, pa.y).unwrap().expect("hit sphere");
    assert_eq!(ha.node_id, a);
    let hb = r.pick(&cam, size, pb.x, pb.y).unwrap().expect("hit cube");
    assert_eq!(hb.node_id, b);
    let mesh = s.mesh(b).unwrap();
    assert!(mesh
        .face_normal(FaceId(hb.face))
        .abs_diff_eq(Vec3::NEG_Y, 1e-4));
    assert!(
        (hb.position[1] + 0.75).abs() < 0.05,
        "reconstructed hit {:?}",
        hb.position
    );
    assert!(mesh
        .face_vertices(FaceId(hb.face))
        .any(|v| v.0 == hb.vertex));
    assert!(mesh.face_edges(FaceId(hb.face)).any(|e| e.0 == hb.edge));
    // Same answer as the CPU ray cast.
    let cpu = forge3d_render::pick_scene(&s, &cam, size.0, size.1, pb.x, pb.y).unwrap();
    assert_eq!((cpu.node_id, cpu.face), (hb.node_id, hb.face));
    // Background.
    assert!(r.pick(&cam, size, 2.0, 2.0).unwrap().is_none());
    assert!(r.pick(&cam, size, -1.0, 5.0).unwrap().is_none());
}

#[test]
fn mesh_cache_rebuilds_only_changed_nodes() {
    let Some(g) = gpu() else { return };
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Ai,
    )
    .unwrap();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"uv_sphere"}}"#,
        Origin::Ai,
    )
    .unwrap();
    let mut r = renderer(&g, wgpu::TextureFormat::Rgba8UnormSrgb);
    let view = Default::default();
    let st = r.update_scene(
        &doc.scene,
        Some(&doc.selection),
        Some(doc.revision()),
        &view,
    );
    assert_eq!((st.uploaded, st.objects), (2, 2));
    let st = r.update_scene(
        &doc.scene,
        Some(&doc.selection),
        Some(doc.revision()),
        &view,
    );
    assert!(st.skipped);
    // Transform-only change: no re-upload.
    let sphere = doc.selection.active().unwrap();
    doc.scene.node_mut(sphere).unwrap().transform = Affine3A::from_translation(Vec3::X * 3.0);
    let st = r.update_scene(
        &doc.scene,
        Some(&doc.selection),
        Some(doc.revision() + 100),
        &view,
    );
    assert_eq!((st.uploaded, st.reused), (0, 2));
    // Geometry change on one node.
    doc.execute_json(r#"{"op":"extrude","faces":[1],"distance":0.5}"#, Origin::Ai)
        .unwrap();
    let st = r.update_scene(
        &doc.scene,
        Some(&doc.selection),
        Some(doc.revision()),
        &view,
    );
    assert_eq!((st.uploaded, st.reused), (1, 1));
    // Deleting a node drops its cache entry.
    let ids: Vec<_> = doc.scene.roots().to_vec();
    doc.scene.remove_subtree(ids[0]).unwrap();
    let st = r.update_scene(&doc.scene, None, None, &view);
    assert_eq!((st.removed, r.cached_mesh_count()), (1, 1));
    let cam = OrbitCamera::default().to_camera();
    r.render_to_rgba(&cam, &opts(32, 32)).unwrap();
}

#[test]
fn selection_outline_wireframe_and_grid_on_gpu() {
    let Some(g) = gpu() else { return };
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Ai,
    )
    .unwrap();
    let id = doc.selection.active().unwrap();
    let cam = OrbitCamera::new(Vec3::ZERO, 9.0, -60.0, 30.0).to_camera();
    let mut r = renderer(&g, wgpu::TextureFormat::Rgba8UnormSrgb);
    let o = opts(96, 72);
    let none = r.render_scene_to_rgba(&doc.scene, None, &cam, &o).unwrap();
    let sel = r
        .render_scene_to_rgba(&doc.scene, Some(&doc.selection), &cam, &o)
        .unwrap();
    let orange = sel
        .pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[2] < 150)
        .count();
    assert!(orange > 30, "outline pixels {orange}");
    assert!(mean_abs_diff(&none, &sel) > 0.5);
    let mut w = o;
    w.view.wireframe = true;
    w.view.grid = true;
    let wire = r.render_scene_to_rgba(&doc.scene, None, &cam, &w).unwrap();
    assert!(mean_abs_diff(&none, &wire) > 1.0);
    let cpu_wire = soft::render_scene(&doc.scene, None, &cam, &w).unwrap();
    let d = mean_abs_diff(&wire, &cpu_wire);
    eprintln!("GPU vs software (grid + wireframe) mean abs diff: {d:.2}");
    assert!(d < 12.0, "{d}");

    // Edit modes: vertex points and selected face tint.
    let mut s2 = doc.selection.clone();
    s2.mode = SelectMode::Vertex;
    s2.elements_mut(id).vertices.insert(0);
    let v = r
        .render_scene_to_rgba(&doc.scene, Some(&s2), &cam, &o)
        .unwrap();
    assert!(mean_abs_diff(&none, &v) > 1.0);
    s2.mode = SelectMode::Face;
    s2.elements_mut(id).faces.insert(0);
    let f = r
        .render_scene_to_rgba(&doc.scene, Some(&s2), &cam, &o)
        .unwrap();
    assert!(mean_abs_diff(&none, &f) > 1.0);
}

#[test]
fn non_srgb_target_and_supersampling() {
    let Some(g) = gpu() else { return };
    let (s, _) = cube_scene();
    let mut a = renderer(&g, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut b = renderer(&g, wgpu::TextureFormat::Bgra8Unorm);
    let o = opts(48, 48);
    let ia = a
        .render_scene_to_rgba(&s, None, &front_camera(), &o)
        .unwrap();
    let ib = b
        .render_scene_to_rgba(&s, None, &front_camera(), &o)
        .unwrap();
    let c1 = ia.pixel(24, 24);
    let c2 = ib.pixel(24, 24);
    for k in 0..3 {
        assert!((c1[k] as i32 - c2[k] as i32).abs() <= 3, "{c1:?} vs {c2:?}");
    }
    let mut ss = o;
    ss.supersample = 2;
    let iss = a
        .render_scene_to_rgba(&s, None, &front_camera(), &ss)
        .unwrap();
    assert_eq!((iss.width, iss.height), (48, 48));
    // An unsupported readback format is a structured error.
    let mut f = renderer(&g, wgpu::TextureFormat::Rgba16Float);
    let e = f
        .render_scene_to_rgba(&s, None, &front_camera(), &o)
        .unwrap_err();
    assert_eq!(e.code(), "unsupported_format");
}

#[test]
fn headless_png_helpers() {
    let (s, _) = cube_scene();
    let o = RenderOptions::with_size(40, 30);
    let (png, backend) = render_png_auto(&s, None, &CameraSpec::default(), &o).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    eprintln!("render_png_auto used {backend:?}");
    let Some(g) = gpu() else {
        assert_eq!(backend, RenderBackend::Software);
        return;
    };
    assert_eq!(backend, RenderBackend::Gpu);
    let png = render_png_gpu(&g, &s, None, &CameraSpec::default(), &o).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

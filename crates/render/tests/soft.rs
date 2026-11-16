//! Software rasterizer, picking and PNG tests (no GPU needed).

use forge3d_core::prelude::*;
use forge3d_core::primitives;
use forge3d_render::camera::Camera;
use forge3d_render::soft;
use forge3d_render::{
    pick_scene, render_image, render_png, CameraSpec, OrbitCamera, RenderOptions, SceneDrawData,
};
use glam::{Affine3A, Vec3};

fn material(rgb: [f32; 3]) -> Material {
    Material {
        name: "m".into(),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic: 0.0,
        roughness: 0.6,
        emissive: [0.0; 3],
    }
}

fn add(scene: &mut Scene, name: &str, mesh: Mesh, at: Vec3, rgb: [f32; 3]) -> uuid::Uuid {
    let mut n = Node::new(name)
        .with_mesh(mesh)
        .with_transform(Affine3A::from_translation(at));
    n.material = Some(material(rgb));
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

fn is_bg(p: [u8; 4]) -> bool {
    p == [0, 0, 255, 255]
}

#[test]
fn cube_center_is_object_corners_are_background() {
    let (s, _) = cube_scene();
    let img = soft::render_scene(&s, None, &front_camera(), &opts(64, 48)).unwrap();
    assert_eq!((img.width, img.height), (64, 48));
    let c = img.pixel(32, 24);
    assert!(
        c[0] > 60 && c[0] > c[1] * 2 && c[0] > c[2] * 2,
        "red-ish: {c:?}"
    );
    for (x, y) in [(0, 0), (63, 0), (0, 47), (63, 47)] {
        assert!(is_bg(img.pixel(x, y)), "corner {x},{y}");
    }
    let covered = (0..48)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .filter(|&(x, y)| !is_bg(img.pixel(x, y)))
        .count();
    assert!(covered > 200, "cube covers part of the image: {covered}");
}

#[test]
fn depth_ordering_nearer_object_wins() {
    let mut s = Scene::new();
    // Green cube in front (towards the camera at -Y), red behind.
    add(
        &mut s,
        "Back",
        primitives::cube(4.0).unwrap(),
        Vec3::new(0.0, 4.0, 0.0),
        [0.9, 0.0, 0.0],
    );
    add(
        &mut s,
        "Front",
        primitives::cube(1.0).unwrap(),
        Vec3::ZERO,
        [0.0, 0.9, 0.0],
    );
    let img = soft::render_scene(&s, None, &front_camera(), &opts(80, 60)).unwrap();
    let c = img.pixel(40, 30);
    assert!(c[1] > c[0] * 2, "front object visible: {c:?}");
    // Swap insertion order: result must not depend on draw order.
    let mut s2 = Scene::new();
    add(
        &mut s2,
        "Front",
        primitives::cube(1.0).unwrap(),
        Vec3::ZERO,
        [0.0, 0.9, 0.0],
    );
    add(
        &mut s2,
        "Back",
        primitives::cube(4.0).unwrap(),
        Vec3::new(0.0, 4.0, 0.0),
        [0.9, 0.0, 0.0],
    );
    let img2 = soft::render_scene(&s2, None, &front_camera(), &opts(80, 60)).unwrap();
    assert_eq!(img2.pixel(40, 30), c);
    // The larger red cube is visible around the green one.
    let side = img.pixel(40 + 11, 30);
    assert!(side[0] > side[1] * 2, "back object around: {side:?}");
}

#[test]
fn picking_returns_node_and_face() {
    let (s, id) = cube_scene();
    let cam = front_camera();
    let hit = pick_scene(&s, &cam, 64, 48, 32.0, 24.0).expect("hit");
    assert_eq!(hit.node_id, id);
    let mesh = s.mesh(id).unwrap();
    let n = mesh.face_normal(FaceId(hit.face));
    assert!(
        n.abs_diff_eq(Vec3::NEG_Y, 1e-4),
        "front face (-Y) normal {n}"
    );
    assert!((hit.position[1] + 1.0).abs() < 1e-3);
    assert!((hit.distance - 7.0).abs() < 1e-3);
    // Vertex and edge belong to the face.
    assert!(mesh
        .face_vertices(FaceId(hit.face))
        .any(|v| v.0 == hit.vertex));
    assert!(mesh.face_edges(FaceId(hit.face)).any(|e| e.0 == hit.edge));
    assert!(pick_scene(&s, &cam, 64, 48, 1.0, 1.0).is_none());
}

#[test]
fn picking_nearest_vertex_and_edge() {
    let (s, id) = cube_scene();
    let cam = front_camera();
    // Pixel near the upper-left corner of the front face.
    let corner = cam
        .project_to_pixel(Vec3::new(-1.0, -1.0, 1.0), 200, 200)
        .unwrap();
    let hit = pick_scene(&s, &cam, 200, 200, corner.x + 3.0, corner.y + 3.0).unwrap();
    let mesh = s.mesh(id).unwrap();
    let p = mesh.position(VertexId(hit.vertex));
    assert!(p.abs_diff_eq(Vec3::new(-1.0, -1.0, 1.0), 1e-5), "{p}");
    // Near the middle of the top edge of the front face.
    let top = cam
        .project_to_pixel(Vec3::new(0.0, -1.0, 1.0), 200, 200)
        .unwrap();
    let hit = pick_scene(&s, &cam, 200, 200, top.x, top.y + 2.0).unwrap();
    let m = mesh.edge_midpoint(EdgeId(hit.edge));
    assert!(m.abs_diff_eq(Vec3::new(0.0, -1.0, 1.0), 1e-5), "{m}");
}

#[test]
fn draw_data_pick_matches_rendered_object() {
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
    let draw = SceneDrawData::extract(&s, None, false);
    let pa = cam
        .project_to_pixel(Vec3::new(-2.0, 0.0, 0.0), 120, 80)
        .unwrap();
    let pb = cam
        .project_to_pixel(Vec3::new(2.0, 0.0, 0.0), 120, 80)
        .unwrap();
    assert_eq!(
        soft::pick(&draw, &cam, 120, 80, pa.x, pa.y)
            .unwrap()
            .node_id,
        a
    );
    assert_eq!(
        soft::pick(&draw, &cam, 120, 80, pb.x, pb.y)
            .unwrap()
            .node_id,
        b
    );
}

fn diff_count(a: &forge3d_render::RgbaImage, b: &forge3d_render::RgbaImage) -> usize {
    a.pixels
        .chunks(4)
        .zip(b.pixels.chunks(4))
        .filter(|(x, y)| x != y)
        .count()
}

#[test]
fn grid_and_wireframe_toggles_change_output() {
    let (s, _) = cube_scene();
    let cam = OrbitCamera::new(Vec3::ZERO, 9.0, -60.0, 30.0).to_camera();
    let base = opts(96, 72);
    let plain = soft::render_scene(&s, None, &cam, &base).unwrap();

    let mut g = base;
    g.view.grid = true;
    let grid = soft::render_scene(&s, None, &cam, &g).unwrap();
    assert!(diff_count(&plain, &grid) > 200, "grid draws lines");
    // Grid pixels are only drawn outside the object (cube sits on/through z=0 but the
    // center of the cube is in front of the grid).
    assert_eq!(plain.pixel(48, 36), grid.pixel(48, 36));

    let mut w = base;
    w.view.wireframe = true;
    let wire = soft::render_scene(&s, None, &cam, &w).unwrap();
    let d = diff_count(&plain, &wire);
    assert!(d > 50, "wireframe draws edges: {d}");
}

#[test]
fn wireframe_uses_real_edges_not_diagonals() {
    // A single quad seen head-on: wireframe must not draw the triangulation diagonal.
    let mut s = Scene::new();
    let mut n = Node::new("Plane").with_mesh(primitives::plane(2.0, 1).unwrap());
    n.material = Some(material([1.0, 1.0, 1.0]));
    s.add_node(n, None).unwrap();
    let cam = Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y, 40.0);
    let mut o = opts(100, 100);
    o.view.wireframe = true;
    let wire = soft::render_scene(&s, None, &cam, &o).unwrap();
    o.view.wireframe = false;
    let plain = soft::render_scene(&s, None, &cam, &o).unwrap();
    // Center of the quad (on any diagonal) and points on both diagonals are unchanged.
    for (x, y) in [(50, 50), (40, 40), (60, 40), (40, 60), (60, 60)] {
        assert_eq!(
            wire.pixel(x, y),
            plain.pixel(x, y),
            "diagonal pixel {x},{y}"
        );
    }
    // A point on the left border edge is darkened.
    let corner = cam
        .project_to_pixel(Vec3::new(-1.0, 0.0, 0.0), 100, 100)
        .unwrap();
    let (x, y) = (corner.x.round() as u32, corner.y as u32);
    let pw = wire.pixel(x, y);
    let pp = plain.pixel(x + 2, y);
    assert!(pw[0] < pp[0], "edge darker: {pw:?} vs {pp:?}");
}

#[test]
fn selection_outline_and_edit_highlights() {
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Ai,
    )
    .unwrap();
    let id = doc.selection.active().unwrap();
    let cam = OrbitCamera::new(Vec3::ZERO, 9.0, -60.0, 30.0).to_camera();
    let o = opts(96, 72);
    let none = soft::render_scene(&doc.scene, None, &cam, &o).unwrap();
    let sel = soft::render_scene(&doc.scene, Some(&doc.selection), &cam, &o).unwrap();
    let d = diff_count(&none, &sel);
    assert!(d > 100, "outline + tint: {d}");
    // Some outline pixels are orange (strong red, little blue) on the blue background.
    let orange = sel
        .pixels
        .chunks(4)
        .filter(|p| p[0] > 200 && p[2] < 150)
        .count();
    assert!(orange > 30, "orange outline pixels: {orange}");

    // Face mode with one selected face tints that face.
    let mut selection = doc.selection.clone();
    selection.mode = SelectMode::Face;
    let mesh = doc.scene.mesh(id).unwrap();
    let front = mesh
        .face_ids()
        .find(|&f| mesh.face_normal(f).abs_diff_eq(Vec3::NEG_Y, 1e-4))
        .unwrap();
    selection.elements_mut(id).faces.insert(front.0);
    let front_cam = front_camera();
    let face_img = soft::render_scene(&doc.scene, Some(&selection), &front_cam, &o).unwrap();
    let plain = soft::render_scene(&doc.scene, None, &front_cam, &o).unwrap();
    let (a, b) = (face_img.pixel(48, 36), plain.pixel(48, 36));
    assert!(a[0] > b[0] && a[2] <= b[2], "orange tint {a:?} vs {b:?}");

    // Vertex mode draws vertex points: more pixels change than with face mode off.
    selection.mode = SelectMode::Vertex;
    let v_img = soft::render_scene(&doc.scene, Some(&selection), &cam, &o).unwrap();
    assert!(diff_count(&none, &v_img) > 100);
}

#[test]
fn png_roundtrip_dimensions() {
    let (s, _) = cube_scene();
    let o = RenderOptions::with_size(73, 41);
    let png = render_png(&s, None, &CameraSpec::default(), &o).unwrap();
    let dec = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (73, 41));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    let img = render_image(&s, None, &CameraSpec::default(), &o).unwrap();
    assert_eq!(&buf[..info.buffer_size()], &img.pixels[..]);
}

#[test]
fn auto_camera_frames_scene_and_empty_scene_renders() {
    let (s, _) = cube_scene();
    let mut o = RenderOptions::with_size(64, 64);
    o.view.grid = false;
    o.supersample = 1;
    let img = render_image(&s, None, &CameraSpec::default(), &o).unwrap();
    // Framed: center covered, every border row/column is background.
    let bg = img.pixel(0, 0);
    assert_ne!(img.pixel(32, 32), bg);
    for i in 0..64 {
        assert_eq!(img.pixel(i, 0), bg);
        assert_eq!(img.pixel(0, i), bg);
    }
    let empty = render_image(&Scene::new(), None, &CameraSpec::default(), &o).unwrap();
    assert!(empty.pixels.chunks(4).all(|p| p == &empty.pixels[..4]));
}

#[test]
fn transparent_background_and_options_validation() {
    let (s, _) = cube_scene();
    let mut o = opts(32, 32);
    o.view.background = [0.0, 0.0, 0.0, 0.0];
    let img = soft::render_scene(&s, None, &front_camera(), &o).unwrap();
    assert_eq!(img.pixel(0, 0)[3], 0);
    assert_eq!(img.pixel(16, 16)[3], 255);

    let mut bad = o;
    bad.width = 0;
    let e = soft::render_scene(&s, None, &front_camera(), &bad).unwrap_err();
    assert_eq!(e.code(), "invalid_render_options");
    assert!(!e.hint().is_empty());
    let mut cam = front_camera();
    cam.target = cam.eye;
    assert_eq!(
        soft::render_scene(&s, None, &cam, &o).unwrap_err().code(),
        "invalid_camera"
    );
    let spec: CameraSpec =
        serde_json::from_str(r#"{"kind":"auto","focus":["00000000-0000-0000-0000-000000000001"]}"#)
            .unwrap();
    assert_eq!(spec.resolve(&s, 1.0).unwrap_err().code(), "node_not_found");
}

#[test]
fn json_options_and_camera_specs() {
    let o: RenderOptions =
        serde_json::from_str(r#"{"width":320,"height":200,"wireframe":true,"samples":3}"#).unwrap();
    assert_eq!((o.width, o.height, o.supersample), (320, 200, 3));
    assert!(o.view.wireframe && o.view.grid);
    let schema = schemars::schema_for!(RenderOptions);
    let js = serde_json::to_string(&schema).unwrap();
    assert!(js.contains("wireframe") && js.contains("supersample"));
    let cam: CameraSpec = serde_json::from_str(
        r#"{"kind":"look_at","eye":[0,-5,2],"target":[0,0,0],"projection":{"type":"perspective","fov_y_deg":60}}"#,
    )
    .unwrap();
    let (s, _) = cube_scene();
    let c = cam.resolve(&s, 1.0).unwrap();
    assert_eq!(c.up, Vec3::Z);
    let orbit: CameraSpec =
        serde_json::from_str(r#"{"kind":"orbit","distance":7,"yaw_deg":0,"pitch_deg":0}"#).unwrap();
    let c = orbit.resolve(&s, 1.0).unwrap();
    assert!(c.eye.abs_diff_eq(Vec3::new(7.0, 0.0, 0.0), 1e-4));
    let top: CameraSpec = serde_json::from_str(r#"{"kind":"auto","view":"top"}"#).unwrap();
    let c = top.resolve(&s, 1.0).unwrap();
    assert!(c.forward().z < -0.99);
    let _ = soft::render_scene(&s, None, &c, &opts(16, 16)).unwrap();
}

#[test]
fn orthographic_and_supersampled_render() {
    let (s, _) = cube_scene();
    let mut orbit = OrbitCamera::new(Vec3::ZERO, 8.0, -90.0, 0.0);
    orbit.orthographic = true;
    let cam = orbit.to_camera();
    let mut o = opts(64, 64);
    o.supersample = 3;
    let img = soft::render_scene(&s, None, &cam, &o).unwrap();
    assert!(!is_bg(img.pixel(32, 32)));
    assert!(is_bg(img.pixel(2, 2)));
    // Orthographic front view of a cube is a square centered in the image.
    let row: Vec<bool> = (0..64).map(|x| !is_bg(img.pixel(x, 32))).collect();
    let first = row.iter().position(|&b| b).unwrap();
    let last = row.iter().rposition(|&b| b).unwrap();
    assert!(
        (first as i32 + last as i32 - 63).abs() <= 2,
        "{first}..{last}"
    );
}

#[test]
fn camera_near_plane_inside_object_does_not_panic() {
    let mut s = Scene::new();
    add(
        &mut s,
        "Big",
        primitives::cube(20.0).unwrap(),
        Vec3::ZERO,
        [0.5, 0.5, 0.5],
    );
    add(
        &mut s,
        "Plane",
        primitives::plane(100.0, 4).unwrap(),
        Vec3::new(0.0, 0.0, -12.0),
        [0.3, 0.6, 0.3],
    );
    let cam = Camera::look_at(
        Vec3::new(0.0, -3.0, 2.0),
        Vec3::new(0.0, 5.0, -1.0),
        Vec3::Z,
        70.0,
    );
    let mut o = opts(64, 48);
    o.view.grid = true;
    o.view.wireframe = true;
    let img = soft::render_scene(&s, None, &cam, &o).unwrap();
    // Inside the cube we see its inner faces everywhere.
    assert!(img.pixels.chunks(4).all(|p| p != [0, 0, 255, 255]));
}

#[test]
fn large_scene_renders_in_reasonable_time() {
    // ~50k triangles: 6 ico spheres (subdiv 4 → 5120 tris each) + a torus 160x60 (19200 tris)
    let mut s = Scene::new();
    for i in 0..6 {
        add(
            &mut s,
            "Ico",
            primitives::ico_sphere(0.8, 4).unwrap(),
            Vec3::new((i % 3) as f32 * 2.0 - 2.0, (i / 3) as f32 * 2.0, 0.8),
            [0.7, 0.5, 0.3],
        );
    }
    add(
        &mut s,
        "Torus",
        primitives::torus(3.0, 0.5, 160, 60).unwrap(),
        Vec3::ZERO,
        [0.3, 0.5, 0.8],
    );
    let draw = SceneDrawData::extract(&s, None, true);
    assert!(draw.triangle_count() >= 49_000, "{}", draw.triangle_count());
    let mut o = RenderOptions::with_size(1024, 768);
    o.supersample = 1;
    o.view.wireframe = true;
    let cam = CameraSpec::default().resolve(&s, o.aspect()).unwrap();
    let t = std::time::Instant::now();
    let img = soft::render(&draw, &cam, &o).unwrap();
    let dt = t.elapsed();
    println!(
        "1024x768, {} tris, wireframe: {dt:?}",
        draw.triangle_count()
    );
    assert_eq!(img.width, 1024);
    // Generous bound: debug builds are much slower than release.
    let limit = if cfg!(debug_assertions) { 60.0 } else { 2.0 };
    assert!(dt.as_secs_f32() < limit, "took {dt:?}");
}

#[test]
fn extraction_shows_evaluated_mesh_except_in_edit_mode() {
    let mut doc = Document::new();
    let id = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    doc.execute_json(
        r#"{"op":"add_modifier","modifier":{"type":"subdivision","levels":1}}"#,
        Origin::Gui,
    )
    .unwrap();
    let draw = SceneDrawData::extract(&doc.scene, Some(&doc.selection), false);
    assert_eq!(draw.objects[0].geometry.triangle_count(), 48);
    doc.execute_json(r#"{"op":"set_select_mode","mode":"face"}"#, Origin::Gui)
        .unwrap();
    let draw = SceneDrawData::extract(&doc.scene, Some(&doc.selection), false);
    assert_eq!(draw.objects[0].geometry.triangle_count(), 12);
    assert_eq!(draw.objects[0].node_id, id);
    // Rendering works with modifiers (soft path).
    let img = render_image(
        &doc.scene,
        None,
        &CameraSpec::default(),
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(img.width > 0);
}

#[test]
fn auto_smooth_keeps_hard_edges_and_smooths_curves() {
    use forge3d_render::extract::MeshGeometry;
    let cube = primitives::cube(2.0).unwrap();
    let hard = MeshGeometry::from_mesh_shaded(&cube, 30.0);
    let flat = MeshGeometry::from_mesh(&cube, false);
    for (a, b) in hard.normals.iter().zip(&flat.normals) {
        assert!(a.abs_diff_eq(*b, 1e-5));
    }
    let smooth = MeshGeometry::from_mesh_shaded(&cube, 180.0);
    assert!(smooth
        .normals
        .iter()
        .all(|n| n.x.abs() > 0.5 && n.z.abs() > 0.5));
    // A sphere at 60 degrees auto smooth equals full smoothing.
    let s = primitives::uv_sphere(1.0, 16, 8).unwrap();
    let a = MeshGeometry::from_mesh_shaded(&s, 60.0);
    let b = MeshGeometry::from_mesh(&s, true);
    for (x, y) in a.normals.iter().zip(&b.normals) {
        assert!(x.abs_diff_eq(*y, 1e-4), "{x} {y}");
    }
}

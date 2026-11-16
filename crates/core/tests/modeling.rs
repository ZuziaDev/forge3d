mod common;

use common::*;
use forge3d_core::modifier::{evaluate, Modifier, ModifierKind};
use forge3d_core::ops;
use forge3d_core::prelude::*;
use forge3d_core::primitives as p;
use glam::Vec3;

fn edge(m: &Mesh, a: u32, b: u32) -> EdgeId {
    m.find_edge(VertexId(a), VertexId(b)).expect("edge exists")
}

fn all_faces(m: &Mesh) -> Vec<FaceId> {
    m.face_ids().collect()
}

fn delete_faces(m: &Mesh, faces: &[u32]) -> Mesh {
    ops::delete_elements(m, ElementKind::Face, faces).unwrap()
}

// ---------------------------------------------------------------- bevel

#[test]
fn bevel_segments_single_edge_rounded() {
    let m = cube(2.0);
    let e = edge(&m, 5, 6);
    let r = ops::bevel_edges(&m, &[e], 0.4, 3).unwrap();
    assert_counts(&r.mesh, 14, 21, 9);
    assert_closed_manifold(&r.mesh);
    assert_eq!(r.selection.faces.len(), 3);
    // Rounded profile removes less volume than the flat chamfer.
    let flat = ops::bevel_edges(&m, &[e], 0.4, 1).unwrap().mesh;
    let (vr, vf) = (r.mesh.signed_volume(), flat.signed_volume());
    assert!(vr < 8.0 && vr > vf, "{vr} vs chamfer {vf}");
    // Quarter circle of radius 0.4 along a 2-long edge: 8 - (1 - pi/4) 0.16 * 2.
    assert_approx(vr, 8.0 - (1.0 - std::f32::consts::FRAC_PI_4) * 0.32, 0.02);
}

#[test]
fn bevel_segments_all_edges_and_corners_stay_manifold() {
    let m = cube(2.0);
    let all: Vec<EdgeId> = m.edge_ids().collect();
    for segs in [2, 4] {
        let r = ops::bevel_edges(&m, &all, 0.2, segs).unwrap();
        assert_closed_manifold(&r.mesh);
        let v = r.mesh.signed_volume();
        assert!(v < 8.0 && v > 7.6, "{v}");
    }
    // Two and three beveled edges meeting at one corner.
    let two = [edge(&m, 5, 6), edge(&m, 6, 7)];
    assert_closed_manifold(&ops::bevel_edges(&m, &two, 0.2, 3).unwrap().mesh);
    let three = [edge(&m, 5, 6), edge(&m, 6, 7), edge(&m, 2, 6)];
    assert_closed_manifold(&ops::bevel_edges(&m, &three, 0.2, 3).unwrap().mesh);
}

#[test]
fn bevel_segments_out_of_range() {
    let m = cube(2.0);
    let err = ops::bevel_edges(&m, &[EdgeId(0)], 0.1, 0).unwrap_err();
    assert_eq!(err.code(), "invalid_parameter");
    assert!(ops::bevel_edges(&m, &[EdgeId(0)], 0.1, 99).is_err());
}

// ---------------------------------------------------------------- solidify / array

#[test]
fn solidify_open_plane_becomes_closed_slab() {
    let m = p::plane(2.0, 2).unwrap();
    let r = ops::solidify(&m, 0.1).unwrap();
    assert_closed_manifold(&r);
    assert_eq!(r.vertex_count(), 2 * m.vertex_count());
    assert_eq!(
        r.face_count(),
        2 * m.face_count() + m.boundary_edges().len()
    );
    assert_approx(r.signed_volume(), 0.4, 1e-3);
}

#[test]
fn solidify_closed_cube_makes_even_inner_shell() {
    let r = ops::solidify(&cube(2.0), 0.1).unwrap();
    let rep = r.validate();
    assert!(rep.is_valid && rep.is_closed, "{rep:#?}");
    assert_eq!(r.face_components().len(), 2);
    assert_approx(r.signed_volume(), 8.0 - 1.8f32.powi(3), 1e-3);
    assert_eq!(
        ops::solidify(&cube(2.0), 0.0).unwrap_err().code(),
        "invalid_parameter"
    );
}

#[test]
fn array_welds_touching_copies() {
    let r = ops::array(&cube(2.0), 3, Vec3::new(2.0, 0.0, 0.0), true, 1e-3).unwrap();
    assert_closed_manifold(&r);
    assert_eq!(r.vertex_count(), 16);
    assert_approx(r.signed_volume(), 24.0, 1e-3);
    let loose = ops::array(&cube(2.0), 3, Vec3::new(3.0, 0.0, 0.0), false, 0.0).unwrap();
    assert_eq!(loose.face_components().len(), 3);
    assert!(ops::array(&cube(2.0), 0, Vec3::X, false, 0.0).is_err());
}

// ---------------------------------------------------------------- bridge / fill

#[test]
fn bridge_two_tube_ends_into_one_tube() {
    let m = cube(2.0);
    let top = face_facing(&m, Vec3::Z).0;
    let bottom = face_facing(&m, -Vec3::Z).0;
    let tube = delete_faces(&m, &[top, bottom]);
    let mut two = tube.clone();
    two.append(&translated(tube, Vec3::new(0.0, 0.0, 3.0)));
    // Boundary edges at z = 1 (first tube top) and z = 2 (second tube bottom).
    let edges: Vec<EdgeId> = two
        .boundary_edges()
        .into_iter()
        .filter(|&e| (1.0..=2.0).contains(&two.edge_midpoint(e).z))
        .collect();
    assert_eq!(edges.len(), 8);
    let r = ops::bridge_edge_loops(&two, &edges).unwrap();
    assert_valid(&r.mesh);
    assert_eq!(r.mesh.face_count(), 12);
    assert_eq!(r.mesh.boundary_loops().len(), 2);
    assert_eq!(r.mesh.face_components().len(), 1);
    assert_eq!(r.selection.faces.len(), 4);
    // Bridge faces point outward (same side as the tube walls).
    for &f in &r.selection.faces {
        let f = FaceId(f);
        let c = r.mesh.face_centroid(f);
        assert!(r.mesh.face_normal(f).dot(Vec3::new(c.x, c.y, 0.0)) > 0.0);
    }
}

#[test]
fn bridge_errors_on_mismatched_or_missing_loops() {
    let m = cube(2.0);
    let err = ops::bridge_edge_loops(&m, &[EdgeId(0)]).unwrap_err();
    assert_eq!(err.code(), "invalid_selection");
    let open = delete_faces(&m, &[face_facing(&m, Vec3::Z).0]);
    assert!(ops::bridge_edge_loops(&open, &open.boundary_edges()).is_err());
}

#[test]
fn fill_holes_ngon_and_fan() {
    let m = cube(2.0);
    let open = delete_faces(&m, &[face_facing(&m, Vec3::Z).0]);
    let r = ops::fill_holes(&open, None, false).unwrap();
    assert_closed_manifold(&r.mesh);
    assert_counts(&r.mesh, 8, 12, 6);
    assert_approx(r.mesh.signed_volume(), 8.0, 1e-4);
    let f = ops::fill_holes(&open, Some(&open.boundary_edges()[..1]), true).unwrap();
    assert_closed_manifold(&f.mesh);
    assert_counts(&f.mesh, 9, 16, 9);
    assert_eq!(
        ops::fill_holes(&m, None, false).unwrap_err().code(),
        "invalid_selection"
    );
}

// ---------------------------------------------------------------- dissolve / triangulate

#[test]
fn triangulate_and_back_to_quads() {
    let m = cube(2.0);
    let t = ops::triangulate_faces(&m, &all_faces(&m)).unwrap().mesh;
    assert_closed_manifold(&t);
    assert_eq!(t.face_count(), 12);
    let q = ops::tris_to_quads(&t, &all_faces(&t), 5.0).unwrap().mesh;
    assert_closed_manifold(&q);
    assert_counts(&q, 8, 12, 6);
}

#[test]
fn dissolve_edges_vertices_and_faces() {
    let m = cube(2.0);
    // Triangulated face -> dissolve its diagonal again.
    let t = ops::triangulate_faces(&m, &[FaceId(0)]).unwrap().mesh;
    let diag: Vec<u32> = t
        .edge_ids()
        .filter(|&e| t.edge_faces(e).iter().all(|f| t.face_len(*f) == 3))
        .map(|e| e.0)
        .collect();
    assert_eq!(diag.len(), 1);
    let d = ops::dissolve(&t, ElementKind::Edge, &diag).unwrap().mesh;
    assert_closed_manifold(&d);
    assert_counts(&d, 8, 12, 6);
    // Poked face -> dissolve the center vertex.
    let pk = ops::poke_faces(&m, &[FaceId(1)], 0.0).unwrap().mesh;
    assert_counts(&pk, 9, 16, 9);
    let dv = ops::dissolve(&pk, ElementKind::Vertex, &[8]).unwrap().mesh;
    assert_closed_manifold(&dv);
    assert_counts(&dv, 8, 12, 6);
    // Two adjacent faces of a loop-cut cube -> one face.
    let lc = ops::loop_cut(&m, EdgeId(0), 1, 0.5).unwrap().mesh;
    let f0 = lc.edge_faces(lc.edge_ids().find(|&e| lc.edge_face_count(e) == 2).unwrap());
    let df = ops::dissolve(&lc, ElementKind::Face, &[f0[0].0, f0[1].0]).unwrap();
    assert_closed_manifold(&df.mesh);
    assert_eq!(df.mesh.face_count(), lc.face_count() - 1);
    assert_eq!(df.selection.faces.len(), 1);
}

#[test]
fn dissolve_rejects_hole_and_empty() {
    let m = cube(2.0);
    // All faces but one around... dissolving every face of a cube has no outline.
    let all: Vec<u32> = (0..6).collect();
    assert_eq!(
        ops::dissolve(&m, ElementKind::Face, &all)
            .unwrap_err()
            .code(),
        "invalid_selection"
    );
    assert!(ops::dissolve(&m, ElementKind::Face, &[0]).is_err());
    assert!(ops::dissolve(&m, ElementKind::Face, &[]).is_err());
}

#[test]
fn poke_with_offset_adds_volume() {
    let m = cube(2.0);
    let r = ops::poke_faces(&m, &[face_facing(&m, Vec3::Z)], 1.0).unwrap();
    assert_closed_manifold(&r.mesh);
    assert_approx(r.mesh.signed_volume(), 8.0 + 4.0 / 3.0, 1e-3);
    assert_eq!(r.selection.faces.len(), 4);
}

// ---------------------------------------------------------------- smooth / split

#[test]
fn smooth_shrinks_and_keeps_topology() {
    let s = p::ico_sphere(1.0, 2).unwrap();
    let all: Vec<VertexId> = s.vertex_ids().collect();
    let r = ops::smooth_vertices(&s, &all, 0.5, 5).unwrap();
    assert_closed_manifold(&r);
    assert_eq!(r.face_index_lists(), s.face_index_lists());
    assert!(r.signed_volume() < s.signed_volume());
    assert!(ops::smooth_vertices(&s, &all, 1.5, 1).is_err());
    assert!(ops::smooth_vertices(&s, &all, 0.5, 0).is_err());
    // Straight plane boundary stays on its outline (z stays 0, edges stay put).
    let pl = p::plane(2.0, 3).unwrap();
    let pv: Vec<VertexId> = pl.vertex_ids().collect();
    let sm = ops::smooth_vertices(&pl, &pv, 1.0, 3).unwrap();
    let bb = sm.bounding_box().unwrap();
    assert_approx(bb.size().z, 0.0, 1e-6);
}

#[test]
fn split_faces_into_two_meshes() {
    let m = cube(2.0);
    let (rest, part) = ops::split_faces(&m, &[FaceId(0)]).unwrap();
    assert_counts(&rest, 8, 12, 5);
    assert_counts(&part, 4, 4, 1);
    assert_valid(&rest);
}

// ---------------------------------------------------------------- modifiers

#[test]
fn modifier_stack_evaluates_in_order_and_skips_failures() {
    let m = translated(cube(1.0), Vec3::new(1.0, 0.0, 0.0));
    let stack = vec![
        Modifier::new(ModifierKind::Mirror {
            axis: Axis::X,
            merge: true,
            merge_threshold: 0.001,
        }),
        Modifier {
            enabled: false,
            kind: ModifierKind::Triangulate,
        },
        Modifier::new(ModifierKind::Subdivision { levels: 9 }),
        Modifier::new(ModifierKind::Subdivision { levels: 1 }),
    ];
    let (out, warnings) = evaluate(&m, &stack);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].index, 2);
    assert_eq!(warnings[0].code, "invalid_parameter");
    assert_closed_manifold(&out);
    assert_eq!(out.face_count(), 12 * 4);
    // No enabled modifiers -> borrowed base mesh.
    let (same, w) = evaluate(&m, &stack[1..2]);
    assert!(matches!(same, std::borrow::Cow::Borrowed(_)) && w.is_empty());
}

#[test]
fn every_modifier_kind_keeps_cube_valid() {
    let m = cube(2.0);
    let kinds = [
        r#"{"type":"mirror","axis":"z"}"#,
        r#"{"type":"array","count":3,"offset":[2,0,0],"merge":true}"#,
        r#"{"type":"subdivision","levels":2}"#,
        r#"{"type":"solidify","thickness":0.1}"#,
        r#"{"type":"bevel","width":0.1,"segments":3}"#,
        r#"{"type":"triangulate"}"#,
    ];
    for k in kinds {
        let kind: ModifierKind = serde_json::from_str(k).unwrap();
        let out = kind.apply(&m).unwrap_or_else(|e| panic!("{k}: {e}"));
        let r = out.validate();
        assert!(r.is_valid && r.is_closed, "{k}: {r:#?}");
    }
}

#[test]
fn modifier_serde_is_flat_and_old_nodes_load() {
    let m = Modifier::new(ModifierKind::Bevel {
        width: 0.1,
        segments: 2,
        angle_deg: 30.0,
    });
    let j = serde_json::to_value(&m).unwrap();
    assert_eq!(j["type"], "bevel");
    assert_eq!(j["enabled"], true);
    let back: Modifier = serde_json::from_value(j).unwrap();
    assert_eq!(back, m);
    // Scenes serialized before modifiers existed still load.
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    let json = serde_json::to_string(&doc.scene).unwrap();
    assert!(!json.contains("modifiers") && !json.contains("shade_smooth"));
    let scene: Scene = serde_json::from_str(&json).unwrap();
    let n = scene.get(scene.roots()[0]).unwrap();
    assert!(n.modifiers.is_empty() && !n.shade_smooth);
}

#[test]
fn modifier_and_shading_commands_through_document() {
    let mut doc = Document::new();
    let id = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
            Origin::Ai,
        )
        .unwrap()
        .created[0]
        .id;
    doc.execute_json(
        r#"{"op":"add_modifier","modifier":{"type":"subdivision","levels":2}}"#,
        Origin::Ai,
    )
    .unwrap();
    assert_eq!(doc.scene.mesh(id).unwrap().face_count(), 6);
    assert_eq!(doc.scene.evaluated_mesh(id).unwrap().face_count(), 96);
    assert_eq!(
        doc.history_list().last().unwrap().label,
        "AI: Add subdivision modifier to 'Cube'"
    );
    doc.execute_json(r#"{"op":"apply_modifier"}"#, Origin::Ai)
        .unwrap();
    assert_eq!(doc.scene.mesh(id).unwrap().face_count(), 96);
    assert!(doc.scene.node(id).unwrap().modifiers.is_empty());
    doc.undo().unwrap();
    assert_eq!(doc.scene.mesh(id).unwrap().face_count(), 6);
    // error paths
    let e = doc
        .execute_json(r#"{"op":"remove_modifier","index":4}"#, Origin::Ai)
        .unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    let e = doc
        .execute_json(
            r#"{"op":"add_modifier","modifier":{"type":"array","count":0}}"#,
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    let e = doc
        .execute_json(r#"{"op":"set_modifier","index":0}"#, Origin::Ai)
        .unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    let e = doc
        .execute_json(
            r#"{"op":"set_shading","smooth":true,"auto_smooth_angle_deg":500}"#,
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    doc.execute_json(
        r#"{"op":"set_shading","smooth":true,"auto_smooth_angle_deg":30}"#,
        Origin::Ai,
    )
    .unwrap();
    let n = doc.scene.node(id).unwrap();
    assert!(n.shade_smooth);
    assert_eq!(n.shading_crease_deg(), 30.0);
}

#[test]
fn separate_and_join_objects() {
    let mut doc = Document::new();
    let a = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"cube"},"name":"A"}"#,
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let s = doc
        .execute_json(r#"{"op":"separate","faces":[0,1]}"#, Origin::Gui)
        .unwrap();
    let b = s.created[0].id;
    assert_eq!(doc.scene.node(b).unwrap().name, "A.001");
    assert_eq!(doc.scene.mesh(a).unwrap().face_count(), 4);
    assert_eq!(doc.scene.mesh(b).unwrap().face_count(), 2);
    let e = doc
        .execute_json(r#"{"op":"separate","faces":[0,1,2,3]}"#, Origin::Gui)
        .unwrap_err();
    assert_eq!(e.code(), "invalid_selection");
    // Move B, then join it back and weld: closed cube in A's space.
    doc.execute_json(
        &format!(r#"{{"op":"translate","objects":["{b}"],"offset":[0,0,0]}}"#),
        Origin::Gui,
    )
    .unwrap();
    let j = doc
        .execute_json(
            &format!(r#"{{"op":"join_objects","objects":["{a}","{b}"],"into":"{a}"}}"#),
            Origin::Gui,
        )
        .unwrap();
    assert_eq!(j.deleted.len(), 1);
    assert!(!doc.scene.contains(b));
    assert_eq!(doc.scene.mesh(a).unwrap().face_count(), 6);
    doc.execute_json(r#"{"op":"merge_vertices","mode":"distance"}"#, Origin::Gui)
        .unwrap();
    assert_closed_manifold(doc.scene.mesh(a).unwrap());
    let e = doc
        .execute_json(r#"{"op":"join_objects"}"#, Origin::Gui)
        .unwrap_err();
    assert_eq!(e.code(), "invalid_selection");
}

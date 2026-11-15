#![allow(dead_code)]

use forge3d_core::prelude::*;
use forge3d_core::scene::Node;
use glam::{Affine3A, Vec3};
use uuid::Uuid;

pub const EPS: f32 = 1e-4;

fn run(doc: &mut Document, json: serde_json::Value) -> Uuid {
    doc.execute_json(&json.to_string(), Origin::Gui).unwrap();
    doc.selection.active().unwrap()
}

/// A document with several objects, parenting, materials and a hidden node:
///
/// ```text
/// Base (cube, rotated/scaled/translated, red metal)
///   Ball (uv sphere, blue)
/// Group (empty)
///   Floor (plane, hidden)
/// Lamp (cube, emissive > 1)
/// ```
pub fn sample_doc() -> Document {
    let mut doc = Document::new();
    let base = run(
        &mut doc,
        serde_json::json!({
            "op": "create_primitive", "primitive": {"type": "cube", "size": 2.0},
            "name": "Base", "translation": [1.0, 2.0, 3.0],
            "rotation_euler_deg": [10.0, 20.0, 30.0], "scale": [1.0, 2.0, 0.5],
            "material": {"name": "Red", "base_color": [1.0, 0.25, 0.125, 1.0],
                         "metallic": 0.25, "roughness": 0.75, "emissive": [0.0, 0.0, 0.0]}
        }),
    );
    run(
        &mut doc,
        serde_json::json!({
            "op": "create_primitive",
            "primitive": {"type": "uv_sphere", "radius": 0.5, "segments": 8, "rings": 4},
            "name": "Ball", "parent": base, "translation": [0.0, 0.0, 2.0],
            "material": {"name": "Blue", "base_color": [0.0, 0.0, 1.0, 0.5],
                         "metallic": 1.0, "roughness": 0.125, "emissive": [0.0, 0.0, 0.0]}
        }),
    );
    let group = run(
        &mut doc,
        serde_json::json!({"op": "create_empty", "name": "Group", "translation": [-4.0, 0.0, 0.0]}),
    );
    let floor = run(
        &mut doc,
        serde_json::json!({
            "op": "create_primitive", "primitive": {"type": "plane", "size": 3.0, "subdivisions": 2},
            "name": "Floor", "parent": group, "translation": [0.0, 1.0, 0.0]
        }),
    );
    doc.execute_json(
        &serde_json::json!({"op": "set_visibility", "objects": [floor], "visible": false})
            .to_string(),
        Origin::Gui,
    )
    .unwrap();
    run(
        &mut doc,
        serde_json::json!({
            "op": "create_primitive", "primitive": {"type": "cube", "size": 0.5},
            "name": "Lamp", "translation": [5.0, 5.0, 5.0],
            "material": {"name": "Lamp", "base_color": [1.0, 1.0, 1.0, 1.0],
                         "metallic": 0.0, "roughness": 0.5, "emissive": [4.0, 2.0, 1.0]}
        }),
    );
    doc
}

pub fn triangle_count(n: &Node) -> usize {
    n.mesh.as_ref().map_or(0, |m| m.triangulate().len())
}

pub fn assert_affine_eq(a: &Affine3A, b: &Affine3A, what: &str) {
    for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array().iter()) {
        assert!((x - y).abs() < EPS, "{what}: {a:?} != {b:?}");
    }
}

pub fn sorted_positions(ps: Vec<Vec3>) -> Vec<Vec3> {
    let mut ps = ps;
    ps.sort_by(|a, b| {
        a.x.total_cmp(&b.x)
            .then(a.y.total_cmp(&b.y))
            .then(a.z.total_cmp(&b.z))
    });
    ps
}

pub fn assert_positions_eq(a: Vec<Vec3>, b: Vec<Vec3>, what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: vertex count");
    for (p, q) in sorted_positions(a).iter().zip(sorted_positions(b).iter()) {
        assert!(p.distance(*q) < EPS, "{what}: {p:?} != {q:?}");
    }
}

pub fn assert_material_eq(a: &Option<Material>, b: &Option<Material>, what: &str) {
    match (a, b) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            assert_eq!(a.name, b.name, "{what}: material name");
            let pairs = a
                .base_color
                .iter()
                .zip(b.base_color.iter())
                .chain(std::iter::once((&a.metallic, &b.metallic)))
                .chain(std::iter::once((&a.roughness, &b.roughness)))
                .chain(a.emissive.iter().zip(b.emissive.iter()));
            for (x, y) in pairs {
                assert!((x - y).abs() < 1e-5, "{what}: material {a:?} != {b:?}");
            }
        }
        _ => panic!("{what}: material presence differs: {a:?} vs {b:?}"),
    }
}

/// Compare hierarchy, names, transforms, materials, visibility and mesh
/// geometry of two scenes (glTF-like fidelity). `tri_only` expects the second
/// scene's meshes to be triangulated versions of the first.
pub fn assert_scenes_match(a: &Scene, b: &Scene, tri_only: bool) {
    assert_eq!(a.len(), b.len(), "node count");
    assert_eq!(a.roots().len(), b.roots().len(), "root count");
    let (ta, tb) = (a.traverse(), b.traverse());
    for (ia, ib) in ta.iter().zip(tb.iter()) {
        let (na, nb) = (a.node(*ia).unwrap(), b.node(*ib).unwrap());
        assert_eq!(na.name, nb.name, "name");
        assert_eq!(na.visible, nb.visible, "visible of {}", na.name);
        assert_eq!(
            na.children().len(),
            nb.children().len(),
            "children of {}",
            na.name
        );
        assert_eq!(
            na.parent().map(|p| a.node(p).unwrap().name.clone()),
            nb.parent().map(|p| b.node(p).unwrap().name.clone()),
            "parent of {}",
            na.name
        );
        assert_affine_eq(&na.transform, &nb.transform, &na.name);
        assert_material_eq(&na.material, &nb.material, &na.name);
        assert_eq!(
            na.mesh.is_some(),
            nb.mesh.is_some(),
            "mesh presence {}",
            na.name
        );
        if let (Some(ma), Some(mb)) = (&na.mesh, &nb.mesh) {
            assert_eq!(
                ma.vertex_count(),
                mb.vertex_count(),
                "vertices of {}",
                na.name
            );
            if tri_only {
                assert_eq!(
                    ma.triangulate().len(),
                    mb.face_count(),
                    "triangles of {}",
                    na.name
                );
            } else {
                assert_eq!(ma.face_count(), mb.face_count(), "faces of {}", na.name);
            }
            assert_positions_eq(ma.positions(), mb.positions(), &na.name);
        }
    }
}

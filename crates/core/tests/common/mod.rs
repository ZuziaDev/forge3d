#![allow(dead_code)]

use forge3d_core::prelude::*;
use glam::Vec3;

/// Assert the mesh is a valid, closed, consistently oriented 2-manifold.
#[track_caller]
pub fn assert_closed_manifold(m: &Mesh) {
    let r = m.validate();
    assert!(r.is_valid, "mesh not valid: {r:#?}");
    assert!(r.is_closed, "mesh not closed: {r:#?}");
    assert_eq!(r.isolated_vertices, 0, "isolated vertices: {r:#?}");
}

/// Assert the mesh is valid (manifold, oriented, non-degenerate) but possibly open.
#[track_caller]
pub fn assert_valid(m: &Mesh) {
    let r = m.validate();
    assert!(r.is_valid, "mesh not valid: {r:#?}");
}

#[track_caller]
pub fn assert_counts(m: &Mesh, v: usize, e: usize, f: usize) {
    assert_eq!(
        (m.vertex_count(), m.edge_count(), m.face_count()),
        (v, e, f),
        "unexpected (V, E, F)"
    );
}

#[track_caller]
pub fn assert_approx(a: f32, b: f32, eps: f32) {
    assert!((a - b).abs() <= eps, "{a} != {b} (eps {eps})");
}

/// Every face normal points away from the mesh center (convex shapes).
#[track_caller]
pub fn assert_outward_convex(m: &Mesh) {
    let c = m.bounding_box().unwrap().center();
    for f in m.face_ids() {
        let n = m.face_normal(f);
        let d = m.face_centroid(f) - c;
        assert!(n.dot(d) > 0.0, "face {} points inward", f.0);
    }
}

pub fn cube(size: f32) -> Mesh {
    forge3d_core::primitives::cube(size).unwrap()
}

pub fn translated(mut m: Mesh, t: Vec3) -> Mesh {
    m.transform(&glam::Affine3A::from_translation(t));
    m
}

/// Index of the face whose normal is closest to `dir`.
pub fn face_facing(m: &Mesh, dir: Vec3) -> FaceId {
    m.face_ids()
        .max_by(|&a, &b| {
            m.face_normal(a)
                .dot(dir)
                .total_cmp(&m.face_normal(b).dot(dir))
        })
        .unwrap()
}

pub fn spec(json: &str) -> CommandSpec {
    CommandSpec::from_json(json).unwrap_or_else(|e| panic!("bad spec {json}: {e}"))
}

mod common;

use common::*;
use forge3d_core::prelude::*;
use forge3d_core::primitives as p;
use std::f32::consts::PI;

#[test]
fn cube_topology_and_normals() {
    let m = p::cube(2.0).unwrap();
    assert_counts(&m, 8, 12, 6);
    assert_eq!(m.euler_characteristic(), 2);
    assert_closed_manifold(&m);
    assert_outward_convex(&m);
    assert_approx(m.signed_volume(), 8.0, 1e-4);
    assert!(m.face_ids().all(|f| m.face_len(f) == 4));
}

#[test]
fn plane_is_open_with_boundary() {
    let m = p::plane(2.0, 3).unwrap();
    assert_counts(&m, 16, 24, 9);
    assert_eq!(m.euler_characteristic(), 1);
    let r = m.validate();
    assert!(r.is_valid && r.is_manifold && !r.is_closed);
    assert_eq!(r.boundary_edges, 12);
    assert!(m.face_ids().all(|f| m.face_normal(f).abs_diff_eq(glam::Vec3::Z, 1e-6)));
    assert_eq!(m.boundary_loops().len(), 1);
    assert_eq!(m.boundary_loops()[0].len(), 12);
    assert_approx(m.surface_area(), 4.0, 1e-4);
}

#[test]
fn uv_sphere() {
    let m = p::uv_sphere(1.0, 8, 4).unwrap();
    assert_counts(&m, 26, 56, 32);
    assert_eq!(m.euler_characteristic(), 2);
    assert_closed_manifold(&m);
    assert_outward_convex(&m);
    assert!(m.signed_volume() > 0.0);
    let hi = p::uv_sphere(1.0, 64, 32).unwrap();
    assert_closed_manifold(&hi);
    assert_approx(hi.signed_volume(), 4.0 / 3.0 * PI, 0.05);
}

#[test]
fn ico_sphere() {
    for (n, v, f) in [(0u32, 12usize, 20usize), (1, 42, 80), (2, 162, 320), (3, 642, 1280)] {
        let m = p::ico_sphere(1.0, n).unwrap();
        assert_eq!((m.vertex_count(), m.face_count()), (v, f));
        assert_eq!(m.euler_characteristic(), 2);
        assert_closed_manifold(&m);
        assert_outward_convex(&m);
        for vtx in m.vertex_ids() {
            assert_approx(m.position(vtx).length(), 1.0, 1e-5);
        }
    }
    assert!(p::ico_sphere(1.0, 8).is_err());
}

#[test]
fn cylinder() {
    let m = p::cylinder(1.0, 2.0, 8).unwrap();
    assert_counts(&m, 16, 24, 10);
    assert_eq!(m.euler_characteristic(), 2);
    assert_closed_manifold(&m);
    assert_outward_convex(&m);
    let hi = p::cylinder(1.0, 2.0, 128).unwrap();
    assert_approx(hi.signed_volume(), PI * 2.0, 0.01);
}

#[test]
fn cone_and_frustum() {
    let m = p::cone(1.0, 0.0, 2.0, 8).unwrap();
    assert_counts(&m, 9, 16, 9);
    assert_eq!(m.euler_characteristic(), 2);
    assert_closed_manifold(&m);
    assert_outward_convex(&m);
    let hi = p::cone(1.0, 0.0, 3.0, 128).unwrap();
    assert_approx(hi.signed_volume(), PI, 0.01);

    let inv = p::cone(0.0, 1.0, 2.0, 8).unwrap();
    assert_closed_manifold(&inv);
    assert!(inv.signed_volume() > 0.0);

    let fr = p::cone(1.0, 0.5, 2.0, 12).unwrap();
    assert_counts(&fr, 24, 36, 14);
    assert_closed_manifold(&fr);
    assert_outward_convex(&fr);
    assert!(p::cone(0.0, 0.0, 1.0, 8).is_err());
}

#[test]
fn torus() {
    let m = p::torus(1.0, 0.25, 8, 6).unwrap();
    assert_counts(&m, 48, 96, 48);
    assert_eq!(m.euler_characteristic(), 0);
    assert_closed_manifold(&m);
    let hi = p::torus(1.0, 0.25, 96, 48).unwrap();
    assert_approx(hi.signed_volume(), 2.0 * PI * PI * 0.25 * 0.25, 0.01);
    assert!(p::torus(1.0, 1.5, 8, 6).is_err());
}

#[test]
fn primitive_spec_defaults_from_json() {
    let s: PrimitiveSpec = serde_json::from_str(r#"{"type":"uv_sphere"}"#).unwrap();
    assert_eq!(
        s,
        PrimitiveSpec::UvSphere {
            radius: 1.0,
            segments: 32,
            rings: 16
        }
    );
    for json in [
        r#"{"type":"cube"}"#,
        r#"{"type":"plane"}"#,
        r#"{"type":"uv_sphere"}"#,
        r#"{"type":"ico_sphere"}"#,
        r#"{"type":"cylinder"}"#,
        r#"{"type":"cone"}"#,
        r#"{"type":"torus"}"#,
    ] {
        let s: PrimitiveSpec = serde_json::from_str(json).unwrap();
        let m = s.build().unwrap();
        assert!(m.validate().is_valid, "{json}");
        if !matches!(s, PrimitiveSpec::Plane { .. }) {
            assert!(m.is_closed(), "{json}");
            assert!(m.signed_volume() > 0.0, "{json}");
        }
    }
}

#[test]
fn invalid_parameters_are_reported() {
    let e = p::cube(-1.0).unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    assert!(p::uv_sphere(1.0, 2, 4).is_err());
    assert!(p::cylinder(1.0, 0.0, 8).is_err());
    assert!(p::plane(1.0, 0).is_err());
}

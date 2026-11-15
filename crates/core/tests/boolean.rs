mod common;

use common::*;
use forge3d_core::ops::{boolean, BooleanOp};
use forge3d_core::prelude::*;
use forge3d_core::primitives as p;
use glam::Vec3;

fn check_closed(m: &Mesh) {
    let r = m.validate();
    assert_eq!(r.boundary_edges, 0, "open boundary after boolean: {r:#?}");
    assert_eq!(r.non_manifold_edges, 0, "non-manifold edges: {r:#?}");
    assert!(r.is_consistently_oriented, "{r:#?}");
    assert!(r.is_closed, "{r:#?}");
    assert!(r.is_valid, "degenerate or non-manifold result: {r:#?}");
}

#[test]
fn overlapping_cubes() {
    let a = cube(2.0);
    let b = translated(cube(2.0), Vec3::splat(1.0));
    let u = boolean(&a, &b, BooleanOp::Union).unwrap();
    check_closed(&u);
    assert_approx(u.signed_volume(), 15.0, 1e-3);
    let d = boolean(&a, &b, BooleanOp::Difference).unwrap();
    check_closed(&d);
    assert_approx(d.signed_volume(), 7.0, 1e-3);
    let i = boolean(&a, &b, BooleanOp::Intersect).unwrap();
    check_closed(&i);
    assert_approx(i.signed_volume(), 1.0, 1e-3);
    assert_eq!(i.euler_characteristic(), 2);
}

#[test]
fn cubes_with_coplanar_faces() {
    let a = cube(2.0);
    let b = translated(cube(2.0), Vec3::new(1.0, 0.0, 0.0));
    let u = boolean(&a, &b, BooleanOp::Union).unwrap();
    check_closed(&u);
    assert_approx(u.signed_volume(), 12.0, 1e-3);
    let d = boolean(&a, &b, BooleanOp::Difference).unwrap();
    check_closed(&d);
    assert_approx(d.signed_volume(), 4.0, 1e-3);
    let i = boolean(&a, &b, BooleanOp::Intersect).unwrap();
    check_closed(&i);
    assert_approx(i.signed_volume(), 4.0, 1e-3);
}

#[test]
fn cube_minus_sphere() {
    let a = cube(2.0);
    let s = p::uv_sphere(1.3, 24, 12).unwrap();
    let d = boolean(&a, &s, BooleanOp::Difference).unwrap();
    check_closed(&d);
    let v = d.signed_volume();
    assert!(v > 0.0 && v < 2.0, "only the corners remain, volume {v}");

    // a sphere carving a dent into one face
    let s = translated(p::ico_sphere(0.8, 2).unwrap(), Vec3::new(0.0, 0.0, 1.0));
    let d = boolean(&a, &s, BooleanOp::Difference).unwrap();
    check_closed(&d);
    assert!(d.signed_volume() < 8.0 && d.signed_volume() > 6.0);
    let u = boolean(&a, &s, BooleanOp::Union).unwrap();
    check_closed(&u);
    assert!(u.signed_volume() > 8.0);
}

#[test]
fn cylinder_through_cube() {
    let a = cube(2.0);
    let c = p::cylinder(0.5, 3.0, 16).unwrap();
    let d = boolean(&a, &c, BooleanOp::Difference).unwrap();
    check_closed(&d);
    // a hole through the cube makes it genus 1
    assert_eq!(d.euler_characteristic(), 0);
    let expected = 8.0 - c.signed_volume() * (2.0 / 3.0);
    assert_approx(d.signed_volume(), expected, 1e-2);
}

#[test]
fn disjoint_and_contained() {
    let a = cube(2.0);
    let far = translated(cube(1.0), Vec3::new(5.0, 0.0, 0.0));
    let u = boolean(&a, &far, BooleanOp::Union).unwrap();
    check_closed(&u);
    assert_eq!(u.face_components().len(), 2);
    let i = boolean(&a, &far, BooleanOp::Intersect).unwrap();
    assert_eq!(i.face_count(), 0);
    let inner = cube(1.0);
    let d = boolean(&a, &inner, BooleanOp::Difference).unwrap();
    check_closed(&d);
    assert_approx(d.signed_volume(), 7.0, 1e-3);
    assert!(boolean(&a, &Mesh::new(), BooleanOp::Union).is_err());
}

#[test]
fn curved_operands() {
    let a = p::ico_sphere(1.0, 2).unwrap();
    let b = translated(p::ico_sphere(1.0, 2).unwrap(), Vec3::new(0.9, 0.3, 0.1));
    for op in [
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersect,
    ] {
        let r = boolean(&a, &b, op).unwrap();
        check_closed(&r);
    }
    let t = p::torus(1.0, 0.3, 24, 12).unwrap();
    let c = translated(cube(1.0), Vec3::new(1.0, 0.0, 0.0));
    let d = boolean(&t, &c, BooleanOp::Difference).unwrap();
    check_closed(&d);
    assert!(d.signed_volume() < t.signed_volume());
    let s = p::uv_sphere(0.9, 16, 8).unwrap();
    let cyl = p::cylinder(0.3, 3.0, 12).unwrap();
    let r = boolean(&s, &cyl, BooleanOp::Difference).unwrap();
    check_closed(&r);
    assert_eq!(r.euler_characteristic(), 0);
}

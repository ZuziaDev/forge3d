//! Parametric primitive meshes. Z is up; all closed primitives are closed
//! 2-manifolds with outward-facing (counter-clockwise) polygons, centered at
//! the origin.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use glam::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::mesh::Mesh;

/// Maximum segment count accepted by primitive builders.
pub const MAX_SEGMENTS: u32 = 1024;
/// Maximum ico-sphere subdivision level.
pub const MAX_ICO_SUBDIVISIONS: u32 = 7;

fn d_size() -> f32 {
    2.0
}
fn d_radius() -> f32 {
    1.0
}
fn d_depth() -> f32 {
    2.0
}
fn d_segments() -> u32 {
    32
}
fn d_rings() -> u32 {
    16
}
fn d_subdiv() -> u32 {
    2
}
fn d_one() -> u32 {
    1
}
fn d_major_radius() -> f32 {
    1.0
}
fn d_minor_radius() -> f32 {
    0.25
}
fn d_major_segments() -> u32 {
    48
}
fn d_minor_segments() -> u32 {
    12
}

/// Parameters of a primitive shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrimitiveSpec {
    /// Axis-aligned cube with edge length `size` (8 vertices, 6 quads).
    Cube {
        #[serde(default = "d_size")]
        size: f32,
    },
    /// Flat square in the XY plane facing +Z, split into `subdivisions`² quads.
    Plane {
        #[serde(default = "d_size")]
        size: f32,
        #[serde(default = "d_one")]
        subdivisions: u32,
    },
    /// Latitude/longitude sphere: `segments` around Z, `rings` bands from pole to pole.
    UvSphere {
        #[serde(default = "d_radius")]
        radius: f32,
        #[serde(default = "d_segments")]
        segments: u32,
        #[serde(default = "d_rings")]
        rings: u32,
    },
    /// Geodesic sphere: icosahedron subdivided `subdivisions` times (0..=7).
    IcoSphere {
        #[serde(default = "d_radius")]
        radius: f32,
        #[serde(default = "d_subdiv")]
        subdivisions: u32,
    },
    /// Cylinder along Z with n-gon caps.
    Cylinder {
        #[serde(default = "d_radius")]
        radius: f32,
        #[serde(default = "d_depth")]
        depth: f32,
        #[serde(default = "d_segments")]
        segments: u32,
    },
    /// Cone (or frustum when `radius_top` > 0) along Z.
    Cone {
        #[serde(default = "d_radius")]
        radius_bottom: f32,
        #[serde(default)]
        radius_top: f32,
        #[serde(default = "d_depth")]
        depth: f32,
        #[serde(default = "d_segments")]
        segments: u32,
    },
    /// Torus around Z.
    Torus {
        #[serde(default = "d_major_radius")]
        major_radius: f32,
        #[serde(default = "d_minor_radius")]
        minor_radius: f32,
        #[serde(default = "d_major_segments")]
        major_segments: u32,
        #[serde(default = "d_minor_segments")]
        minor_segments: u32,
    },
}

impl PrimitiveSpec {
    /// Default object name for this primitive.
    pub fn default_name(&self) -> &'static str {
        match self {
            PrimitiveSpec::Cube { .. } => "Cube",
            PrimitiveSpec::Plane { .. } => "Plane",
            PrimitiveSpec::UvSphere { .. } => "Sphere",
            PrimitiveSpec::IcoSphere { .. } => "Icosphere",
            PrimitiveSpec::Cylinder { .. } => "Cylinder",
            PrimitiveSpec::Cone { .. } => "Cone",
            PrimitiveSpec::Torus { .. } => "Torus",
        }
    }

    /// Build the mesh.
    pub fn build(&self) -> CoreResult<Mesh> {
        match *self {
            PrimitiveSpec::Cube { size } => cube(size),
            PrimitiveSpec::Plane { size, subdivisions } => plane(size, subdivisions),
            PrimitiveSpec::UvSphere {
                radius,
                segments,
                rings,
            } => uv_sphere(radius, segments, rings),
            PrimitiveSpec::IcoSphere {
                radius,
                subdivisions,
            } => ico_sphere(radius, subdivisions),
            PrimitiveSpec::Cylinder {
                radius,
                depth,
                segments,
            } => cylinder(radius, depth, segments),
            PrimitiveSpec::Cone {
                radius_bottom,
                radius_top,
                depth,
                segments,
            } => cone(radius_bottom, radius_top, depth, segments),
            PrimitiveSpec::Torus {
                major_radius,
                minor_radius,
                major_segments,
                minor_segments,
            } => torus(major_radius, minor_radius, major_segments, minor_segments),
        }
    }
}

fn positive(name: &str, v: f32) -> CoreResult<()> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(CoreError::param(
            name,
            format!("must be a positive finite number, got {v}"),
        ))
    }
}

fn segments(name: &str, v: u32, min: u32) -> CoreResult<()> {
    if (min..=MAX_SEGMENTS).contains(&v) {
        Ok(())
    } else {
        Err(CoreError::param(
            name,
            format!("must be in {min}..={MAX_SEGMENTS}, got {v}"),
        ))
    }
}

/// Cube with edge length `size`.
pub fn cube(size: f32) -> CoreResult<Mesh> {
    positive("size", size)?;
    let h = size * 0.5;
    let p = vec![
        Vec3::new(-h, -h, -h),
        Vec3::new(h, -h, -h),
        Vec3::new(h, h, -h),
        Vec3::new(-h, h, -h),
        Vec3::new(-h, -h, h),
        Vec3::new(h, -h, h),
        Vec3::new(h, h, h),
        Vec3::new(-h, h, h),
    ];
    let f: [[u32; 4]; 6] = [
        [0, 3, 2, 1], // -Z
        [4, 5, 6, 7], // +Z
        [0, 1, 5, 4], // -Y
        [2, 3, 7, 6], // +Y
        [0, 4, 7, 3], // -X
        [1, 2, 6, 5], // +X
    ];
    Mesh::from_polygons(p, &f)
}

/// Square plane of side `size` in XY facing +Z with `subdivisions`² quads.
pub fn plane(size: f32, subdivisions: u32) -> CoreResult<Mesh> {
    positive("size", size)?;
    segments("subdivisions", subdivisions, 1)?;
    let n = subdivisions;
    let h = size * 0.5;
    let mut p = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    for j in 0..=n {
        for i in 0..=n {
            p.push(Vec3::new(
                -h + size * i as f32 / n as f32,
                -h + size * j as f32 / n as f32,
                0.0,
            ));
        }
    }
    let idx = |i: u32, j: u32| j * (n + 1) + i;
    let mut f = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            f.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    Mesh::from_polygons(p, &f)
}

/// UV sphere (triangle fans at the poles, quads elsewhere).
pub fn uv_sphere(radius: f32, segs: u32, rings: u32) -> CoreResult<Mesh> {
    positive("radius", radius)?;
    segments("segments", segs, 3)?;
    segments("rings", rings, 2)?;
    let mut p = vec![Vec3::new(0.0, 0.0, radius)];
    for k in 1..rings {
        let theta = PI * k as f32 / rings as f32;
        let (st, ct) = theta.sin_cos();
        for i in 0..segs {
            let phi = TAU * i as f32 / segs as f32;
            let (sp, cp) = phi.sin_cos();
            p.push(Vec3::new(radius * st * cp, radius * st * sp, radius * ct));
        }
    }
    let bottom = p.len() as u32;
    p.push(Vec3::new(0.0, 0.0, -radius));
    let ring = |k: u32, i: u32| 1 + (k - 1) * segs + (i % segs);
    let mut f: Vec<Vec<u32>> = Vec::with_capacity((segs * rings) as usize);
    for i in 0..segs {
        f.push(vec![0, ring(1, i), ring(1, i + 1)]);
    }
    for k in 1..rings - 1 {
        for i in 0..segs {
            f.push(vec![
                ring(k, i),
                ring(k + 1, i),
                ring(k + 1, i + 1),
                ring(k, i + 1),
            ]);
        }
    }
    for i in 0..segs {
        f.push(vec![ring(rings - 1, i), bottom, ring(rings - 1, i + 1)]);
    }
    Mesh::from_polygons(p, &f)
}

/// Geodesic ico sphere.
pub fn ico_sphere(radius: f32, subdivisions: u32) -> CoreResult<Mesh> {
    positive("radius", radius)?;
    if subdivisions > MAX_ICO_SUBDIVISIONS {
        return Err(CoreError::param(
            "subdivisions",
            format!("must be in 0..={MAX_ICO_SUBDIVISIONS}, got {subdivisions}"),
        ));
    }
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut p: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, p: &mut Vec<Vec3>| -> u32 {
            let key = if a < b { (a, b) } else { (b, a) };
            *cache.entry(key).or_insert_with(|| {
                p.push(((p[a as usize] + p[b as usize]) * 0.5).normalize());
                (p.len() - 1) as u32
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = mid(a, b, &mut p);
            let bc = mid(b, c, &mut p);
            let ca = mid(c, a, &mut p);
            next.push([a, ab, ca]);
            next.push([b, bc, ab]);
            next.push([c, ca, bc]);
            next.push([ab, bc, ca]);
        }
        tris = next;
    }
    // Enforce outward winding (the sphere is convex and centered).
    for t in &mut tris {
        let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            t.swap(1, 2);
        }
    }
    for v in &mut p {
        *v *= radius;
    }
    Mesh::from_polygons(p, &tris)
}

/// Cylinder along Z.
pub fn cylinder(radius: f32, depth: f32, segs: u32) -> CoreResult<Mesh> {
    positive("radius", radius)?;
    cone_impl(radius, radius, depth, segs)
}

/// Cone or frustum along Z.
pub fn cone(radius_bottom: f32, radius_top: f32, depth: f32, segs: u32) -> CoreResult<Mesh> {
    if !(radius_bottom.is_finite() && radius_bottom >= 0.0) {
        return Err(CoreError::param("radius_bottom", "must be >= 0"));
    }
    if !(radius_top.is_finite() && radius_top >= 0.0) {
        return Err(CoreError::param("radius_top", "must be >= 0"));
    }
    if radius_bottom <= 0.0 && radius_top <= 0.0 {
        return Err(CoreError::param(
            "radius_bottom",
            "at least one of radius_bottom / radius_top must be positive",
        ));
    }
    cone_impl(radius_bottom, radius_top, depth, segs)
}

fn cone_impl(r0: f32, r1: f32, depth: f32, segs: u32) -> CoreResult<Mesh> {
    positive("depth", depth)?;
    segments("segments", segs, 3)?;
    let h = depth * 0.5;
    let mut p = Vec::new();
    // Each end is either a ring of `segs` vertices or a single apex.
    let make_end = |r: f32, z: f32, p: &mut Vec<Vec3>| -> Vec<u32> {
        if r <= 0.0 {
            p.push(Vec3::new(0.0, 0.0, z));
            vec![(p.len() - 1) as u32]
        } else {
            (0..segs)
                .map(|i| {
                    let phi = TAU * i as f32 / segs as f32;
                    let (s, c) = phi.sin_cos();
                    p.push(Vec3::new(r * c, r * s, z));
                    (p.len() - 1) as u32
                })
                .collect()
        }
    };
    let bottom = make_end(r0, -h, &mut p);
    let top = make_end(r1, h, &mut p);
    let mut f: Vec<Vec<u32>> = Vec::new();
    for i in 0..segs as usize {
        let j = (i + 1) % segs as usize;
        let mut face = Vec::with_capacity(4);
        if bottom.len() == 1 {
            face.push(bottom[0]);
        } else {
            face.push(bottom[i]);
            face.push(bottom[j]);
        }
        if top.len() == 1 {
            face.push(top[0]);
        } else {
            face.push(top[j]);
            face.push(top[i]);
        }
        f.push(face);
    }
    if top.len() > 1 {
        f.push(top.clone());
    }
    if bottom.len() > 1 {
        f.push(bottom.iter().rev().copied().collect());
    }
    Mesh::from_polygons(p, &f)
}

/// Torus around the Z axis.
pub fn torus(major: f32, minor: f32, major_segs: u32, minor_segs: u32) -> CoreResult<Mesh> {
    positive("major_radius", major)?;
    positive("minor_radius", minor)?;
    if minor >= major {
        return Err(CoreError::param(
            "minor_radius",
            "must be smaller than major_radius (otherwise the torus self-intersects)",
        ));
    }
    segments("major_segments", major_segs, 3)?;
    segments("minor_segments", minor_segs, 3)?;
    let (m, n) = (major_segs, minor_segs);
    let mut p = Vec::with_capacity((m * n) as usize);
    for i in 0..m {
        let phi = TAU * i as f32 / m as f32;
        let (sp, cp) = phi.sin_cos();
        for j in 0..n {
            let theta = TAU * j as f32 / n as f32;
            let (st, ct) = theta.sin_cos();
            let r = major + minor * ct;
            p.push(Vec3::new(r * cp, r * sp, minor * st));
        }
    }
    let idx = |i: u32, j: u32| (i % m) * n + (j % n);
    let mut f = Vec::with_capacity((m * n) as usize);
    for i in 0..m {
        for j in 0..n {
            f.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    Mesh::from_polygons(p, &f)
}

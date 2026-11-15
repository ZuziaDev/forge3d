//! CPU picking (ray casting) and resolution of the nearest vertex / edge of a
//! hit face, shared with the GPU ID-buffer picker.

use glam::{Affine3A, Mat4, Vec2, Vec3, Vec4Swizzles};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::camera::{Camera, Ray};
use crate::extract::{MeshGeometry, SceneDrawData};

/// Result of a pick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PickHit {
    /// Hit node.
    pub node_id: Uuid,
    /// Hit face index of the node's mesh.
    pub face: u32,
    /// Vertex of the hit face nearest to the cursor (screen space).
    pub vertex: u32,
    /// Edge of the hit face nearest to the cursor (screen space).
    pub edge: u32,
    /// World-space hit position.
    pub position: [f32; 3],
    /// Distance from the ray origin.
    pub distance: f32,
}

/// Möller–Trumbore ray/triangle intersection (two-sided); returns `t`.
pub fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-7).then_some(t)
}

/// Distance from point `p` to segment `a`–`b` (2D).
pub fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// Nearest vertex and edge of face `face` to the cursor, measured in screen
/// space. `geometry` is transformed by `to_world`, then by `view_proj`.
/// Returns `(vertex, edge)`.
pub fn nearest_face_elements(
    geometry: &MeshGeometry,
    to_world: &Affine3A,
    view_proj: &Mat4,
    viewport: (u32, u32),
    cursor: Vec2,
    face: u32,
) -> (u32, u32) {
    let (w, h) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    let project = |vi: u32| -> Vec2 {
        let p = to_world.transform_point3(geometry.vertices[vi as usize]);
        let c = *view_proj * p.extend(1.0);
        let n = c.xy() / c.w.max(1e-12);
        Vec2::new((n.x * 0.5 + 0.5) * w, (0.5 - n.y * 0.5) * h)
    };
    let corners = geometry.face_vertices(face);
    let mut best_v = (f32::INFINITY, corners.first().copied().unwrap_or(0));
    for &v in corners {
        let d = project(v).distance(cursor);
        if d < best_v.0 {
            best_v = (d, v);
        }
    }
    let edges = geometry.face_edge_ids(face);
    let mut best_e = (f32::INFINITY, edges.first().copied().unwrap_or(0));
    for &e in edges {
        let [a, b] = geometry.edges[e as usize];
        let d = point_segment_distance(cursor, project(a), project(b));
        if d < best_e.0 {
            best_e = (d, e);
        }
    }
    (best_v.1, best_e.1)
}

/// Nearest hit of a ray against the draw data (world-space geometry).
/// Returns `(object index, triangle index, t)`.
pub fn cast_ray(draw: &SceneDrawData, ray: &Ray) -> Option<(usize, usize, f32)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for (oi, obj) in draw.objects.iter().enumerate() {
        let g = &obj.geometry;
        if let Some(b) = g.bounds() {
            if !ray_hits_aabb(ray, b.min, b.max, best.map_or(f32::INFINITY, |b| b.2)) {
                continue;
            }
        }
        for t in 0..g.triangle_count() {
            let p = &g.positions[3 * t..3 * t + 3];
            if let Some(d) = ray_triangle(ray, p[0], p[1], p[2]) {
                if best.is_none_or(|b| d < b.2) {
                    best = Some((oi, t, d));
                }
            }
        }
    }
    best
}

fn ray_hits_aabb(ray: &Ray, min: Vec3, max: Vec3, t_max: f32) -> bool {
    let inv = ray.dir.recip();
    let t0 = (min - ray.origin) * inv;
    let t1 = (max - ray.origin) * inv;
    let tmin = t0.min(t1);
    let tmax = t0.max(t1);
    // NaN (0 * inf) components are ignored by max_element/min_element semantics below.
    let enter = tmin.x.max(tmin.y).max(tmin.z);
    let exit = tmax.x.min(tmax.y).min(tmax.z);
    let eps = 1e-4 * (1.0 + exit.abs());
    exit + eps >= enter.max(0.0) && enter <= t_max
}

/// Pick the object/face under pixel (`x`, `y`) (top-left origin, pixel
/// units; pass `+0.5` for pixel centers) of a `width` × `height` viewport.
pub fn pick(
    draw: &SceneDrawData,
    camera: &Camera,
    width: u32,
    height: u32,
    x: f32,
    y: f32,
) -> Option<PickHit> {
    let ray = camera.ray_through_pixel(x, y, width, height);
    let (oi, ti, t) = cast_ray(draw, &ray)?;
    let obj = &draw.objects[oi];
    let face = obj.geometry.tri_faces[ti];
    let vp = camera.view_projection(width.max(1) as f32 / height.max(1) as f32);
    let (vertex, edge) = nearest_face_elements(
        &obj.geometry,
        &Affine3A::IDENTITY,
        &vp,
        (width, height),
        Vec2::new(x, y),
        face,
    );
    Some(PickHit {
        node_id: obj.node_id,
        face,
        vertex,
        edge,
        position: ray.at(t).to_array(),
        distance: t,
    })
}

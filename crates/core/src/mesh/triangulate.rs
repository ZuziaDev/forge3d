//! Polygon triangulation (ear clipping in the face plane).

use glam::{Vec2, Vec3};

use super::{FaceId, Mesh};
use crate::math::newell_normal_slice;

impl Mesh {
    /// Triangulate all faces. Returns vertex index triples, wound like their faces.
    pub fn triangulate(&self) -> Vec<[u32; 3]> {
        self.triangulate_with_faces()
            .into_iter()
            .map(|(_, t)| t)
            .collect()
    }

    /// Triangulate all faces, tagging each triangle with its source face.
    pub fn triangulate_with_faces(&self) -> Vec<(FaceId, [u32; 3])> {
        let mut out = Vec::with_capacity(self.loops.len());
        let mut pts = Vec::new();
        let mut idx = Vec::new();
        for f in self.face_ids() {
            idx.clear();
            idx.extend(self.face_vertices(f).map(|v| v.0));
            if idx.len() == 3 {
                out.push((f, [idx[0], idx[1], idx[2]]));
                continue;
            }
            pts.clear();
            pts.extend(idx.iter().map(|&i| self.vertices[i as usize].position));
            for t in triangulate_polygon(&pts) {
                out.push((f, [idx[t[0]], idx[t[1]], idx[t[2]]]));
            }
        }
        out
    }
}

/// Triangulate a (possibly concave, roughly planar) polygon given by its corner
/// positions. Returns local corner index triples preserving winding.
pub fn triangulate_polygon(pts: &[Vec3]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    let normal = newell_normal_slice(pts);
    if normal.length_squared() <= f32::MIN_POSITIVE {
        return fan(n);
    }
    let normal = normal.normalize();
    // Right-handed basis (u × v = normal) so CCW in 3D stays CCW in 2D.
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    let p2: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.dot(u), p.dot(v))).collect();
    if n == 4 {
        // Fast path for quads: choose a diagonal that keeps both triangles CCW,
        // prefer the shorter one when both work.
        let ok02 = cross2(p2[0], p2[1], p2[2]) > 0.0 && cross2(p2[0], p2[2], p2[3]) > 0.0;
        let ok13 = cross2(p2[1], p2[2], p2[3]) > 0.0 && cross2(p2[1], p2[3], p2[0]) > 0.0;
        let d02 = pts[0].distance_squared(pts[2]);
        let d13 = pts[1].distance_squared(pts[3]);
        return if ok02 && (!ok13 || d02 <= d13) {
            vec![[0, 1, 2], [0, 2, 3]]
        } else if ok13 {
            vec![[1, 2, 3], [1, 3, 0]]
        } else {
            vec![[0, 1, 2], [0, 2, 3]]
        };
    }
    ear_clip(&p2)
}

fn fan(n: usize) -> Vec<[usize; 3]> {
    (1..n - 1).map(|i| [0, i, i + 1]).collect()
}

#[inline]
fn cross2(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}

fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = cross2(a, b, p);
    let d2 = cross2(b, c, p);
    let d3 = cross2(c, a, p);
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

fn ear_clip(p: &[Vec2]) -> Vec<[usize; 3]> {
    let mut remaining: Vec<usize> = (0..p.len()).collect();
    let mut tris = Vec::with_capacity(p.len() - 2);
    while remaining.len() > 3 {
        let m = remaining.len();
        let mut clipped = false;
        for i in 0..m {
            let ia = remaining[(i + m - 1) % m];
            let ib = remaining[i];
            let ic = remaining[(i + 1) % m];
            let (a, b, c) = (p[ia], p[ib], p[ic]);
            if cross2(a, b, c) <= 0.0 {
                continue; // reflex or degenerate corner
            }
            let blocked = remaining.iter().any(|&j| {
                j != ia
                    && j != ib
                    && j != ic
                    && p[j] != a
                    && p[j] != b
                    && p[j] != c
                    && point_in_triangle(p[j], a, b, c)
            });
            if blocked {
                continue;
            }
            tris.push([ia, ib, ic]);
            remaining.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Self-intersecting or fully degenerate remainder: fall back to a fan.
            for i in 1..remaining.len() - 1 {
                tris.push([remaining[0], remaining[i], remaining[i + 1]]);
            }
            return tris;
        }
    }
    tris.push([remaining[0], remaining[1], remaining[2]]);
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concave_l_shape() {
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        ];
        let tris = triangulate_polygon(&pts);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|t| (pts[t[1]] - pts[t[0]]).cross(pts[t[2]] - pts[t[0]]).z * 0.5)
            .sum();
        assert!((area - 3.0).abs() < 1e-5);
        for t in &tris {
            let z = (pts[t[1]] - pts[t[0]]).cross(pts[t[2]] - pts[t[0]]).z;
            assert!(z > 0.0, "triangle must keep winding");
        }
    }
}

//! Constructive solid geometry (union / difference / intersection).
//!
//! Algorithm: BSP-tree CSG (as popularized by csg.js) evaluated in `f64` on
//! normalized coordinates, followed by a repair pass that turns the polygon
//! soup back into a connected mesh:
//!
//! 1. vertices closer than a tolerance are welded (spatial hash),
//! 2. degenerate / repeated-vertex polygons are removed or split,
//! 3. zero-area slivers are dropped,
//! 4. T-junctions are repaired by inserting every vertex lying on a polygon
//!    edge into that edge,
//! 5. duplicate and mutually-opposite faces are cancelled.
//!
//! For closed, well-formed inputs this yields a closed manifold in the
//! common cases (overlapping boxes, box minus sphere, coplanar contacts).
//! The BSP is stored in an arena and processed iteratively, so deep trees do
//! not overflow the stack.

use glam::{DVec3, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::math::Aabb;
use crate::mesh::soup::Soup;
use crate::mesh::triangulate_polygon;
use crate::mesh::Mesh;

/// Boolean operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    /// A ∪ B
    Union,
    /// A − B
    Difference,
    /// A ∩ B
    Intersect,
}

/// Plane classification tolerance in normalized coordinates.
const PLANE_EPS: f64 = 1e-6;
/// Vertex welding / T-junction tolerance in normalized coordinates.
const WELD_EPS: f64 = 1e-6;

#[derive(Debug, Clone, Copy)]
struct Plane {
    n: DVec3,
    w: f64,
}

impl Plane {
    fn flip(&mut self) {
        self.n = -self.n;
        self.w = -self.w;
    }
}

#[derive(Debug, Clone)]
struct Poly {
    v: Vec<DVec3>,
    plane: Plane,
}

impl Poly {
    fn new(v: Vec<DVec3>) -> Option<Poly> {
        let c = v.iter().copied().sum::<DVec3>() / v.len() as f64;
        let mut n = DVec3::ZERO;
        for i in 0..v.len() {
            n += (v[i] - c).cross(v[(i + 1) % v.len()] - c);
        }
        let len = n.length();
        if len <= 1e-18 {
            return None;
        }
        let n = n / len;
        Some(Poly {
            plane: Plane { n, w: n.dot(c) },
            v,
        })
    }

    fn flip(&mut self) {
        self.v.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// Split `poly` by `plane` into the four output lists.
fn split_polygon(
    plane: &Plane,
    poly: Poly,
    coplanar_front: &mut Vec<Poly>,
    coplanar_back: &mut Vec<Poly>,
    front: &mut Vec<Poly>,
    back: &mut Vec<Poly>,
) {
    let mut ptype = 0u8;
    let types: Vec<u8> = poly
        .v
        .iter()
        .map(|p| {
            let t = plane.n.dot(*p) - plane.w;
            let ty = if t < -PLANE_EPS {
                BACK
            } else if t > PLANE_EPS {
                FRONT
            } else {
                COPLANAR
            };
            ptype |= ty;
            ty
        })
        .collect();
    match ptype {
        COPLANAR => {
            if plane.n.dot(poly.plane.n) > 0.0 {
                coplanar_front.push(poly);
            } else {
                coplanar_back.push(poly);
            }
        }
        FRONT => front.push(poly),
        BACK => back.push(poly),
        _ => {
            let n = poly.v.len();
            let mut f = Vec::with_capacity(n + 1);
            let mut b = Vec::with_capacity(n + 1);
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (poly.v[i], poly.v[j]);
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let t = (plane.w - plane.n.dot(vi)) / plane.n.dot(vj - vi);
                    let v = vi.lerp(vj, t);
                    f.push(v);
                    b.push(v);
                }
            }
            if f.len() >= 3 {
                front.push(Poly {
                    v: f,
                    plane: poly.plane,
                });
            }
            if b.len() >= 3 {
                back.push(Poly {
                    v: b,
                    plane: poly.plane,
                });
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BspNode {
    plane: Option<Plane>,
    front: Option<usize>,
    back: Option<usize>,
    polys: Vec<Poly>,
}

/// Arena-backed BSP tree.
#[derive(Debug, Clone)]
struct Bsp {
    nodes: Vec<BspNode>,
}

impl Bsp {
    fn new(polys: Vec<Poly>) -> Bsp {
        let mut b = Bsp {
            nodes: vec![BspNode::default()],
        };
        b.build(polys);
        b
    }

    fn build(&mut self, polys: Vec<Poly>) {
        let mut stack = vec![(0usize, polys)];
        while let Some((ni, ps)) = stack.pop() {
            if ps.is_empty() {
                continue;
            }
            let plane = *self.nodes[ni].plane.get_or_insert(ps[0].plane);
            let mut front = Vec::new();
            let mut back = Vec::new();
            let mut cop_f = Vec::new();
            let mut cop_b = Vec::new();
            for p in ps {
                split_polygon(&plane, p, &mut cop_f, &mut cop_b, &mut front, &mut back);
            }
            self.nodes[ni].polys.extend(cop_f);
            self.nodes[ni].polys.extend(cop_b);
            if !front.is_empty() {
                let c = match self.nodes[ni].front {
                    Some(c) => c,
                    None => {
                        self.nodes.push(BspNode::default());
                        let c = self.nodes.len() - 1;
                        self.nodes[ni].front = Some(c);
                        c
                    }
                };
                stack.push((c, front));
            }
            if !back.is_empty() {
                let c = match self.nodes[ni].back {
                    Some(c) => c,
                    None => {
                        self.nodes.push(BspNode::default());
                        let c = self.nodes.len() - 1;
                        self.nodes[ni].back = Some(c);
                        c
                    }
                };
                stack.push((c, back));
            }
        }
    }

    /// Convert solid space to empty space and vice versa.
    fn invert(&mut self) {
        for n in &mut self.nodes {
            for p in &mut n.polys {
                p.flip();
            }
            if let Some(pl) = &mut n.plane {
                pl.flip();
            }
            std::mem::swap(&mut n.front, &mut n.back);
        }
    }

    /// Remove the parts of `polys` that are inside this BSP's solid.
    fn clip_polygons(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, polys)];
        while let Some((ni, ps)) = stack.pop() {
            let node = &self.nodes[ni];
            let Some(plane) = node.plane else {
                out.extend(ps);
                continue;
            };
            let mut front = Vec::new();
            let mut back = Vec::new();
            for p in ps {
                let (mut cf, mut cb) = (Vec::new(), Vec::new());
                split_polygon(&plane, p, &mut cf, &mut cb, &mut front, &mut back);
                front.extend(cf);
                back.extend(cb);
            }
            match node.front {
                Some(c) => stack.push((c, front)),
                None => out.extend(front),
            }
            if let Some(c) = node.back {
                stack.push((c, back));
            }
        }
        out
    }

    /// Remove all polygons of this tree that are inside `other`.
    fn clip_to(&mut self, other: &Bsp) {
        for i in 0..self.nodes.len() {
            let ps = std::mem::take(&mut self.nodes[i].polys);
            self.nodes[i].polys = other.clip_polygons(ps);
        }
    }

    fn all_polygons(&self) -> Vec<Poly> {
        self.nodes
            .iter()
            .flat_map(|n| n.polys.iter().cloned())
            .collect()
    }
}

/// Is the polygon planar (within tolerance) and convex?
fn planar_convex(pts: &[DVec3], plane: &Plane) -> bool {
    let n = pts.len();
    if pts
        .iter()
        .any(|p| (plane.n.dot(*p) - plane.w).abs() > PLANE_EPS)
    {
        return false;
    }
    (0..n).all(|i| {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let c = pts[(i + 2) % n];
        (b - a).cross(c - b).dot(plane.n) >= -1e-12
    })
}

fn mesh_polys(mesh: &Mesh, center: Vec3, inv_scale: f64) -> Vec<Poly> {
    let to = |p: Vec3| (p - center).as_dvec3() * inv_scale;
    let mut out = Vec::with_capacity(mesh.face_count());
    for f in mesh.face_ids() {
        let pts: Vec<DVec3> = mesh
            .face_vertices(f)
            .map(|v| to(mesh.position(v)))
            .collect();
        let Some(poly) = Poly::new(pts.clone()) else {
            continue;
        };
        if pts.len() == 3 || planar_convex(&pts, &poly.plane) {
            out.push(poly);
        } else {
            let f32pts: Vec<Vec3> = mesh.face_vertices(f).map(|v| mesh.position(v)).collect();
            for t in triangulate_polygon(&f32pts) {
                if let Some(p) = Poly::new(vec![pts[t[0]], pts[t[1]], pts[t[2]]]) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Compute `a <op> b`. Both meshes must be in the same coordinate space and
/// should be closed (watertight) for meaningful results.
pub fn boolean(a: &Mesh, b: &Mesh, op: BooleanOp) -> CoreResult<Mesh> {
    let (ba, bb) = match (a.bounding_box(), b.bounding_box()) {
        (Some(x), Some(y)) if a.face_count() > 0 && b.face_count() > 0 => (x, y),
        _ => {
            return Err(CoreError::BooleanFailed(
                "both operands need at least one face".into(),
            ))
        }
    };
    let bounds: Aabb = ba.union(&bb);
    let center = bounds.center();
    let scale = (bounds.diagonal() as f64 * 0.5).max(1e-12);
    let inv = 1.0 / scale;
    let pa = mesh_polys(a, center, inv);
    let pb = mesh_polys(b, center, inv);
    let mut ta = Bsp::new(pa);
    let mut tb = Bsp::new(pb);
    let polys = match op {
        BooleanOp::Union => {
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            tb.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.build(tb.all_polygons());
            ta.all_polygons()
        }
        BooleanOp::Difference => {
            ta.invert();
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            tb.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.build(tb.all_polygons());
            ta.invert();
            ta.all_polygons()
        }
        BooleanOp::Intersect => {
            ta.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            ta.build(tb.all_polygons());
            ta.invert();
            ta.all_polygons()
        }
    };
    let soup = repair(polys);
    // Back to object space.
    let positions: Vec<Vec3> = soup
        .0
        .iter()
        .map(|p| (*p * scale).as_vec3() + center)
        .collect();
    let mut s = Soup {
        positions,
        faces: soup.1,
    };
    s.clean_faces();
    s.remove_unused_vertices(None);
    s.into_mesh()
}

/// Weld, clean, drop slivers and fix T-junctions. Returns positions + faces.
fn repair(polys: Vec<Poly>) -> (Vec<DVec3>, Vec<Vec<u32>>) {
    // 1. weld
    let mut positions: Vec<DVec3> = Vec::new();
    let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<u32>> = Default::default();
    let inv = 1.0 / WELD_EPS;
    let mut weld = |p: DVec3, positions: &mut Vec<DVec3>| -> u32 {
        let k = (
            (p.x * inv).floor() as i64,
            (p.y * inv).floor() as i64,
            (p.z * inv).floor() as i64,
        );
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        for &i in list {
                            if positions[i as usize].distance_squared(p) <= WELD_EPS * WELD_EPS {
                                return i;
                            }
                        }
                    }
                }
            }
        }
        positions.push(p);
        let i = (positions.len() - 1) as u32;
        grid.entry(k).or_default().push(i);
        i
    };
    let mut faces: Vec<Vec<u32>> = Vec::with_capacity(polys.len());
    for p in polys {
        let f: Vec<u32> = p.v.iter().map(|&v| weld(v, &mut positions)).collect();
        faces.push(f);
    }
    // 2. split / drop degenerate index lists
    let mut cleaned = Vec::with_capacity(faces.len());
    for f in faces {
        crate::mesh::soup::split_polygon(f, &mut cleaned);
    }
    // 3. drop zero-area slivers
    cleaned.retain(|f| {
        let c = f.iter().map(|&i| positions[i as usize]).sum::<DVec3>() / f.len() as f64;
        let mut n = DVec3::ZERO;
        for i in 0..f.len() {
            n += (positions[f[i] as usize] - c).cross(positions[f[(i + 1) % f.len()] as usize] - c);
        }
        n.length() * 0.5 > WELD_EPS * WELD_EPS
    });
    // 4. T-junctions
    let faces = fix_t_junctions(&positions, cleaned);
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        crate::mesh::soup::split_polygon(f, &mut out);
    }
    (positions, out)
}

fn fix_t_junctions(positions: &[DVec3], faces: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
    let mut used = vec![false; positions.len()];
    for f in &faces {
        for &v in f {
            used[v as usize] = true;
        }
    }
    let mut order: Vec<u32> = (0..positions.len() as u32)
        .filter(|&v| used[v as usize])
        .collect();
    order.sort_by(|&a, &b| positions[a as usize].x.total_cmp(&positions[b as usize].x));
    let xs: Vec<f64> = order.iter().map(|&i| positions[i as usize].x).collect();
    let eps = WELD_EPS * 2.0;
    faces
        .into_iter()
        .map(|f| {
            let n = f.len();
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let (ia, ib) = (f[i], f[(i + 1) % n]);
                out.push(ia);
                let (a, b) = (positions[ia as usize], positions[ib as usize]);
                let lo = a.min(b) - DVec3::splat(eps);
                let hi = a.max(b) + DVec3::splat(eps);
                let start = xs.partition_point(|&x| x < lo.x);
                let ab = b - a;
                let len2 = ab.length_squared();
                if len2 <= 0.0 {
                    continue;
                }
                let mut hits: Vec<(f64, u32)> = Vec::new();
                for k in start..xs.len() {
                    if xs[k] > hi.x {
                        break;
                    }
                    let vi = order[k];
                    if vi == ia || vi == ib {
                        continue;
                    }
                    let p = positions[vi as usize];
                    if p.y < lo.y || p.y > hi.y || p.z < lo.z || p.z > hi.z {
                        continue;
                    }
                    let t = (p - a).dot(ab) / len2;
                    if t <= 0.0 || t >= 1.0 {
                        continue;
                    }
                    let d2 = (a + ab * t).distance_squared(p);
                    if d2 <= eps * eps {
                        hits.push((t, vi));
                    }
                }
                hits.sort_by(|x, y| x.0.total_cmp(&y.0));
                hits.dedup_by_key(|h| h.1);
                out.extend(hits.into_iter().map(|h| h.1));
            }
            out
        })
        .collect()
}

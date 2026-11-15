//! Catmull–Clark subdivision.

use glam::Vec3;

use crate::error::{CoreError, CoreResult};
use crate::mesh::Mesh;

/// Maximum subdivision levels per operation (each level multiplies the face
/// count by ~4).
pub const MAX_SUBDIVISION_LEVELS: u32 = 4;

/// Apply `levels` iterations of Catmull–Clark subdivision to the whole mesh.
///
/// Boundary edges (one face) and non-manifold edges are treated as creases:
/// their edge points are midpoints and boundary vertices follow the cubic
/// B-spline boundary rule; boundary corners (two incident edges) stay fixed.
/// Every n-gon becomes n quads. Vertex layout of each level:
/// `[vertex points][edge points][face points]`.
pub fn subdivide_catmull_clark(mesh: &Mesh, levels: u32) -> CoreResult<Mesh> {
    if !(1..=MAX_SUBDIVISION_LEVELS).contains(&levels) {
        return Err(CoreError::param(
            "levels",
            format!("must be in 1..={MAX_SUBDIVISION_LEVELS}"),
        ));
    }
    let projected = mesh.face_count() as u64 * 4u64.pow(levels);
    if projected > 4_000_000 {
        return Err(CoreError::param(
            "levels",
            format!("would produce about {projected} faces; use fewer levels"),
        ));
    }
    let mut m = mesh.clone();
    for _ in 0..levels {
        m = cc_step(&m)?;
    }
    Ok(m)
}

fn cc_step(mesh: &Mesh) -> CoreResult<Mesh> {
    let nv = mesh.vertex_count();
    let ne = mesh.edge_count();
    let nf = mesh.face_count();
    let face_pts: Vec<Vec3> = mesh.face_ids().map(|f| mesh.face_centroid(f)).collect();
    let crease = |e| mesh.edge_face_count(e) != 2;
    let edge_pts: Vec<Vec3> = mesh
        .edge_ids()
        .map(|e| {
            let [a, b] = mesh.edge_vertices(e);
            let (pa, pb) = (mesh.position(a), mesh.position(b));
            if crease(e) {
                (pa + pb) * 0.5
            } else {
                let fs = mesh.edge_faces(e);
                (pa + pb + face_pts[fs[0].index()] + face_pts[fs[1].index()]) * 0.25
            }
        })
        .collect();
    let mut positions = Vec::with_capacity(nv + ne + nf);
    for v in mesh.vertex_ids() {
        let p = mesh.position(v);
        let edges = mesh.vertex_edges(v);
        if edges.is_empty() {
            positions.push(p);
            continue;
        }
        let creases: Vec<_> = edges.iter().copied().filter(|&e| crease(e)).collect();
        let np = if creases.is_empty() {
            let n = edges.len() as f32;
            let faces = mesh.vertex_faces(v);
            let f_avg =
                faces.iter().map(|f| face_pts[f.index()]).sum::<Vec3>() / faces.len() as f32;
            let r_avg = edges.iter().map(|&e| mesh.edge_midpoint(e)).sum::<Vec3>() / n;
            (f_avg + 2.0 * r_avg + (n - 3.0) * p) / n
        } else if creases.len() == 2 && edges.len() > 2 {
            let a = mesh.position(mesh.edge_other_vertex(creases[0], v));
            let b = mesh.position(mesh.edge_other_vertex(creases[1], v));
            p * 0.75 + (a + b) * 0.125
        } else {
            p
        };
        positions.push(np);
    }
    positions.extend(edge_pts);
    positions.extend(face_pts);
    let e_base = nv as u32;
    let f_base = (nv + ne) as u32;
    let mut faces: Vec<[u32; 4]> = Vec::with_capacity(mesh.loop_count());
    for f in mesh.face_ids() {
        let loops: Vec<_> = mesh.face_loops(f).collect();
        let k = loops.len();
        for i in 0..k {
            let l = loops[i];
            let lp = loops[(i + k - 1) % k];
            faces.push([
                mesh.loop_vertex(l).0,
                e_base + mesh.loop_edge(l).0,
                f_base + f.0,
                e_base + mesh.loop_edge(lp).0,
            ]);
        }
    }
    Mesh::from_polygons(positions, &faces)
}

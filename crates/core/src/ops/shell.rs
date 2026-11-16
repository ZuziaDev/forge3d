//! Solidify (shell) and array duplication.

use glam::Vec3;

use super::{finite, loose_mask};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::Soup;
use crate::mesh::Mesh;

/// Maximum number of array copies.
pub const MAX_ARRAY_COUNT: u32 = 1000;

/// Give the surface a thickness: an inner shell is offset by `thickness`
/// against the vertex normals (negative values grow outward) with reversed
/// winding, and every boundary edge gets a rim quad joining both shells. A
/// closed mesh becomes two nested closed shells; an open one a closed solid.
/// Offsets are corrected for corner angles so walls keep an even thickness.
pub fn solidify(mesh: &Mesh, thickness: f32) -> CoreResult<Mesh> {
    finite("thickness", thickness)?;
    if thickness.abs() < 1e-6 {
        return Err(CoreError::param("thickness", "must be non-zero"));
    }
    if mesh.face_count() == 0 {
        return Err(CoreError::InvalidMesh(
            "mesh has no faces to solidify".into(),
        ));
    }
    if let Some(e) = mesh.edge_ids().find(|&e| mesh.is_non_manifold_edge(e)) {
        return Err(CoreError::NonManifold(format!(
            "edge {} has more than two faces; solidify needs a manifold surface",
            e.0
        )));
    }
    let n = mesh.vertex_count();
    let mut sum = vec![Vec3::ZERO; n];
    for f in mesh.face_ids() {
        let fnorm = mesh.face_normal(f);
        for l in mesh.face_loops(f) {
            let v = mesh.loop_vertex(l);
            let p = mesh.position(v);
            let a = (mesh.position(mesh.loop_vertex(mesh.loop_prev(l))) - p).normalize_or_zero();
            let b = (mesh.position(mesh.loop_target(l)) - p).normalize_or_zero();
            let angle = a.dot(b).clamp(-1.0, 1.0).acos();
            sum[v.index()] += fnorm * angle;
        }
    }
    let (positions, mut faces) = mesh.to_polygons();
    let mut out = positions.clone();
    for v in mesh.vertex_ids() {
        let dir = sum[v.index()].normalize_or_zero();
        // Even thickness: divide by the mean cosine to the adjacent faces.
        let fs = mesh.vertex_faces(v);
        let mean_cos = if fs.is_empty() {
            1.0
        } else {
            fs.iter()
                .map(|&f| dir.dot(mesh.face_normal(f)))
                .sum::<f32>()
                / fs.len() as f32
        };
        let scale = 1.0 / mean_cos.clamp(0.5, 1.0);
        out.push(positions[v.index()] - dir * thickness * scale);
    }
    let nu = n as u32;
    let inner: Vec<Vec<u32>> = faces
        .iter()
        .map(|f| f.iter().rev().map(|&v| v + nu).collect())
        .collect();
    for e in mesh.boundary_edges() {
        let l = mesh.edge_loops(e)[0];
        let (a, b) = (mesh.loop_vertex(l).0, mesh.loop_target(l).0);
        faces.push(vec![b, a, a + nu, b + nu]);
    }
    faces.extend(inner);
    let mut soup = Soup {
        positions: out,
        faces,
    };
    let mut keep = loose_mask(mesh);
    keep.resize(2 * n, false);
    soup.remove_unused_vertices(Some(&keep));
    soup.into_mesh()
}

/// `count` copies of the mesh, each translated by `offset` from the previous
/// one (local units). With `merge`, coincident vertices (within `threshold`)
/// of neighbouring copies are welded.
pub fn array(
    mesh: &Mesh,
    count: u32,
    offset: Vec3,
    merge: bool,
    threshold: f32,
) -> CoreResult<Mesh> {
    if !(1..=MAX_ARRAY_COUNT).contains(&count) {
        return Err(CoreError::param(
            "count",
            format!("must be in 1..={MAX_ARRAY_COUNT}"),
        ));
    }
    if !offset.is_finite() {
        return Err(CoreError::param("offset", "must contain finite numbers"));
    }
    if mesh.vertex_count() * count as usize > 4_000_000 {
        return Err(CoreError::param(
            "count",
            "result would exceed 4 million vertices",
        ));
    }
    let (positions, polys) = mesh.to_polygons();
    let nv = positions.len() as u32;
    let mut all_pos = Vec::with_capacity(positions.len() * count as usize);
    let mut all_faces = Vec::with_capacity(polys.len() * count as usize);
    for k in 0..count {
        let t = offset * k as f32;
        all_pos.extend(positions.iter().map(|&p| p + t));
        all_faces.extend(
            polys
                .iter()
                .map(|f| f.iter().map(|&v| v + k * nv).collect::<Vec<u32>>()),
        );
    }
    let mut out = Mesh::from_polygons(all_pos, &all_faces)?;
    if merge && count > 1 {
        out = super::merge_by_distance(&out, None, threshold)?.0.mesh;
    }
    Ok(out)
}

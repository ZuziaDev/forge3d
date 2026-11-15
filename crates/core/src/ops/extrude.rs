//! Region extrusion of faces.

use glam::Vec3;

use super::{check_faces, finite, loose_mask, require_nonempty, OpResult};
use crate::error::CoreResult;
use crate::mesh::soup::{Soup, REMOVED};
use crate::mesh::{FaceId, Mesh};
use crate::selection::ElementSelection;

/// Extrude a region of faces.
///
/// The selected faces are moved, and side quads are created along the
/// region's boundary (edges shared by two selected faces are *not*
/// duplicated). Each moved vertex is displaced by `offset` when given,
/// otherwise by `distance` along the average normal of its adjacent
/// selected faces. Edges of selected faces lying on the mesh boundary also
/// get side walls (extruding a plane produces an open box).
///
/// The extruded (moved) faces are selected afterwards.
pub fn extrude_faces(
    mesh: &Mesh,
    faces: &[FaceId],
    distance: f32,
    offset: Option<Vec3>,
) -> CoreResult<OpResult> {
    require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    finite("distance", distance)?;
    if let Some(o) = offset {
        if !o.is_finite() {
            return Err(crate::error::CoreError::param("offset", "must be finite"));
        }
    }
    let nv = mesh.vertex_count();
    let mut sel = vec![false; mesh.face_count()];
    for f in faces {
        sel[f.index()] = true;
    }
    let keep = loose_mask(mesh);
    let mut soup = Soup::from_mesh(mesh);
    let mut newv = vec![REMOVED; nv];
    let mut dir = vec![Vec3::ZERO; nv];
    let selected: Vec<FaceId> = mesh.face_ids().filter(|f| sel[f.index()]).collect();
    for &f in &selected {
        let n = mesh.face_normal(f);
        for v in mesh.face_vertices(f) {
            if newv[v.index()] == REMOVED {
                newv[v.index()] = soup.add_vertex(mesh.position(v));
            }
            dir[v.index()] += n;
        }
    }
    // Side walls along the region boundary.
    for &f in &selected {
        for l in mesh.face_loops(f) {
            let e = mesh.loop_edge(l);
            let interior = mesh
                .edge_loops(e)
                .into_iter()
                .any(|r| r != l && sel[mesh.loop_face(r).index()]);
            if interior {
                continue;
            }
            let a = mesh.loop_vertex(l).0;
            let b = mesh.loop_target(l).0;
            soup.faces
                .push(vec![a, b, newv[b as usize], newv[a as usize]]);
        }
    }
    for &f in &selected {
        for v in soup.faces[f.index()].iter_mut() {
            *v = newv[*v as usize];
        }
    }
    for v in 0..nv {
        let nvx = newv[v];
        if nvx == REMOVED {
            continue;
        }
        let delta = match offset {
            Some(o) => o,
            None => dir[v].normalize_or_zero() * distance,
        };
        soup.positions[nvx as usize] += delta;
    }
    let mut keep_all = keep;
    keep_all.resize(soup.positions.len(), false);
    soup.remove_unused_vertices(Some(&keep_all));
    let mesh = soup.into_mesh()?;
    let mut selection = ElementSelection::default();
    selection.faces.extend(selected.iter().map(|f| f.0));
    Ok(OpResult { mesh, selection })
}

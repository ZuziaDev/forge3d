//! Face inset (region or individual).

use glam::Vec3;

use super::{check_faces, finite, loose_mask, require_nonempty, OpResult};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::{Soup, REMOVED};
use crate::mesh::{FaceId, LoopId, Mesh};
use crate::selection::ElementSelection;

/// Smallest cosine used for miter offsets (limits spikes at sharp corners).
const MIN_MITER_COS: f32 = 0.2;

/// Inset faces by `thickness` (in the face plane) and move the inner faces
/// by `depth` along their normals.
///
/// * `individual == false`: the selection is inset as one region; only its
///   outer boundary gets a rim of quads.
/// * `individual == true`: every face is inset on its own.
///
/// The inner faces are selected afterwards.
pub fn inset_faces(
    mesh: &Mesh,
    faces: &[FaceId],
    thickness: f32,
    depth: f32,
    individual: bool,
) -> CoreResult<OpResult> {
    require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    finite("depth", depth)?;
    if !(thickness.is_finite() && thickness > 0.0) {
        return Err(CoreError::param(
            "thickness",
            "must be a positive finite number",
        ));
    }
    let mut sel = vec![false; mesh.face_count()];
    for f in faces {
        sel[f.index()] = true;
    }
    let selected: Vec<FaceId> = mesh.face_ids().filter(|f| sel[f.index()]).collect();
    let mut keep = loose_mask(mesh);
    let mut soup = Soup::from_mesh(mesh);
    if individual {
        inset_individual(mesh, &selected, thickness, depth, &mut soup);
    } else {
        inset_region(mesh, &selected, &sel, thickness, depth, &mut soup)?;
    }
    keep.resize(soup.positions.len(), false);
    soup.remove_unused_vertices(Some(&keep));
    let mesh = soup.into_mesh()?;
    let mut selection = ElementSelection::default();
    selection.faces.extend(selected.iter().map(|f| f.0));
    Ok(OpResult { mesh, selection })
}

/// In-plane inward offset of a corner between an incoming edge direction
/// `d_in` (prev → v) and outgoing `d_out` (v → next).
fn miter(n_in: Vec3, d_in: Vec3, n_out: Vec3, d_out: Vec3, thickness: f32) -> Vec3 {
    let in_a = n_in.cross(d_in).normalize_or_zero();
    let in_b = n_out.cross(d_out).normalize_or_zero();
    let m = (in_a + in_b).normalize_or_zero();
    if m == Vec3::ZERO {
        return in_a * thickness;
    }
    let c = m.dot(in_a).max(MIN_MITER_COS);
    m * (thickness / c)
}

fn inset_individual(mesh: &Mesh, selected: &[FaceId], thickness: f32, depth: f32, soup: &mut Soup) {
    for &f in selected {
        let n = mesh.face_normal(f);
        let verts: Vec<u32> = mesh.face_vertices(f).map(|v| v.0).collect();
        let k = verts.len();
        let pos: Vec<Vec3> = verts.iter().map(|&v| soup.positions[v as usize]).collect();
        let mut inner = Vec::with_capacity(k);
        for i in 0..k {
            let prev = pos[(i + k - 1) % k];
            let next = pos[(i + 1) % k];
            let d_in = (pos[i] - prev).normalize_or_zero();
            let d_out = (next - pos[i]).normalize_or_zero();
            let p = pos[i] + miter(n, d_in, n, d_out, thickness) + n * depth;
            inner.push(soup.add_vertex(p));
        }
        for i in 0..k {
            let j = (i + 1) % k;
            soup.faces
                .push(vec![verts[i], verts[j], inner[j], inner[i]]);
        }
        soup.faces[f.index()] = inner;
    }
}

fn inset_region(
    mesh: &Mesh,
    selected: &[FaceId],
    sel: &[bool],
    thickness: f32,
    depth: f32,
    soup: &mut Soup,
) -> CoreResult<()> {
    let nv = mesh.vertex_count();
    // Region-boundary half-edges.
    let mut out_l: Vec<Option<LoopId>> = vec![None; nv];
    let mut in_l: Vec<Option<LoopId>> = vec![None; nv];
    let mut boundary: Vec<LoopId> = Vec::new();
    for &f in selected {
        for l in mesh.face_loops(f) {
            let e = mesh.loop_edge(l);
            let interior = mesh
                .edge_loops(e)
                .into_iter()
                .any(|r| r != l && sel[mesh.loop_face(r).index()]);
            if interior {
                continue;
            }
            let a = mesh.loop_vertex(l).index();
            let b = mesh.loop_target(l).index();
            for (slot, v) in [(&mut out_l[a], a), (&mut in_l[b], b)] {
                if slot.replace(l).is_some() {
                    return Err(CoreError::InvalidSelection(format!(
                        "the selected region touches itself at vertex {v}; use individual inset"
                    )));
                }
            }
            boundary.push(l);
        }
    }
    if boundary.is_empty() {
        return Err(CoreError::InvalidSelection(
            "the selected faces form a closed region without boundary; nothing to inset".into(),
        ));
    }
    let mut newv = vec![REMOVED; nv];
    for v in 0..nv {
        let (Some(lo), Some(li)) = (out_l[v], in_l[v]) else {
            continue;
        };
        let p = mesh.position(mesh.loop_vertex(lo));
        let prev = mesh.position(mesh.loop_vertex(li));
        let next = mesh.position(mesh.loop_target(lo));
        let n_in = mesh.face_normal(mesh.loop_face(li));
        let n_out = mesh.face_normal(mesh.loop_face(lo));
        let d_in = (p - prev).normalize_or_zero();
        let d_out = (next - p).normalize_or_zero();
        let n_avg = (n_in + n_out).normalize_or_zero();
        let q = p + miter(n_in, d_in, n_out, d_out, thickness) + n_avg * depth;
        newv[v] = soup.add_vertex(q);
    }
    // Interior vertices of the region move by `depth` along their normal.
    if depth != 0.0 {
        let mut moved = vec![false; nv];
        let mut acc = vec![Vec3::ZERO; nv];
        for &f in selected {
            for v in mesh.face_vertices(f) {
                acc[v.index()] += mesh.face_normal(f);
            }
        }
        for &f in selected {
            for v in mesh.face_vertices(f) {
                let i = v.index();
                if newv[i] == REMOVED && !moved[i] {
                    moved[i] = true;
                    soup.positions[i] += acc[i].normalize_or_zero() * depth;
                }
            }
        }
    }
    for &l in &boundary {
        let a = mesh.loop_vertex(l).0;
        let b = mesh.loop_target(l).0;
        soup.faces
            .push(vec![a, b, newv[b as usize], newv[a as usize]]);
    }
    for &f in selected {
        for v in soup.faces[f.index()].iter_mut() {
            let nvx = newv[*v as usize];
            if nvx != REMOVED {
                *v = nvx;
            }
        }
    }
    Ok(())
}

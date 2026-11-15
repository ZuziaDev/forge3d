//! Vertex merging.

use super::{check_vertices, loose_mask, OpResult};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::{weld_map, Soup, REMOVED};
use crate::mesh::{Mesh, VertexId};
use crate::selection::ElementSelection;

/// Collapse a set of (≥ 2) vertices into one vertex at their average
/// position. Faces that degenerate are removed; faces that become
/// figure-eights are split. The merged vertex is selected afterwards.
pub fn merge_at_center(mesh: &Mesh, verts: &[VertexId]) -> CoreResult<OpResult> {
    check_vertices(mesh, verts)?;
    let mut uniq: Vec<VertexId> = verts.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    if uniq.len() < 2 {
        return Err(CoreError::InvalidSelection(
            "merge at center needs at least two vertices".into(),
        ));
    }
    let center = super::vertices_center(mesh, &uniq);
    let target = uniq[0].0;
    let mut soup = Soup::from_mesh(mesh);
    let mut remap: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    for v in &uniq {
        remap[v.index()] = target;
    }
    soup.positions[target as usize] = center;
    finish(mesh, soup, &remap, Some(target))
}

/// Weld vertices closer than `threshold`. Only the given vertices take part
/// (all vertices when `verts` is `None`). Returns the result and the number
/// of vertices removed. Merged vertices are selected afterwards.
pub fn merge_by_distance(
    mesh: &Mesh,
    verts: Option<&[VertexId]>,
    threshold: f32,
) -> CoreResult<(OpResult, usize)> {
    if !(threshold.is_finite() && threshold >= 0.0) {
        return Err(CoreError::param(
            "threshold",
            "must be a finite number >= 0",
        ));
    }
    let subset = match verts {
        Some(v) => {
            check_vertices(mesh, v)?;
            let mut mask = vec![false; mesh.vertex_count()];
            for x in v {
                mask[x.index()] = true;
            }
            Some(mask)
        }
        None => None,
    };
    let remap = weld_map(&mesh.positions(), threshold, subset.as_deref());
    let soup = Soup::from_mesh(mesh);
    let before = mesh.vertex_count();
    let res = finish(mesh, soup, &remap, None)?;
    let removed = before - res.mesh.vertex_count();
    Ok((res, removed))
}

fn finish(mesh: &Mesh, mut soup: Soup, remap: &[u32], select: Option<u32>) -> CoreResult<OpResult> {
    let loose = loose_mask(mesh);
    // Keep loose vertices that are not merged away.
    let keep: Vec<bool> = (0..mesh.vertex_count())
        .map(|v| loose[v] && remap[v] == v as u32)
        .collect();
    soup.remap_faces(remap);
    soup.clean_faces();
    // Representatives of merged clusters must survive even if all their
    // faces collapsed (Blender keeps the merged vertex).
    let mut keep = keep;
    let mut reps: Vec<u32> = Vec::new();
    for (v, &r) in remap.iter().enumerate() {
        if r != v as u32 {
            keep[r as usize] = true;
            reps.push(r);
        }
    }
    let final_map = soup.remove_unused_vertices(Some(&keep));
    let mesh = soup.into_mesh()?;
    let mut selection = ElementSelection::default();
    match select {
        Some(t) => {
            if final_map[t as usize] != REMOVED {
                selection.vertices.insert(final_map[t as usize]);
            }
        }
        None => {
            for r in reps {
                if final_map[r as usize] != REMOVED {
                    selection.vertices.insert(final_map[r as usize]);
                }
            }
        }
    }
    Ok(OpResult { mesh, selection })
}

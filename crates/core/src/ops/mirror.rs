//! Mirror geometry across a local axis plane.

use crate::error::{CoreError, CoreResult};
use crate::math::Axis;
use crate::mesh::soup::Soup;
use crate::mesh::Mesh;

use super::loose_mask;

/// Mirror the mesh across the plane through the local origin perpendicular
/// to `axis`, keeping the original. The copy has reversed winding so
/// normals stay outward. With `merge`, vertices within `threshold` of the
/// mirror plane are snapped onto it and shared with their mirror image, and
/// faces lying in the plane (which would become internal) are removed.
pub fn mirror(mesh: &Mesh, axis: Axis, merge: bool, threshold: f32) -> CoreResult<Mesh> {
    if !(threshold.is_finite() && threshold >= 0.0) {
        return Err(CoreError::param(
            "merge_threshold",
            "must be a finite number >= 0",
        ));
    }
    let ax = axis.index();
    let n = mesh.vertex_count() as u32;
    let mut soup = Soup::from_mesh(mesh);
    let mut remap: Vec<u32> = (0..2 * n).collect();
    if merge {
        for v in 0..n as usize {
            if soup.positions[v][ax].abs() <= threshold {
                soup.positions[v][ax] = 0.0;
                remap[v + n as usize] = v as u32;
            }
        }
    }
    let mirrored: Vec<_> = soup
        .positions
        .iter()
        .map(|p| {
            let mut q = *p;
            q[ax] = -q[ax];
            q
        })
        .collect();
    soup.positions.extend(mirrored);
    let copies: Vec<Vec<u32>> = soup
        .faces
        .iter()
        .map(|f| f.iter().rev().map(|&v| remap[(v + n) as usize]).collect())
        .collect();
    soup.faces.extend(copies);
    soup.clean_faces();
    let mut keep = loose_mask(mesh);
    let mirrored_keep: Vec<bool> = (0..n as usize)
        .map(|v| keep[v] && remap[v + n as usize] == v as u32 + n)
        .collect();
    keep.extend(mirrored_keep);
    soup.remove_unused_vertices(Some(&keep));
    soup.into_mesh()
}

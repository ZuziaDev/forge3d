//! Mesh editing operations.
//!
//! All operations are pure functions taking a mesh (plus element handles and
//! parameters) and returning a new mesh, so they are trivially undoable via
//! snapshots and safe to use from any layer. They are wrapped as commands in
//! [`crate::commands`].

mod bevel;
pub mod boolean;
mod extrude;
mod inset;
mod loopcut;
mod merge;
mod mirror;
mod normals;
mod subdivide;

use glam::{Affine3A, Vec3};

use crate::error::{CoreError, CoreResult};
use crate::mesh::{EdgeId, ElementKind, FaceId, Mesh, VertexId};
use crate::selection::ElementSelection;

pub use bevel::bevel_edges;
pub use boolean::{boolean, BooleanOp};
pub use extrude::extrude_faces;
pub use inset::inset_faces;
pub use loopcut::loop_cut;
pub use merge::{merge_at_center, merge_by_distance};
pub use mirror::mirror;
pub use normals::{flip_normals, recalculate_normals};
pub use subdivide::{subdivide_catmull_clark, MAX_SUBDIVISION_LEVELS};

/// Result of an element-level operation: the new mesh and the elements that
/// should be selected afterwards (indices refer to the new mesh).
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    pub mesh: Mesh,
    pub selection: ElementSelection,
}

pub(crate) fn require_nonempty<T>(items: &[T], what: &str) -> CoreResult<()> {
    if items.is_empty() {
        Err(CoreError::EmptySelection(format!("no {what} selected")))
    } else {
        Ok(())
    }
}

pub(crate) fn check_faces(mesh: &Mesh, faces: &[FaceId]) -> CoreResult<()> {
    for f in faces {
        mesh.check_index(ElementKind::Face, f.0)?;
    }
    Ok(())
}

pub(crate) fn check_edges(mesh: &Mesh, edges: &[EdgeId]) -> CoreResult<()> {
    for e in edges {
        mesh.check_index(ElementKind::Edge, e.0)?;
    }
    Ok(())
}

pub(crate) fn check_vertices(mesh: &Mesh, verts: &[VertexId]) -> CoreResult<()> {
    for v in verts {
        mesh.check_index(ElementKind::Vertex, v.0)?;
    }
    Ok(())
}

pub(crate) fn finite(name: &str, v: f32) -> CoreResult<()> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(CoreError::param(name, "must be a finite number"))
    }
}

/// Vertices originally not used by any face (kept through compaction).
pub(crate) fn loose_mask(mesh: &Mesh) -> Vec<bool> {
    mesh.vertex_ids()
        .map(|v| mesh.vertex_edges(v).is_empty())
        .collect()
}

/// Delete elements: vertices remove adjacent faces; edges remove adjacent
/// faces and vertices left without faces; faces remove themselves and
/// vertices left without faces.
pub fn delete_elements(mesh: &Mesh, kind: ElementKind, indices: &[u32]) -> CoreResult<Mesh> {
    require_nonempty(indices, "elements")?;
    let mut m = mesh.clone();
    match kind {
        ElementKind::Vertex => {
            let v: Vec<VertexId> = indices.iter().map(|&i| VertexId(i)).collect();
            m.remove_vertices(&v)?;
        }
        ElementKind::Edge => {
            let e: Vec<EdgeId> = indices.iter().map(|&i| EdgeId(i)).collect();
            m.remove_edges(&e)?;
        }
        ElementKind::Face => {
            let f: Vec<FaceId> = indices.iter().map(|&i| FaceId(i)).collect();
            m.remove_faces(&f, true)?;
        }
    }
    Ok(m)
}

/// Transform a subset of vertices with `m` (in mesh-local space).
pub fn transform_vertices(mesh: &Mesh, verts: &[VertexId], m: &Affine3A) -> CoreResult<Mesh> {
    check_vertices(mesh, verts)?;
    let mut out = mesh.clone();
    let mut done = vec![false; mesh.vertex_count()];
    for &v in verts {
        if !std::mem::replace(&mut done[v.index()], true) {
            out.set_position(v, m.transform_point3(mesh.position(v)));
        }
    }
    out.recalc_normals();
    Ok(out)
}

/// Median (average) point of a set of vertices.
pub fn vertices_center(mesh: &Mesh, verts: &[VertexId]) -> Vec3 {
    if verts.is_empty() {
        return Vec3::ZERO;
    }
    verts.iter().map(|&v| mesh.position(v)).sum::<Vec3>() / verts.len() as f32
}

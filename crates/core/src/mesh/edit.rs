//! In-place edits: element removal with compaction, transforms, merging meshes.

use glam::Affine3A;

use super::soup::{Soup, REMOVED};
use super::{EdgeId, FaceId, Mesh, VertexId};
use crate::error::CoreResult;

/// Old → new index tables produced by compacting operations
/// (`u32::MAX` marks a removed element).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Remap {
    pub vertices: Vec<u32>,
    pub faces: Vec<u32>,
}

impl Remap {
    pub const REMOVED: u32 = REMOVED;
}

impl Mesh {
    /// Remove faces and compact. When `remove_isolated` is set, vertices that
    /// were used by the removed faces and are no longer used are removed too
    /// (pre-existing loose vertices are kept).
    pub fn remove_faces(&mut self, faces: &[FaceId], remove_isolated: bool) -> CoreResult<Remap> {
        let mut drop = vec![false; self.face_count()];
        for f in faces {
            self.check_index(super::ElementKind::Face, f.0)?;
            drop[f.index()] = true;
        }
        let was_used: Vec<bool> = self
            .vertex_ids()
            .map(|v| !self.vertex_edges(v).is_empty())
            .collect();
        self.rebuild_filtered(&drop, |v| !remove_isolated || !was_used[v as usize])
    }

    /// Remove vertices together with every face using them, then compact.
    pub fn remove_vertices(&mut self, verts: &[VertexId]) -> CoreResult<Remap> {
        let mut dead = vec![false; self.vertex_count()];
        for v in verts {
            self.check_index(super::ElementKind::Vertex, v.0)?;
            dead[v.index()] = true;
        }
        let drop: Vec<bool> = self
            .face_ids()
            .map(|f| self.face_vertices(f).any(|v| dead[v.index()]))
            .collect();
        self.rebuild_filtered(&drop, |v| !dead[v as usize])
    }

    /// Remove edges together with their faces; vertices left without faces
    /// are removed as well. Compacts the mesh.
    pub fn remove_edges(&mut self, edges: &[EdgeId]) -> CoreResult<Remap> {
        let mut drop = vec![false; self.face_count()];
        for e in edges {
            self.check_index(super::ElementKind::Edge, e.0)?;
            for f in self.edge_faces(*e) {
                drop[f.index()] = true;
            }
        }
        let was_used: Vec<bool> = self
            .vertex_ids()
            .map(|v| !self.vertex_edges(v).is_empty())
            .collect();
        self.rebuild_filtered(&drop, |v| !was_used[v as usize])
    }

    /// Remove all vertices that are not used by any face.
    pub fn remove_isolated_vertices(&mut self) -> CoreResult<Remap> {
        let drop = vec![false; self.face_count()];
        self.rebuild_filtered(&drop, |_| false)
    }

    /// Rebuild keeping faces with `drop[f] == false`; unused vertices are kept
    /// only when `keep_unused(v)` returns true.
    fn rebuild_filtered(
        &mut self,
        drop: &[bool],
        keep_unused: impl Fn(u32) -> bool,
    ) -> CoreResult<Remap> {
        let (positions, faces) = self.to_polygons();
        let mut face_remap = vec![REMOVED; faces.len()];
        let mut kept = Vec::with_capacity(faces.len());
        for (i, f) in faces.into_iter().enumerate() {
            if !drop[i] {
                face_remap[i] = kept.len() as u32;
                kept.push(f);
            }
        }
        let keep: Vec<bool> = (0..positions.len() as u32).map(&keep_unused).collect();
        let mut soup = Soup {
            positions,
            faces: kept,
        };
        let vremap = soup.remove_unused_vertices(Some(&keep));
        *self = soup.into_mesh()?;
        Ok(Remap {
            vertices: vremap,
            faces: face_remap,
        })
    }

    /// Apply an affine transform to all vertices. A mirroring transform
    /// (negative determinant) also reverses face winding so normals stay outward.
    pub fn transform(&mut self, m: &Affine3A) {
        for v in &mut self.vertices {
            v.position = m.transform_point3(v.position);
        }
        if m.matrix3.determinant() < 0.0 {
            self.flip_all_faces();
        } else {
            self.recalc_normals();
        }
    }

    /// Reverse the winding of every face.
    pub fn flip_all_faces(&mut self) {
        let all: Vec<FaceId> = self.face_ids().collect();
        self.flip_faces(&all);
    }

    /// Reverse the winding of the given faces (rebuilds topology).
    pub fn flip_faces(&mut self, faces: &[FaceId]) {
        let (positions, mut polys) = self.to_polygons();
        for f in faces {
            if let Some(p) = polys.get_mut(f.index()) {
                p.reverse();
            }
        }
        *self = Mesh::from_polygons(positions, &polys).expect("flipping keeps polygons valid");
    }

    /// Append another mesh (vertex/face indices of `other` are offset).
    pub fn append(&mut self, other: &Mesh) {
        let (mut positions, mut polys) = self.to_polygons();
        let offset = positions.len() as u32;
        let (p2, f2) = other.to_polygons();
        positions.extend(p2);
        polys.extend(
            f2.into_iter()
                .map(|f| f.into_iter().map(|v| v + offset).collect::<Vec<u32>>()),
        );
        *self = Mesh::from_polygons(positions, &polys).expect("appending valid meshes stays valid");
    }
}

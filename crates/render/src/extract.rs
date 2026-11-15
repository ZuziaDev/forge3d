//! Scene extraction: [`Scene`] (+ optional [`Selection`]) → draw data.
//!
//! [`MeshGeometry`] is the renderer-friendly form of one mesh: unwelded
//! triangle corners with normals and source face ids, the real mesh edges
//! (not triangulation diagonals) and a face → corner/edge table for picking.

use std::collections::BTreeSet;

use forge3d_core::math::{newell_normal_slice, Aabb};
use forge3d_core::mesh::{EdgeId, FaceId, Mesh};
use forge3d_core::scene::{Material, Scene};
use forge3d_core::selection::{SelectMode, Selection};
use glam::{Affine3A, Mat3, Vec3};
use uuid::Uuid;

/// Renderable geometry of one mesh (in the space of its source: local for
/// [`MeshGeometry::from_mesh`], world after [`MeshGeometry::transformed`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeshGeometry {
    /// Triangle corner positions, 3 per triangle (unwelded).
    pub positions: Vec<Vec3>,
    /// Triangle corner normals (unit), 3 per triangle.
    pub normals: Vec<Vec3>,
    /// Source face of each triangle.
    pub tri_faces: Vec<u32>,
    /// Mesh vertex positions (index = vertex id).
    pub vertices: Vec<Vec3>,
    /// Mesh edges as vertex index pairs (index = edge id).
    pub edges: Vec<[u32; 2]>,
    /// CSR offsets into `face_corners` / `face_edges` (len = faces + 1).
    pub face_offsets: Vec<u32>,
    /// Vertex ids of each face in winding order.
    pub face_corners: Vec<u32>,
    /// Edge ids of each face in winding order (edge i joins corner i and i+1).
    pub face_edges: Vec<u32>,
}

impl MeshGeometry {
    /// Build from a mesh. Normals are computed from the positions (Newell),
    /// flat per face or smooth (area-weighted per vertex).
    pub fn from_mesh(mesh: &Mesh, smooth: bool) -> MeshGeometry {
        let vertices = mesh.positions();
        let nf = mesh.face_count();
        let mut face_offsets = Vec::with_capacity(nf + 1);
        let mut face_corners = Vec::with_capacity(mesh.loop_count());
        let mut face_edges = Vec::with_capacity(mesh.loop_count());
        let mut face_normals = Vec::with_capacity(nf);
        let mut vertex_normals = vec![Vec3::ZERO; if smooth { vertices.len() } else { 0 }];
        let mut pts = Vec::new();
        face_offsets.push(0);
        for f in mesh.face_ids() {
            pts.clear();
            for v in mesh.face_vertices(f) {
                face_corners.push(v.0);
                pts.push(vertices[v.index()]);
            }
            face_edges.extend(mesh.face_edges(f).map(|e| e.0));
            face_offsets.push(face_corners.len() as u32);
            let n = newell_normal_slice(&pts);
            if smooth {
                for v in mesh.face_vertices(f) {
                    vertex_normals[v.index()] += n;
                }
            }
            face_normals.push(n.normalize_or_zero());
        }
        for n in &mut vertex_normals {
            *n = n.normalize_or_zero();
        }
        let tris = mesh.triangulate_with_faces();
        let mut positions = Vec::with_capacity(tris.len() * 3);
        let mut normals = Vec::with_capacity(tris.len() * 3);
        let mut tri_faces = Vec::with_capacity(tris.len());
        for (f, t) in tris {
            let p = t.map(|i| vertices[i as usize]);
            let tri_n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            let fallback = if face_normals[f.index()] != Vec3::ZERO {
                face_normals[f.index()]
            } else if tri_n != Vec3::ZERO {
                tri_n
            } else {
                Vec3::Z
            };
            for (k, &vi) in t.iter().enumerate() {
                positions.push(p[k]);
                let n = if smooth {
                    vertex_normals[vi as usize]
                } else {
                    fallback
                };
                normals.push(if n == Vec3::ZERO { fallback } else { n });
            }
            tri_faces.push(f.0);
        }
        let edges = mesh
            .edge_ids()
            .map(|e| mesh.edge_vertices(e).map(|v| v.0))
            .collect();
        MeshGeometry {
            positions,
            normals,
            tri_faces,
            vertices,
            edges,
            face_offsets,
            face_corners,
            face_edges,
        }
    }

    /// Copy transformed by `m` (normals by the inverse transpose).
    pub fn transformed(&self, m: &Affine3A) -> MeshGeometry {
        let nm = normal_matrix(m);
        let flip = m.matrix3.determinant() < 0.0;
        let mut g = self.clone();
        for p in g.positions.iter_mut().chain(g.vertices.iter_mut()) {
            *p = m.transform_point3(*p);
        }
        for n in &mut g.normals {
            *n = (nm * *n).normalize_or(*n);
        }
        if flip {
            // Mirroring transforms reverse the winding; keep triangles CCW.
            for t in 0..g.tri_faces.len() {
                g.positions.swap(3 * t + 1, 3 * t + 2);
                g.normals.swap(3 * t + 1, 3 * t + 2);
            }
        }
        g
    }

    pub fn triangle_count(&self) -> usize {
        self.tri_faces.len()
    }
    pub fn face_count(&self) -> usize {
        self.face_offsets.len().saturating_sub(1)
    }
    /// Vertex ids of face `f`.
    pub fn face_vertices(&self, f: u32) -> &[u32] {
        let (a, b) = self.face_range(f);
        &self.face_corners[a..b]
    }
    /// Edge ids of face `f`.
    pub fn face_edge_ids(&self, f: u32) -> &[u32] {
        let (a, b) = self.face_range(f);
        &self.face_edges[a..b]
    }
    fn face_range(&self, f: u32) -> (usize, usize) {
        let f = f as usize;
        if f + 1 >= self.face_offsets.len() {
            return (0, 0);
        }
        (
            self.face_offsets[f] as usize,
            self.face_offsets[f + 1] as usize,
        )
    }
    /// Bounding box of the vertices.
    pub fn bounds(&self) -> Option<Aabb> {
        Aabb::from_points(self.vertices.iter().copied())
    }
}

/// Inverse-transpose of the linear part (for transforming normals).
pub fn normal_matrix(m: &Affine3A) -> Mat3 {
    let l = Mat3::from(m.matrix3);
    if l.determinant().abs() < 1e-20 {
        l
    } else {
        l.inverse().transpose()
    }
}

/// Selected elements to highlight on an object in an edit mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditHighlight {
    pub mode: SelectMode,
    /// Selected vertices (vertex mode).
    pub vertices: BTreeSet<u32>,
    /// Highlighted edges.
    pub edges: BTreeSet<u32>,
    /// Highlighted faces.
    pub faces: BTreeSet<u32>,
}

impl EditHighlight {
    /// Highlighted elements of `mesh` for the selection of object `id`.
    pub fn from_selection(selection: &Selection, id: Uuid, mesh: &Mesh) -> EditHighlight {
        let mode = selection.mode;
        let mut h = EditHighlight {
            mode,
            ..EditHighlight::default()
        };
        let Some(sel) = selection.elements(id) else {
            return h;
        };
        let mut sel = sel.clone();
        sel.prune(mesh);
        match mode {
            SelectMode::Object => {}
            SelectMode::Vertex => {
                h.vertices = sel
                    .resolved_vertices(mesh)
                    .into_iter()
                    .map(|v| v.0)
                    .collect();
                h.edges = mesh
                    .edge_ids()
                    .filter(|&e| {
                        mesh.edge_vertices(e)
                            .iter()
                            .all(|v| h.vertices.contains(&v.0))
                    })
                    .map(|e| e.0)
                    .collect();
                h.faces = mesh
                    .face_ids()
                    .filter(|&f| mesh.face_vertices(f).all(|v| h.vertices.contains(&v.0)))
                    .map(|f| f.0)
                    .collect();
            }
            SelectMode::Edge => {
                h.edges = sel.resolved_edges(mesh).into_iter().map(|e| e.0).collect();
                h.faces = mesh
                    .face_ids()
                    .filter(|&f| mesh.face_edges(f).all(|e| h.edges.contains(&e.0)))
                    .map(|f| f.0)
                    .collect();
            }
            SelectMode::Face => {
                h.faces = sel.resolved_faces(mesh).into_iter().map(|f| f.0).collect();
                for &f in &h.faces {
                    h.edges
                        .extend(mesh.face_edges(FaceId(f)).map(|e: EdgeId| e.0));
                }
            }
        }
        h
    }
}

/// Selection state of one drawn object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjectHighlight {
    /// Selected in object mode (outlined).
    pub selected: bool,
    /// The active object (brighter outline).
    pub active: bool,
    /// Edit-mode highlight (wireframe, selected elements) when the selection
    /// is in a vertex/edge/face mode and this object is being edited.
    pub edit: Option<EditHighlight>,
}

/// One mesh object ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawObject {
    pub node_id: Uuid,
    pub name: String,
    /// 1-based id used in ID buffers (0 = background).
    pub pick_id: u32,
    pub world: Affine3A,
    pub material: Material,
    /// Geometry in world space.
    pub geometry: MeshGeometry,
    pub highlight: ObjectHighlight,
}

/// Extracted scene.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneDrawData {
    pub objects: Vec<DrawObject>,
    /// World bounds of all objects.
    pub bounds: Option<Aabb>,
}

/// Whether object `id` is being edited (edit mode) given a selection.
pub fn is_edited(selection: &Selection, id: Uuid) -> bool {
    selection.mode != SelectMode::Object
        && (selection.is_selected(id) || selection.elements(id).is_some_and(|e| !e.is_empty()))
}

/// Selection highlight of object `id`.
pub fn object_highlight(selection: Option<&Selection>, id: Uuid, mesh: &Mesh) -> ObjectHighlight {
    let Some(sel) = selection else {
        return ObjectHighlight::default();
    };
    let object_mode = sel.mode == SelectMode::Object;
    ObjectHighlight {
        selected: object_mode && sel.is_selected(id),
        active: object_mode && sel.active() == Some(id),
        edit: is_edited(sel, id).then(|| EditHighlight::from_selection(sel, id, mesh)),
    }
}

impl SceneDrawData {
    /// Extract all visible mesh nodes in traversal order.
    pub fn extract(scene: &Scene, selection: Option<&Selection>, smooth: bool) -> SceneDrawData {
        let mut objects = Vec::new();
        let mut bounds: Option<Aabb> = None;
        for id in scene.traverse() {
            let Some(node) = scene.get(id) else { continue };
            if !node.visible {
                continue;
            }
            let Some(mesh) = node.mesh.as_ref() else {
                continue;
            };
            if mesh.face_count() == 0 && mesh.edge_count() == 0 {
                continue;
            }
            let Ok(world) = scene.world_transform(id) else {
                continue;
            };
            let geometry = MeshGeometry::from_mesh(mesh, smooth).transformed(&world);
            if let Some(b) = geometry.bounds() {
                bounds = Some(bounds.map_or(b, |o| o.union(&b)));
            }
            objects.push(DrawObject {
                node_id: id,
                name: node.name.clone(),
                pick_id: objects.len() as u32 + 1,
                world,
                material: node.material.clone().unwrap_or_default(),
                geometry,
                highlight: object_highlight(selection, id, mesh),
            });
        }
        SceneDrawData { objects, bounds }
    }

    pub fn triangle_count(&self) -> usize {
        self.objects
            .iter()
            .map(|o| o.geometry.triangle_count())
            .sum()
    }

    /// Object by its 1-based pick id.
    pub fn by_pick_id(&self, pick_id: u32) -> Option<&DrawObject> {
        pick_id
            .checked_sub(1)
            .and_then(|i| self.objects.get(i as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge3d_core::primitives::cube;

    #[test]
    fn cube_geometry_uses_real_edges() {
        let m = cube(2.0).unwrap();
        let g = MeshGeometry::from_mesh(&m, false);
        assert_eq!(g.triangle_count(), 12);
        assert_eq!(g.edges.len(), 12, "12 cube edges, no diagonals");
        assert_eq!(g.face_count(), 6);
        assert_eq!(g.face_vertices(0).len(), 4);
        // Flat normals are axis aligned.
        for n in &g.normals {
            assert!((n.abs().max_element() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn mirrored_transform_keeps_outward_winding() {
        let m = cube(2.0).unwrap();
        let g = MeshGeometry::from_mesh(&m, false)
            .transformed(&Affine3A::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        for t in 0..g.triangle_count() {
            let p = &g.positions[3 * t..3 * t + 3];
            let geo = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(geo.dot(g.normals[3 * t]) > 0.0);
            let c = (p[0] + p[1] + p[2]) / 3.0;
            assert!(c.dot(g.normals[3 * t]) > 0.0, "outward");
        }
    }
}

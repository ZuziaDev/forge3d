//! Selection model and selection queries.

use std::collections::{BTreeMap, BTreeSet};

use glam::{Affine3A, Mat3, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::math::Aabb;
use crate::mesh::{EdgeId, ElementKind, FaceId, Mesh, VertexId};
use crate::scene::Scene;

/// Interaction / selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    #[default]
    Object,
    Vertex,
    Edge,
    Face,
}

impl SelectMode {
    /// Element kind for element modes, `None` in object mode.
    pub fn element_kind(self) -> Option<ElementKind> {
        match self {
            SelectMode::Object => None,
            SelectMode::Vertex => Some(ElementKind::Vertex),
            SelectMode::Edge => Some(ElementKind::Edge),
            SelectMode::Face => Some(ElementKind::Face),
        }
    }
}

/// How a selection query combines with the existing selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectAction {
    /// Replace the current selection.
    #[default]
    Set,
    /// Add to the current selection.
    Add,
    /// Remove from the current selection.
    Remove,
    /// Keep only elements that are both selected and matched.
    Intersect,
}

/// Selected mesh elements of one object (indices into its mesh).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ElementSelection {
    #[serde(default)]
    pub vertices: BTreeSet<u32>,
    #[serde(default)]
    pub edges: BTreeSet<u32>,
    #[serde(default)]
    pub faces: BTreeSet<u32>,
}

impl ElementSelection {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.edges.is_empty() && self.faces.is_empty()
    }
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.edges.clear();
        self.faces.clear();
    }
    pub fn set(&self, kind: ElementKind) -> &BTreeSet<u32> {
        match kind {
            ElementKind::Vertex => &self.vertices,
            ElementKind::Edge => &self.edges,
            ElementKind::Face => &self.faces,
        }
    }
    pub fn set_mut(&mut self, kind: ElementKind) -> &mut BTreeSet<u32> {
        match kind {
            ElementKind::Vertex => &mut self.vertices,
            ElementKind::Edge => &mut self.edges,
            ElementKind::Face => &mut self.faces,
        }
    }

    /// Combine `indices` into the set of `kind` according to `action`.
    pub fn apply(&mut self, kind: ElementKind, indices: impl IntoIterator<Item = u32>, action: SelectAction) {
        let set = self.set_mut(kind);
        match action {
            SelectAction::Set => {
                set.clear();
                set.extend(indices);
            }
            SelectAction::Add => set.extend(indices),
            SelectAction::Remove => {
                for i in indices {
                    set.remove(&i);
                }
            }
            SelectAction::Intersect => {
                let keep: BTreeSet<u32> = indices.into_iter().collect();
                set.retain(|i| keep.contains(i));
            }
        }
    }

    /// Drop indices that do not exist in `mesh`.
    pub fn prune(&mut self, mesh: &Mesh) {
        let (nv, ne, nf) = (mesh.vertex_count() as u32, mesh.edge_count() as u32, mesh.face_count() as u32);
        self.vertices.retain(|&v| v < nv);
        self.edges.retain(|&e| e < ne);
        self.faces.retain(|&f| f < nf);
    }

    /// Faces to operate on: explicitly selected faces, else faces whose edges
    /// are all selected, else faces whose vertices are all selected.
    pub fn resolved_faces(&self, mesh: &Mesh) -> Vec<FaceId> {
        if !self.faces.is_empty() {
            return self.faces.iter().map(|&f| FaceId(f)).collect();
        }
        if !self.edges.is_empty() {
            return mesh
                .face_ids()
                .filter(|&f| mesh.face_edges(f).all(|e| self.edges.contains(&e.0)))
                .collect();
        }
        if !self.vertices.is_empty() {
            return mesh
                .face_ids()
                .filter(|&f| mesh.face_vertices(f).all(|v| self.vertices.contains(&v.0)))
                .collect();
        }
        Vec::new()
    }

    /// Edges to operate on: selected edges, else edges of selected faces,
    /// else edges with both vertices selected.
    pub fn resolved_edges(&self, mesh: &Mesh) -> Vec<EdgeId> {
        if !self.edges.is_empty() {
            return self.edges.iter().map(|&e| EdgeId(e)).collect();
        }
        if !self.faces.is_empty() {
            let mut s: BTreeSet<u32> = BTreeSet::new();
            for &f in &self.faces {
                s.extend(mesh.face_edges(FaceId(f)).map(|e| e.0));
            }
            return s.into_iter().map(EdgeId).collect();
        }
        if !self.vertices.is_empty() {
            return mesh
                .edge_ids()
                .filter(|&e| mesh.edge_vertices(e).iter().all(|v| self.vertices.contains(&v.0)))
                .collect();
        }
        Vec::new()
    }

    /// Vertices to operate on: selected vertices, else vertices of selected
    /// edges, else vertices of selected faces.
    pub fn resolved_vertices(&self, mesh: &Mesh) -> Vec<VertexId> {
        if !self.vertices.is_empty() {
            return self.vertices.iter().map(|&v| VertexId(v)).collect();
        }
        let mut s: BTreeSet<u32> = BTreeSet::new();
        if !self.edges.is_empty() {
            for &e in &self.edges {
                s.extend(mesh.edge_vertices(EdgeId(e)).iter().map(|v| v.0));
            }
        } else {
            for &f in &self.faces {
                s.extend(mesh.face_vertices(FaceId(f)).map(|v| v.0));
            }
        }
        s.into_iter().map(VertexId).collect()
    }

    /// All vertices touched by any selected element (union of all kinds).
    pub fn touched_vertices(&self, mesh: &Mesh) -> Vec<VertexId> {
        let mut s: BTreeSet<u32> = self.vertices.clone();
        for &e in &self.edges {
            s.extend(mesh.edge_vertices(EdgeId(e)).iter().map(|v| v.0));
        }
        for &f in &self.faces {
            s.extend(mesh.face_vertices(FaceId(f)).map(|v| v.0));
        }
        s.into_iter().map(VertexId).collect()
    }
}

/// Document selection: mode, selected objects and per-object element selection.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Selection {
    pub mode: SelectMode,
    objects: Vec<Uuid>,
    active: Option<Uuid>,
    elements: BTreeMap<Uuid, ElementSelection>,
}

impl Selection {
    pub fn new() -> Selection {
        Selection::default()
    }
    /// Selected objects in selection order.
    pub fn objects(&self) -> &[Uuid] {
        &self.objects
    }
    /// The active object (last selected by default).
    pub fn active(&self) -> Option<Uuid> {
        self.active
    }
    pub fn is_selected(&self, id: Uuid) -> bool {
        self.objects.contains(&id)
    }
    /// Replace the object selection; the last id becomes active.
    pub fn set_objects(&mut self, ids: impl IntoIterator<Item = Uuid>) {
        self.objects.clear();
        for id in ids {
            if !self.objects.contains(&id) {
                self.objects.push(id);
            }
        }
        self.active = self.objects.last().copied();
    }
    /// Combine object ids into the selection.
    pub fn apply_objects(&mut self, ids: &[Uuid], action: SelectAction) {
        match action {
            SelectAction::Set => self.set_objects(ids.iter().copied()),
            SelectAction::Add => {
                for &id in ids {
                    if !self.objects.contains(&id) {
                        self.objects.push(id);
                    }
                }
                if let Some(&l) = ids.last() {
                    self.active = Some(l);
                }
            }
            SelectAction::Remove => {
                self.objects.retain(|o| !ids.contains(o));
                if self.active.is_some_and(|a| ids.contains(&a)) {
                    self.active = self.objects.last().copied();
                }
            }
            SelectAction::Intersect => {
                self.objects.retain(|o| ids.contains(o));
                if self.active.is_some_and(|a| !self.objects.contains(&a)) {
                    self.active = self.objects.last().copied();
                }
            }
        }
    }
    /// Set the active object (adds it to the selection).
    pub fn set_active(&mut self, id: Option<Uuid>) {
        if let Some(i) = id {
            if !self.objects.contains(&i) {
                self.objects.push(i);
            }
        }
        self.active = id;
    }
    pub fn clear_objects(&mut self) {
        self.objects.clear();
        self.active = None;
    }
    /// Element selection of an object, if any.
    pub fn elements(&self, id: Uuid) -> Option<&ElementSelection> {
        self.elements.get(&id)
    }
    /// Mutable element selection of an object (created on demand).
    pub fn elements_mut(&mut self, id: Uuid) -> &mut ElementSelection {
        self.elements.entry(id).or_default()
    }
    /// Replace the element selection of an object.
    pub fn set_elements(&mut self, id: Uuid, sel: ElementSelection) {
        if sel.is_empty() {
            self.elements.remove(&id);
        } else {
            self.elements.insert(id, sel);
        }
    }
    pub fn clear_elements(&mut self, id: Uuid) {
        self.elements.remove(&id);
    }
    /// Objects with a non-empty element selection.
    pub fn objects_with_elements(&self) -> impl Iterator<Item = (&Uuid, &ElementSelection)> {
        self.elements.iter().filter(|(_, s)| !s.is_empty())
    }
    /// Remove references to nodes that no longer exist and element indices
    /// outside their meshes.
    pub fn prune(&mut self, scene: &Scene) {
        self.objects.retain(|id| scene.contains(*id));
        if self.active.is_some_and(|a| !scene.contains(a)) {
            self.active = self.objects.last().copied();
        }
        self.elements.retain(|id, sel| match scene.get(*id).and_then(|n| n.mesh.as_ref()) {
            Some(m) => {
                sel.prune(m);
                !sel.is_empty()
            }
            None => false,
        });
    }
}

// ---------------------------------------------------------------------------
// Queries (pure functions on meshes)
// ---------------------------------------------------------------------------

/// All indices of a kind.
pub fn all_elements(mesh: &Mesh, kind: ElementKind) -> Vec<u32> {
    (0..mesh.element_count(kind) as u32).collect()
}

/// Indices of a kind that are *not* in `current`.
pub fn inverted_elements(mesh: &Mesh, kind: ElementKind, current: &BTreeSet<u32>) -> Vec<u32> {
    (0..mesh.element_count(kind) as u32)
        .filter(|i| !current.contains(i))
        .collect()
}

/// Validate an explicit index list.
pub fn check_indices(mesh: &Mesh, kind: ElementKind, indices: &[u32]) -> CoreResult<()> {
    for &i in indices {
        mesh.check_index(kind, i)?;
    }
    Ok(())
}

/// Per-element unit normals (vertex normals, averaged face normals for
/// edges, face normals), optionally transformed by a normal matrix.
pub fn element_normals(mesh: &Mesh, kind: ElementKind, normal_matrix: Option<Mat3>) -> Vec<Vec3> {
    let raw: Vec<Vec3> = match kind {
        ElementKind::Vertex => mesh.vertex_normals(),
        ElementKind::Edge => mesh.edge_ids().map(|e| mesh.edge_normal(e)).collect(),
        ElementKind::Face => mesh.face_ids().map(|f| mesh.face_normal(f)).collect(),
    };
    match normal_matrix {
        Some(m) => raw.into_iter().map(|n| (m * n).normalize_or_zero()).collect(),
        None => raw,
    }
}

/// Normal matrix (inverse transpose of the linear part) of a transform.
pub fn normal_matrix(m: &Affine3A) -> Mat3 {
    Mat3::from(m.matrix3).inverse().transpose()
}

/// Elements whose normal is within `max_angle_deg` of `direction`.
/// `normal_matrix` maps local normals into the space of `direction`.
pub fn elements_facing(
    mesh: &Mesh,
    kind: ElementKind,
    direction: Vec3,
    max_angle_deg: f32,
    normal_matrix: Option<Mat3>,
) -> CoreResult<Vec<u32>> {
    let dir = direction.normalize_or_zero();
    if dir == Vec3::ZERO || !dir.is_finite() {
        return Err(CoreError::param("direction", "must be a non-zero finite vector"));
    }
    if !(max_angle_deg.is_finite() && (0.0..=180.0).contains(&max_angle_deg)) {
        return Err(CoreError::param("max_angle_deg", "must be in 0..=180"));
    }
    let cos = max_angle_deg.to_radians().cos() - 1e-6;
    Ok(element_normals(mesh, kind, normal_matrix)
        .into_iter()
        .enumerate()
        .filter(|(_, n)| *n != Vec3::ZERO && n.dot(dir) >= cos)
        .map(|(i, _)| i as u32)
        .collect())
}

/// Elements whose normal is within `max_angle_deg` of the normal of the
/// `reference` element (select similar: normal).
pub fn elements_similar_normal(mesh: &Mesh, kind: ElementKind, reference: u32, max_angle_deg: f32) -> CoreResult<Vec<u32>> {
    mesh.check_index(kind, reference)?;
    let normals = element_normals(mesh, kind, None);
    let r = normals[reference as usize];
    if r == Vec3::ZERO {
        return Err(CoreError::InvalidSelection(format!(
            "reference {kind:?} {reference} has no defined normal"
        )));
    }
    elements_facing(mesh, kind, r, max_angle_deg, None)
}

/// Elements entirely inside `aabb` (vertices: the vertex; edges: both
/// endpoints; faces: all corners). Positions are first mapped by `to_space`
/// (e.g. the object's world transform for a world-space box).
pub fn elements_in_box(mesh: &Mesh, kind: ElementKind, aabb: &Aabb, to_space: &Affine3A) -> Vec<u32> {
    let inside: Vec<bool> = mesh
        .vertex_ids()
        .map(|v| aabb.contains(to_space.transform_point3(mesh.position(v))))
        .collect();
    match kind {
        ElementKind::Vertex => (0..mesh.vertex_count() as u32)
            .filter(|&v| inside[v as usize])
            .collect(),
        ElementKind::Edge => mesh
            .edge_ids()
            .filter(|&e| mesh.edge_vertices(e).iter().all(|v| inside[v.index()]))
            .map(|e| e.0)
            .collect(),
        ElementKind::Face => mesh
            .face_ids()
            .filter(|&f| mesh.face_vertices(f).all(|v| inside[v.index()]))
            .map(|f| f.0)
            .collect(),
    }
}

/// Edge loop or ring through an edge.
pub fn edge_loop_or_ring(mesh: &Mesh, edge: u32, ring: bool) -> CoreResult<Vec<u32>> {
    mesh.check_index(ElementKind::Edge, edge)?;
    let e = EdgeId(edge);
    let list = if ring { mesh.edge_ring(e) } else { mesh.edge_loop(e) };
    Ok(list.into_iter().map(|e| e.0).collect())
}

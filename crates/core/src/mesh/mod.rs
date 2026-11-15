//! Half-edge polygon mesh.
//!
//! # Design
//!
//! The mesh stores four element arrays addressed by typed index handles:
//!
//! * [`VertexId`] – position + normal,
//! * [`EdgeId`] – an undirected edge between two vertices,
//! * [`LoopId`] – a *half-edge* ("loop", Blender terminology): one face corner,
//!   directed from its `vertex` to the vertex of its `next` loop,
//! * [`FaceId`] – an n-gon (≥ 3 corners) referencing its first loop.
//!
//! Every loop knows `next`/`prev` (around its face), its `face`, its `edge`
//! and a *radial* cycle (`radial_next`/`radial_prev`) of all loops that use the
//! same edge. For a manifold interior edge the radial cycle has exactly two
//! loops, so [`Mesh::loop_twin`] is the classic half-edge `twin`; on a boundary
//! the cycle has one loop (no twin); non-manifold edges (3+ faces) are still
//! representable, which keeps the structure robust for boolean output and
//! validation of imported data. Vertex→edge adjacency is stored in a compact
//! CSR array.
//!
//! Topology is immutable between rebuilds: editing operations produce a new
//! polygon list and rebuild the half-edge structure in O(n) (hash based edge
//! lookup), which also *compacts* all element arrays. Positions can be edited
//! in place. Loose vertices (without faces) are supported; loose edges are not
//! (edges exist only as face sides).
//!
//! Faces' loops are stored contiguously (`first .. first + len`).

mod edit;
mod query;
pub(crate) mod soup;
mod triangulate;
mod validate;

use std::collections::HashMap;

use glam::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::math::newell_normal_slice;

pub use edit::Remap;
pub use triangulate::triangulate_polygon;
pub use validate::ValidationReport;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// The raw index as `usize`.
            #[inline]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl From<u32> for $name {
            fn from(v: u32) -> Self {
                $name(v)
            }
        }

        impl From<$name> for u32 {
            fn from(v: $name) -> u32 {
                v.0
            }
        }
    };
}

id_type!(
    /// Handle of a mesh vertex.
    VertexId
);
id_type!(
    /// Handle of an (undirected) mesh edge.
    EdgeId
);
id_type!(
    /// Handle of a mesh face.
    FaceId
);
id_type!(
    /// Handle of a loop / half-edge (face corner).
    LoopId
);

/// Kind of mesh element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Vertex,
    Edge,
    Face,
}

/// A mesh vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    pub(crate) position: Vec3,
    pub(crate) normal: Vec3,
}

/// An undirected edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub(crate) vertices: [VertexId; 2],
    /// One loop of the radial cycle.
    pub(crate) loop_: LoopId,
}

/// A half-edge / face corner.
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    pub(crate) vertex: VertexId,
    pub(crate) edge: EdgeId,
    pub(crate) face: FaceId,
    pub(crate) next: LoopId,
    pub(crate) prev: LoopId,
    pub(crate) radial_next: LoopId,
    pub(crate) radial_prev: LoopId,
}

/// A polygon face.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub(crate) first: LoopId,
    pub(crate) len: u32,
    pub(crate) normal: Vec3,
}

/// Plain indexed-polygon representation (used for serialization and as an
/// interchange format for io crates / JSON APIs).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct MeshData {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Polygons as lists of vertex indices, counter-clockwise when seen from outside.
    pub faces: Vec<Vec<u32>>,
}

/// Half-edge mesh. See the module documentation for the design.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(into = "MeshData", try_from = "MeshData")]
pub struct Mesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) loops: Vec<Loop>,
    pub(crate) faces: Vec<Face>,
    /// CSR offsets: edges of vertex `v` are `vert_edges[vert_edge_start[v]..vert_edge_start[v+1]]`.
    pub(crate) vert_edge_start: Vec<u32>,
    pub(crate) vert_edges: Vec<EdgeId>,
}

impl From<Mesh> for MeshData {
    fn from(m: Mesh) -> Self {
        m.to_mesh_data()
    }
}

impl TryFrom<MeshData> for Mesh {
    type Error = CoreError;
    fn try_from(d: MeshData) -> CoreResult<Mesh> {
        Mesh::from_mesh_data(&d)
    }
}

impl Mesh {
    /// Empty mesh.
    pub fn new() -> Self {
        Mesh {
            vert_edge_start: vec![0],
            ..Default::default()
        }
    }

    /// Build a half-edge mesh from positions and polygon index lists.
    ///
    /// Every polygon needs ≥ 3 distinct, in-range indices. Polygons should be
    /// wound counter-clockwise when viewed from the outside (normal side).
    pub fn from_polygons<F: AsRef<[u32]>>(positions: Vec<Vec3>, faces: &[F]) -> CoreResult<Mesh> {
        let nv = positions.len();
        if nv > u32::MAX as usize / 2 {
            return Err(CoreError::InvalidMesh("too many vertices".into()));
        }
        for (i, p) in positions.iter().enumerate() {
            if !p.is_finite() {
                return Err(CoreError::InvalidMesh(format!(
                    "vertex {i} has a non-finite position"
                )));
            }
        }
        let mut total_loops = 0usize;
        let mut scratch: Vec<u32> = Vec::new();
        for (fi, f) in faces.iter().enumerate() {
            let f = f.as_ref();
            if f.len() < 3 {
                return Err(CoreError::InvalidMesh(format!(
                    "face {fi} has {} vertices; at least 3 are required",
                    f.len()
                )));
            }
            for &v in f {
                if v as usize >= nv {
                    return Err(CoreError::InvalidMesh(format!(
                        "face {fi} references vertex {v} but there are only {nv} vertices"
                    )));
                }
            }
            scratch.clear();
            scratch.extend_from_slice(f);
            scratch.sort_unstable();
            if scratch.windows(2).any(|w| w[0] == w[1]) {
                return Err(CoreError::InvalidMesh(format!(
                    "face {fi} uses the same vertex more than once"
                )));
            }
            total_loops += f.len();
        }

        let mut mesh = Mesh {
            vertices: positions
                .into_iter()
                .map(|p| Vertex {
                    position: p,
                    normal: Vec3::ZERO,
                })
                .collect(),
            edges: Vec::with_capacity(total_loops / 2 + 1),
            loops: Vec::with_capacity(total_loops),
            faces: Vec::with_capacity(faces.len()),
            vert_edge_start: Vec::new(),
            vert_edges: Vec::new(),
        };

        let mut edge_map: HashMap<(u32, u32), u32> = HashMap::with_capacity(total_loops / 2 + 1);
        for (fi, f) in faces.iter().enumerate() {
            let f = f.as_ref();
            let first = mesh.loops.len() as u32;
            let n = f.len() as u32;
            let fid = FaceId(fi as u32);
            for (i, &v) in f.iter().enumerate() {
                let i = i as u32;
                let w = f[((i + 1) % n) as usize];
                let l = LoopId(first + i);
                let key = if v < w { (v, w) } else { (w, v) };
                let eid = match edge_map.get(&key) {
                    Some(&e) => EdgeId(e),
                    None => {
                        let e = mesh.edges.len() as u32;
                        mesh.edges.push(Edge {
                            vertices: [VertexId(v), VertexId(w)],
                            loop_: l,
                        });
                        edge_map.insert(key, e);
                        EdgeId(e)
                    }
                };
                mesh.loops.push(Loop {
                    vertex: VertexId(v),
                    edge: eid,
                    face: fid,
                    next: LoopId(first + (i + 1) % n),
                    prev: LoopId(first + (i + n - 1) % n),
                    radial_next: l,
                    radial_prev: l,
                });
                let head = mesh.edges[eid.index()].loop_;
                if head != l {
                    // Insert `l` into the radial cycle right after `head`.
                    let after = mesh.loops[head.index()].radial_next;
                    mesh.loops[l.index()].radial_next = after;
                    mesh.loops[l.index()].radial_prev = head;
                    mesh.loops[head.index()].radial_next = l;
                    mesh.loops[after.index()].radial_prev = l;
                }
            }
            mesh.faces.push(Face {
                first: LoopId(first),
                len: n,
                normal: Vec3::ZERO,
            });
        }
        mesh.rebuild_vertex_edges();
        mesh.recalc_normals();
        Ok(mesh)
    }

    /// Like [`Mesh::from_polygons`] but first cleans the polygon soup:
    /// collapses consecutive duplicate indices, splits polygons that revisit a
    /// vertex, drops polygons with < 3 vertices or out-of-range indices and
    /// removes duplicate / mutually-opposite face pairs.
    pub fn from_polygons_cleaned(positions: Vec<Vec3>, faces: Vec<Vec<u32>>) -> CoreResult<Mesh> {
        let nv = positions.len() as u32;
        let faces = faces
            .into_iter()
            .filter(|f| f.iter().all(|&v| v < nv))
            .collect();
        let mut s = soup::Soup { positions, faces };
        s.clean_faces();
        s.into_mesh()
    }

    /// Build from the serializable [`MeshData`].
    pub fn from_mesh_data(d: &MeshData) -> CoreResult<Mesh> {
        Mesh::from_polygons(
            d.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
            &d.faces,
        )
    }

    /// Convert to the serializable [`MeshData`].
    pub fn to_mesh_data(&self) -> MeshData {
        let (p, f) = self.to_polygons();
        MeshData {
            positions: p.into_iter().map(|v| v.to_array()).collect(),
            faces: f,
        }
    }

    /// Convert back to positions + indexed polygons.
    pub fn to_polygons(&self) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        (self.positions(), self.face_index_lists())
    }

    /// Polygon index lists (one per face, in face order).
    pub fn face_index_lists(&self) -> Vec<Vec<u32>> {
        self.faces
            .iter()
            .map(|f| {
                (f.first.0..f.first.0 + f.len)
                    .map(|l| self.loops[l as usize].vertex.0)
                    .collect()
            })
            .collect()
    }

    pub(crate) fn rebuild_vertex_edges(&mut self) {
        let nv = self.vertices.len();
        let mut start = vec![0u32; nv + 1];
        for e in &self.edges {
            start[e.vertices[0].index() + 1] += 1;
            start[e.vertices[1].index() + 1] += 1;
        }
        for i in 0..nv {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut list = vec![EdgeId(0); start[nv] as usize];
        for (ei, e) in self.edges.iter().enumerate() {
            for v in e.vertices {
                list[fill[v.index()] as usize] = EdgeId(ei as u32);
                fill[v.index()] += 1;
            }
        }
        self.vert_edge_start = start;
        self.vert_edges = list;
    }

    // ----- counts -----

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }
    pub fn loop_count(&self) -> usize {
        self.loops.len()
    }
    /// Count of elements of a kind.
    pub fn element_count(&self, kind: ElementKind) -> usize {
        match kind {
            ElementKind::Vertex => self.vertex_count(),
            ElementKind::Edge => self.edge_count(),
            ElementKind::Face => self.face_count(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
    /// V − E + F.
    pub fn euler_characteristic(&self) -> i64 {
        self.vertex_count() as i64 - self.edge_count() as i64 + self.face_count() as i64
    }

    /// Check an element index, returning a structured error when out of range.
    pub fn check_index(&self, kind: ElementKind, index: u32) -> CoreResult<()> {
        let count = self.element_count(kind);
        if (index as usize) < count {
            Ok(())
        } else {
            Err(CoreError::ElementOutOfRange {
                kind,
                index,
                count: count as u32,
            })
        }
    }

    // ----- element iterators -----

    pub fn vertex_ids(&self) -> impl Iterator<Item = VertexId> + '_ {
        (0..self.vertices.len() as u32).map(VertexId)
    }
    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        (0..self.edges.len() as u32).map(EdgeId)
    }
    pub fn face_ids(&self) -> impl Iterator<Item = FaceId> + '_ {
        (0..self.faces.len() as u32).map(FaceId)
    }

    // ----- vertex data -----

    pub fn position(&self, v: VertexId) -> Vec3 {
        self.vertices[v.index()].position
    }
    /// Set a vertex position. Call [`Mesh::recalc_normals`] afterwards.
    pub fn set_position(&mut self, v: VertexId, p: Vec3) {
        self.vertices[v.index()].position = p;
    }
    /// All positions in vertex order.
    pub fn positions(&self) -> Vec<Vec3> {
        self.vertices.iter().map(|v| v.position).collect()
    }
    /// Stored (area-weighted) vertex normal.
    pub fn vertex_normal(&self, v: VertexId) -> Vec3 {
        self.vertices[v.index()].normal
    }
    /// All vertex normals in vertex order.
    pub fn vertex_normals(&self) -> Vec<Vec3> {
        self.vertices.iter().map(|v| v.normal).collect()
    }

    // ----- face data -----

    /// Stored unit face normal (zero for degenerate faces).
    pub fn face_normal(&self, f: FaceId) -> Vec3 {
        self.faces[f.index()].normal
    }
    pub fn face_len(&self, f: FaceId) -> usize {
        self.faces[f.index()].len as usize
    }
    /// Loops of a face in winding order.
    pub fn face_loops(&self, f: FaceId) -> impl Iterator<Item = LoopId> + '_ {
        let face = &self.faces[f.index()];
        (face.first.0..face.first.0 + face.len).map(LoopId)
    }
    /// Vertices of a face in winding order.
    pub fn face_vertices(&self, f: FaceId) -> impl Iterator<Item = VertexId> + '_ {
        self.face_loops(f).map(move |l| self.loops[l.index()].vertex)
    }
    /// Edges of a face in winding order (edge i goes from corner i to i+1).
    pub fn face_edges(&self, f: FaceId) -> impl Iterator<Item = EdgeId> + '_ {
        self.face_loops(f).map(move |l| self.loops[l.index()].edge)
    }
    /// Face corner positions.
    pub fn face_positions(&self, f: FaceId) -> Vec<Vec3> {
        self.face_vertices(f).map(|v| self.position(v)).collect()
    }
    /// Average of face corner positions.
    pub fn face_centroid(&self, f: FaceId) -> Vec3 {
        let pts = self.face_positions(f);
        pts.iter().copied().sum::<Vec3>() / pts.len() as f32
    }
    /// Polygon area (exact for planar polygons).
    pub fn face_area(&self, f: FaceId) -> f32 {
        newell_normal_slice(&self.face_positions(f)).length() * 0.5
    }

    // ----- edge / loop data -----

    pub fn edge_vertices(&self, e: EdgeId) -> [VertexId; 2] {
        self.edges[e.index()].vertices
    }
    pub fn edge_length(&self, e: EdgeId) -> f32 {
        let [a, b] = self.edge_vertices(e);
        self.position(a).distance(self.position(b))
    }
    pub fn edge_midpoint(&self, e: EdgeId) -> Vec3 {
        let [a, b] = self.edge_vertices(e);
        (self.position(a) + self.position(b)) * 0.5
    }
    /// Normalized average of the adjacent face normals.
    pub fn edge_normal(&self, e: EdgeId) -> Vec3 {
        self.edge_loops(e)
            .iter()
            .map(|&l| self.face_normal(self.loop_face(l)))
            .sum::<Vec3>()
            .normalize_or_zero()
    }
    /// The vertex a loop starts at.
    pub fn loop_vertex(&self, l: LoopId) -> VertexId {
        self.loops[l.index()].vertex
    }
    /// The vertex a loop points to (start of the next loop).
    pub fn loop_target(&self, l: LoopId) -> VertexId {
        self.loops[self.loops[l.index()].next.index()].vertex
    }
    pub fn loop_edge(&self, l: LoopId) -> EdgeId {
        self.loops[l.index()].edge
    }
    pub fn loop_face(&self, l: LoopId) -> FaceId {
        self.loops[l.index()].face
    }
    pub fn loop_next(&self, l: LoopId) -> LoopId {
        self.loops[l.index()].next
    }
    pub fn loop_prev(&self, l: LoopId) -> LoopId {
        self.loops[l.index()].prev
    }
    pub fn loop_radial_next(&self, l: LoopId) -> LoopId {
        self.loops[l.index()].radial_next
    }
    /// The opposite half-edge across a manifold edge; `None` on boundary or
    /// non-manifold (3+ faces) edges.
    pub fn loop_twin(&self, l: LoopId) -> Option<LoopId> {
        let r = self.loops[l.index()].radial_next;
        if r != l && self.loops[r.index()].radial_next == l {
            Some(r)
        } else {
            None
        }
    }
    /// All loops (half-edges) using an edge.
    pub fn edge_loops(&self, e: EdgeId) -> Vec<LoopId> {
        let first = self.edges[e.index()].loop_;
        let mut out = vec![first];
        let mut l = self.loops[first.index()].radial_next;
        while l != first {
            out.push(l);
            l = self.loops[l.index()].radial_next;
            if out.len() > self.loops.len() {
                break;
            }
        }
        out
    }
    /// Number of faces using an edge.
    pub fn edge_face_count(&self, e: EdgeId) -> usize {
        self.edge_loops(e).len()
    }

    // ----- normals -----

    /// Recompute stored face normals (Newell) and area-weighted vertex normals.
    pub fn recalc_normals(&mut self) {
        let mut vn = vec![Vec3::ZERO; self.vertices.len()];
        let mut pts: Vec<Vec3> = Vec::new();
        for fi in 0..self.faces.len() {
            let f = &self.faces[fi];
            pts.clear();
            for l in f.first.0..f.first.0 + f.len {
                pts.push(self.vertices[self.loops[l as usize].vertex.index()].position);
            }
            let n = newell_normal_slice(&pts);
            for l in f.first.0..f.first.0 + f.len {
                vn[self.loops[l as usize].vertex.index()] += n;
            }
            self.faces[fi].normal = n.normalize_or_zero();
        }
        for (v, n) in self.vertices.iter_mut().zip(vn) {
            v.normal = n.normalize_or_zero();
        }
    }
}

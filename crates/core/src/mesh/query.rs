//! Adjacency and geometric queries.

use std::collections::HashSet;

use glam::Vec3;

use super::{EdgeId, FaceId, LoopId, Mesh, VertexId};
use crate::math::Aabb;

impl Mesh {
    /// Edges incident to a vertex.
    pub fn vertex_edges(&self, v: VertexId) -> &[EdgeId] {
        let s = self.vert_edge_start[v.index()] as usize;
        let e = self.vert_edge_start[v.index() + 1] as usize;
        &self.vert_edges[s..e]
    }

    /// Number of incident edges.
    pub fn vertex_valence(&self, v: VertexId) -> usize {
        self.vertex_edges(v).len()
    }

    /// Loops (face corners) that start at `v`.
    pub fn vertex_loops(&self, v: VertexId) -> Vec<LoopId> {
        let mut out = Vec::new();
        for &e in self.vertex_edges(v) {
            for l in self.edge_loops(e) {
                if self.loop_vertex(l) == v {
                    out.push(l);
                }
            }
        }
        out
    }

    /// Faces around a vertex (each once).
    pub fn vertex_faces(&self, v: VertexId) -> Vec<FaceId> {
        let mut out: Vec<FaceId> = self
            .vertex_loops(v)
            .into_iter()
            .map(|l| self.loop_face(l))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Vertices connected to `v` by an edge.
    pub fn vertex_neighbors(&self, v: VertexId) -> Vec<VertexId> {
        self.vertex_edges(v)
            .iter()
            .map(|&e| self.edge_other_vertex(e, v))
            .collect()
    }

    /// The other endpoint of an edge.
    pub fn edge_other_vertex(&self, e: EdgeId, v: VertexId) -> VertexId {
        let [a, b] = self.edge_vertices(e);
        if a == v {
            b
        } else {
            a
        }
    }

    /// Faces using an edge.
    pub fn edge_faces(&self, e: EdgeId) -> Vec<FaceId> {
        self.edge_loops(e)
            .into_iter()
            .map(|l| self.loop_face(l))
            .collect()
    }

    /// Edge between two vertices, if any.
    pub fn find_edge(&self, a: VertexId, b: VertexId) -> Option<EdgeId> {
        let (small, other) = if self.vertex_valence(a) <= self.vertex_valence(b) {
            (a, b)
        } else {
            (b, a)
        };
        self.vertex_edges(small)
            .iter()
            .copied()
            .find(|&e| self.edge_other_vertex(e, small) == other)
    }

    /// Faces sharing an edge with `f`.
    pub fn face_neighbors(&self, f: FaceId) -> Vec<FaceId> {
        let mut out = Vec::new();
        for e in self.face_edges(f) {
            for g in self.edge_faces(e) {
                if g != f {
                    out.push(g);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether the edge has exactly one face.
    pub fn is_boundary_edge(&self, e: EdgeId) -> bool {
        self.edge_face_count(e) == 1
    }

    /// Whether the edge has more than two faces.
    pub fn is_non_manifold_edge(&self, e: EdgeId) -> bool {
        self.edge_face_count(e) > 2
    }

    /// Whether the vertex lies on a boundary edge (or has no faces at all).
    pub fn is_boundary_vertex(&self, v: VertexId) -> bool {
        let edges = self.vertex_edges(v);
        edges.is_empty() || edges.iter().any(|&e| self.is_boundary_edge(e))
    }

    /// All boundary edges.
    pub fn boundary_edges(&self) -> Vec<EdgeId> {
        self.edge_ids()
            .filter(|&e| self.is_boundary_edge(e))
            .collect()
    }

    /// True when every edge has exactly two faces (no holes, no non-manifold edges).
    pub fn is_closed(&self) -> bool {
        !self.faces.is_empty() && self.edge_ids().all(|e| self.edge_face_count(e) == 2)
    }

    /// Boundary loops as vertex chains following the boundary half-edges'
    /// direction. Each loop is returned once; open chains (at non-manifold
    /// boundary vertices) are returned as far as they can be followed.
    pub fn boundary_loops(&self) -> Vec<Vec<VertexId>> {
        let mut visited = vec![false; self.edges.len()];
        let mut loops = Vec::new();
        for e in self.edge_ids() {
            if visited[e.index()] || !self.is_boundary_edge(e) {
                continue;
            }
            let mut chain = Vec::new();
            let mut l = self.edges[e.index()].loop_;
            loop {
                let edge = self.loop_edge(l);
                if visited[edge.index()] {
                    break;
                }
                visited[edge.index()] = true;
                chain.push(self.loop_vertex(l));
                let target = self.loop_target(l);
                // next boundary half-edge leaving `target`
                let next = self.vertex_edges(target).iter().copied().find(|&ne| {
                    !visited[ne.index()]
                        && self.is_boundary_edge(ne)
                        && self.loop_vertex(self.edges[ne.index()].loop_) == target
                });
                match next {
                    Some(ne) => l = self.edges[ne.index()].loop_,
                    None => break,
                }
            }
            loops.push(chain);
        }
        loops
    }

    /// Given an edge and one of its vertices, the edge continuing "straight
    /// through" the vertex in quad topology (valence-4 interior vertex).
    pub fn edge_loop_step(&self, e: EdgeId, v: VertexId) -> Option<EdgeId> {
        let edges = self.vertex_edges(v);
        if edges.len() != 4 || self.edge_face_count(e) != 2 {
            return None;
        }
        if edges.iter().any(|&x| self.edge_face_count(x) != 2) {
            return None;
        }
        let faces = self.edge_faces(e);
        let mut candidate = None;
        for &x in edges {
            if x == e {
                continue;
            }
            let shares = faces.iter().any(|&f| self.face_edges(f).any(|fe| fe == x));
            if !shares {
                if candidate.is_some() {
                    return None;
                }
                candidate = Some(x);
            }
        }
        candidate
    }

    /// Edge loop through `e`: walks across valence-4 vertices in both
    /// directions. The returned edges are ordered along the loop.
    pub fn edge_loop(&self, e: EdgeId) -> Vec<EdgeId> {
        let mut visited: HashSet<EdgeId> = HashSet::new();
        visited.insert(e);
        let [a, b] = self.edge_vertices(e);
        let mut forward = Vec::new();
        let mut closed = false;
        let (mut cur, mut v) = (e, b);
        while let Some(n) = self.edge_loop_step(cur, v) {
            if n == e {
                closed = true;
                break;
            }
            if !visited.insert(n) {
                break;
            }
            forward.push(n);
            v = self.edge_other_vertex(n, v);
            cur = n;
        }
        let mut backward = Vec::new();
        if !closed {
            let (mut cur, mut v) = (e, a);
            while let Some(n) = self.edge_loop_step(cur, v) {
                if !visited.insert(n) {
                    break;
                }
                backward.push(n);
                v = self.edge_other_vertex(n, v);
                cur = n;
            }
        }
        backward.reverse();
        backward.push(e);
        backward.extend(forward);
        backward
    }

    /// Edge ring through `e`: walks across quads to the opposite edge in both
    /// directions. Ordered along the ring.
    pub fn edge_ring(&self, e: EdgeId) -> Vec<EdgeId> {
        self.edge_ring_loops(e)
            .into_iter()
            .map(|l| self.loop_edge(l))
            .collect()
    }

    /// Ring walk returning, per ring edge, one loop on it. Consecutive loops
    /// are opposite sides of a quad (or twins across an edge).
    pub(crate) fn edge_ring_loops(&self, e: EdgeId) -> Vec<LoopId> {
        let start_loops = self.edge_loops(e);
        let mut visited: HashSet<EdgeId> = HashSet::new();
        visited.insert(e);
        let mut forward: Vec<LoopId> = Vec::new();
        let mut closed = false;
        let walk = |start: LoopId, out: &mut Vec<LoopId>, visited: &mut HashSet<EdgeId>| -> bool {
            let mut cur = start;
            loop {
                let f = self.loop_face(cur);
                if self.face_len(f) != 4 {
                    return false;
                }
                let opp = self.loop_next(self.loop_next(cur));
                let oe = self.loop_edge(opp);
                if oe == e {
                    return true;
                }
                if !visited.insert(oe) {
                    return false;
                }
                out.push(opp);
                match self.loop_twin(opp) {
                    Some(t) => cur = t,
                    None => return false,
                }
            }
        };
        if let Some(&l0) = start_loops.first() {
            closed = walk(l0, &mut forward, &mut visited);
        }
        let mut backward: Vec<LoopId> = Vec::new();
        if !closed && start_loops.len() == 2 {
            walk(start_loops[1], &mut backward, &mut visited);
        }
        backward.reverse();
        backward.push(start_loops[0]);
        backward.extend(forward);
        backward
    }

    /// Connected components of faces (connected through shared edges).
    pub fn face_components(&self) -> Vec<Vec<FaceId>> {
        let mut comp = vec![u32::MAX; self.faces.len()];
        let mut out = Vec::new();
        for start in self.face_ids() {
            if comp[start.index()] != u32::MAX {
                continue;
            }
            let id = out.len() as u32;
            let mut members = vec![start];
            comp[start.index()] = id;
            let mut i = 0;
            while i < members.len() {
                let f = members[i];
                i += 1;
                for l in self.face_loops(f) {
                    for r in self.edge_loops(self.loop_edge(l)) {
                        let g = self.loop_face(r);
                        if comp[g.index()] == u32::MAX {
                            comp[g.index()] = id;
                            members.push(g);
                        }
                    }
                }
            }
            out.push(members);
        }
        out
    }

    /// Bounding box of all vertices.
    pub fn bounding_box(&self) -> Option<Aabb> {
        Aabb::from_points(self.vertices.iter().map(|v| v.position))
    }

    /// Signed enclosed volume (positive for closed meshes with outward normals).
    pub fn signed_volume(&self) -> f32 {
        let c = self
            .bounding_box()
            .map(|b| b.center())
            .unwrap_or(Vec3::ZERO);
        let mut vol = 0.0f64;
        for [a, b, d] in self.triangulate() {
            let pa = (self.position(VertexId(a)) - c).as_dvec3();
            let pb = (self.position(VertexId(b)) - c).as_dvec3();
            let pd = (self.position(VertexId(d)) - c).as_dvec3();
            vol += pa.dot(pb.cross(pd));
        }
        (vol / 6.0) as f32
    }

    /// Total surface area.
    pub fn surface_area(&self) -> f32 {
        self.face_ids().map(|f| self.face_area(f)).sum()
    }

    /// Vertices that are not used by any face.
    pub fn isolated_vertices(&self) -> Vec<VertexId> {
        self.vertex_ids()
            .filter(|&v| self.vertex_edges(v).is_empty())
            .collect()
    }
}

//! Structural / topological validation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{FaceId, LoopId, Mesh};

/// Result of [`Mesh::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ValidationReport {
    pub vertex_count: usize,
    pub edge_count: usize,
    pub face_count: usize,
    /// V − E + F (2 for a closed genus-0 surface).
    pub euler_characteristic: i64,
    /// Edges with exactly one face.
    pub boundary_edges: usize,
    /// Edges with more than two faces.
    pub non_manifold_edges: usize,
    /// Two-face edges whose faces traverse them in the same direction.
    pub inconsistent_winding_edges: usize,
    /// Vertices whose face fan is not a single edge-connected disk/half-disk.
    pub non_manifold_vertices: usize,
    /// Faces with (near) zero area.
    pub degenerate_faces: usize,
    /// Vertices not used by any face.
    pub isolated_vertices: usize,
    /// Internal references that are out of range or inconsistent.
    pub dangling_references: usize,
    /// No non-manifold edges or vertices.
    pub is_manifold: bool,
    /// Adjacent faces agree on winding everywhere.
    pub is_consistently_oriented: bool,
    /// Manifold and without boundary edges.
    pub is_closed: bool,
    /// Manifold, consistently oriented, no degenerate faces and no dangling references.
    pub is_valid: bool,
    /// Human-readable descriptions of the first problems found.
    pub issues: Vec<String>,
}

const MAX_ISSUES: usize = 32;

impl Mesh {
    /// Validate the mesh structure and topology.
    pub fn validate(&self) -> ValidationReport {
        let mut issues = Vec::new();
        let push = |issues: &mut Vec<String>, s: String| {
            if issues.len() < MAX_ISSUES {
                issues.push(s);
            }
        };

        let nv = self.vertices.len();
        let ne = self.edges.len();
        let nl = self.loops.len();
        let nf = self.faces.len();

        // ---- dangling references / internal consistency ----
        let mut dangling = 0usize;
        for (fi, f) in self.faces.iter().enumerate() {
            let end = f.first.0 as usize + f.len as usize;
            if f.len < 3 || end > nl {
                dangling += 1;
                push(&mut issues, format!("face {fi} has an invalid loop range"));
            }
        }
        for (li, l) in self.loops.iter().enumerate() {
            let bad = l.vertex.index() >= nv
                || l.edge.index() >= ne
                || l.face.index() >= nf
                || l.next.index() >= nl
                || l.prev.index() >= nl
                || l.radial_next.index() >= nl
                || l.radial_prev.index() >= nl;
            if bad {
                dangling += 1;
                push(
                    &mut issues,
                    format!("loop {li} references out of range elements"),
                );
                continue;
            }
            let lid = LoopId(li as u32);
            let consistent = self.loops[l.next.index()].prev == lid
                && self.loops[l.prev.index()].next == lid
                && self.loops[l.radial_next.index()].radial_prev == lid
                && self.loops[l.radial_next.index()].edge == l.edge
                && self.loops[l.next.index()].face == l.face;
            let face = &self.faces[l.face.index()];
            let in_face = (face.first.0..face.first.0 + face.len).contains(&(li as u32));
            let ev = self.edges[l.edge.index()].vertices;
            let target = self.loops[l.next.index()].vertex;
            let edge_ok =
                (ev[0] == l.vertex && ev[1] == target) || (ev[1] == l.vertex && ev[0] == target);
            if !consistent || !in_face || !edge_ok {
                dangling += 1;
                push(&mut issues, format!("loop {li} has inconsistent links"));
            }
        }
        for (ei, e) in self.edges.iter().enumerate() {
            if e.vertices[0].index() >= nv || e.vertices[1].index() >= nv || e.loop_.index() >= nl {
                dangling += 1;
                push(
                    &mut issues,
                    format!("edge {ei} references out of range elements"),
                );
            } else if self.loops[e.loop_.index()].edge.index() != ei {
                dangling += 1;
                push(
                    &mut issues,
                    format!("edge {ei} points to a loop of another edge"),
                );
            }
        }
        if self.vert_edge_start.len() != nv + 1 {
            dangling += 1;
            push(
                &mut issues,
                "vertex adjacency table has the wrong size".into(),
            );
        }
        if dangling > 0 {
            // Topology queries below are unsafe on broken data.
            return ValidationReport {
                vertex_count: nv,
                edge_count: ne,
                face_count: nf,
                euler_characteristic: self.euler_characteristic(),
                boundary_edges: 0,
                non_manifold_edges: 0,
                inconsistent_winding_edges: 0,
                non_manifold_vertices: 0,
                degenerate_faces: 0,
                isolated_vertices: 0,
                dangling_references: dangling,
                is_manifold: false,
                is_consistently_oriented: false,
                is_closed: false,
                is_valid: false,
                issues,
            };
        }

        // ---- edges ----
        let mut boundary = 0;
        let mut non_manifold_edges = 0;
        let mut inconsistent = 0;
        for e in self.edge_ids() {
            let loops = self.edge_loops(e);
            match loops.len() {
                1 => boundary += 1,
                2 => {
                    if self.loop_vertex(loops[0]) == self.loop_vertex(loops[1]) {
                        inconsistent += 1;
                        push(
                            &mut issues,
                            format!("edge {} has faces with inconsistent winding", e.0),
                        );
                    }
                }
                n => {
                    non_manifold_edges += 1;
                    push(&mut issues, format!("edge {} is shared by {n} faces", e.0));
                }
            }
        }

        // ---- vertices: fan connectivity ----
        let mut non_manifold_vertices = 0;
        let mut isolated = 0;
        let mut local_faces: Vec<FaceId> = Vec::new();
        let mut parent: Vec<usize> = Vec::new();
        for v in self.vertex_ids() {
            let edges = self.vertex_edges(v);
            if edges.is_empty() {
                isolated += 1;
                continue;
            }
            local_faces.clear();
            local_faces.extend(self.vertex_faces(v));
            parent.clear();
            parent.extend(0..local_faces.len());
            fn find(p: &mut [usize], mut x: usize) -> usize {
                while p[x] != x {
                    p[x] = p[p[x]];
                    x = p[x];
                }
                x
            }
            for &e in edges {
                let faces = self.edge_faces(e);
                if faces.len() != 2 {
                    continue;
                }
                let a = local_faces.binary_search(&faces[0]);
                let b = local_faces.binary_search(&faces[1]);
                if let (Ok(a), Ok(b)) = (a, b) {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if ra != rb {
                        parent[ra] = rb;
                    }
                }
            }
            let mut roots = 0;
            for i in 0..local_faces.len() {
                if find(&mut parent, i) == i {
                    roots += 1;
                }
            }
            if roots > 1 {
                non_manifold_vertices += 1;
                push(
                    &mut issues,
                    format!("vertex {} joins {roots} separate face fans", v.0),
                );
            }
        }

        // ---- degenerate faces ----
        let scale = self
            .bounding_box()
            .map(|b| b.diagonal().max(1e-6))
            .unwrap_or(1.0);
        let area_eps = 1e-12 * scale * scale;
        let mut degenerate = 0;
        for f in self.face_ids() {
            if self.face_area(f) <= area_eps {
                degenerate += 1;
                push(
                    &mut issues,
                    format!("face {} is degenerate (zero area)", f.0),
                );
            }
        }

        let is_manifold = non_manifold_edges == 0 && non_manifold_vertices == 0;
        let is_consistently_oriented = inconsistent == 0;
        let is_closed = is_manifold && boundary == 0 && nf > 0;
        let is_valid = is_manifold && is_consistently_oriented && degenerate == 0;
        ValidationReport {
            vertex_count: nv,
            edge_count: ne,
            face_count: nf,
            euler_characteristic: self.euler_characteristic(),
            boundary_edges: boundary,
            non_manifold_edges,
            inconsistent_winding_edges: inconsistent,
            non_manifold_vertices,
            degenerate_faces: degenerate,
            isolated_vertices: isolated,
            dangling_references: 0,
            is_manifold,
            is_consistently_oriented,
            is_closed,
            is_valid,
            issues,
        }
    }
}

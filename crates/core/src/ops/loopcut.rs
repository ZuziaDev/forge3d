//! Loop cut: insert edge loops across a ring of quads.

use std::collections::HashMap;

use super::{check_edges, OpResult};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::Soup;
use crate::mesh::{EdgeId, FaceId, Mesh, VertexId};
use crate::selection::ElementSelection;

/// Maximum number of cuts per loop-cut operation.
pub const MAX_LOOP_CUTS: u32 = 64;

/// Insert `cuts` edge loops across the quad ring that contains `edge`.
///
/// The ring is walked through quads in both directions from `edge` until it
/// closes or reaches a boundary / non-quad face. Non-quad faces at the ends
/// of an open ring receive the new vertices on their shared edge. With a
/// single cut, `factor` (0..1, exclusive) positions the cut along `edge`
/// measured from its first vertex; multiple cuts are evenly spaced.
///
/// The new edges of the inserted loop(s) are selected afterwards.
pub fn loop_cut(mesh: &Mesh, edge: EdgeId, cuts: u32, factor: f32) -> CoreResult<OpResult> {
    check_edges(mesh, &[edge])?;
    if !(1..=MAX_LOOP_CUTS).contains(&cuts) {
        return Err(CoreError::param(
            "cuts",
            format!("must be in 1..={MAX_LOOP_CUTS}"),
        ));
    }
    if !(factor.is_finite() && factor > 0.0 && factor < 1.0) {
        return Err(CoreError::param(
            "factor",
            "must be strictly between 0 and 1",
        ));
    }
    // ring edge -> side-0 vertex
    let mut ring: HashMap<EdgeId, VertexId> = HashMap::new();
    let mut ring_order: Vec<EdgeId> = vec![edge];
    let mut quads: HashMap<FaceId, ()> = HashMap::new();
    let mut quad_order: Vec<FaceId> = Vec::new();
    let s0 = mesh.edge_vertices(edge)[0];
    ring.insert(edge, s0);
    for start in mesh.edge_loops(edge).into_iter().take(2) {
        let mut l = start;
        let mut side = s0;
        loop {
            let f = mesh.loop_face(l);
            if mesh.face_len(f) != 4 || quads.contains_key(&f) {
                break;
            }
            quads.insert(f, ());
            quad_order.push(f);
            let q0 = mesh.loop_vertex(l);
            let opp = mesh.loop_next(mesh.loop_next(l));
            let (q2, q3) = (mesh.loop_vertex(opp), mesh.loop_target(opp));
            let next_side = if side == q0 { q3 } else { q2 };
            let oe = mesh.loop_edge(opp);
            if ring.contains_key(&oe) {
                break;
            }
            ring.insert(oe, next_side);
            ring_order.push(oe);
            match mesh.loop_twin(opp) {
                Some(t) => {
                    l = t;
                    side = next_side;
                }
                None => break,
            }
        }
    }
    if quads.is_empty() {
        return Err(CoreError::InvalidSelection(format!(
            "edge {} is not part of any quad; loop cut needs quads",
            edge.0
        )));
    }

    let (positions, polys) = mesh.to_polygons();
    let mut soup = Soup {
        positions,
        faces: Vec::with_capacity(polys.len() + quads.len() * cuts as usize),
    };
    // Cut vertices per ring edge, ordered from side 0 to side 1.
    let mut cut_verts: HashMap<EdgeId, Vec<u32>> = HashMap::new();
    for &e in &ring_order {
        let a = ring[&e];
        let b = mesh.edge_other_vertex(e, a);
        let (pa, pb) = (mesh.position(a), mesh.position(b));
        let list = (1..=cuts)
            .map(|k| {
                // Side 0 of every ring edge corresponds to the first vertex
                // of `edge`, so `factor` applies uniformly along the ring.
                let t = if cuts == 1 {
                    factor
                } else {
                    k as f32 / (cuts + 1) as f32
                };
                soup.add_vertex(pa.lerp(pb, t))
            })
            .collect();
        cut_verts.insert(e, list);
    }
    // Ordered cut vertices when walking an edge from `from`.
    let oriented = |e: EdgeId, from: VertexId| -> Vec<u32> {
        let mut v = cut_verts[&e].clone();
        if ring[&e] != from {
            v.reverse();
        }
        v
    };

    let mut new_edges: Vec<(u32, u32)> = Vec::new();
    for f in mesh.face_ids() {
        if quads.contains_key(&f) {
            let loops: Vec<_> = mesh.face_loops(f).collect();
            let start = (0..4)
                .find(|&i| {
                    ring.contains_key(&mesh.loop_edge(loops[i]))
                        && ring.contains_key(&mesh.loop_edge(loops[(i + 2) % 4]))
                })
                .expect("ring quads have two opposite ring edges");
            let l = loops[start];
            let opp = loops[(start + 2) % 4];
            let q0 = mesh.loop_vertex(l);
            let q1 = mesh.loop_target(l);
            let q2 = mesh.loop_vertex(opp);
            let q3 = mesh.loop_target(opp);
            let mut a = vec![q0.0];
            a.extend(oriented(mesh.loop_edge(l), q0));
            a.push(q1.0);
            let mut b = vec![q3.0];
            b.extend(oriented(mesh.loop_edge(opp), q3));
            b.push(q2.0);
            for j in 0..a.len() - 1 {
                soup.faces.push(vec![a[j], a[j + 1], b[j + 1], b[j]]);
                if j > 0 {
                    new_edges.push((a[j], b[j]));
                }
            }
        } else {
            let mut face = Vec::with_capacity(mesh.face_len(f) + cuts as usize);
            for l in mesh.face_loops(f) {
                let v = mesh.loop_vertex(l);
                face.push(v.0);
                let e = mesh.loop_edge(l);
                if ring.contains_key(&e) {
                    face.extend(oriented(e, v));
                }
            }
            soup.faces.push(face);
        }
    }
    let mesh = soup.into_mesh()?;
    let mut selection = ElementSelection::default();
    for (a, b) in new_edges {
        if let Some(e) = mesh.find_edge(VertexId(a), VertexId(b)) {
            selection.edges.insert(e.0);
        }
    }
    Ok(OpResult { mesh, selection })
}

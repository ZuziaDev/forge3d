//! Single-segment edge bevel (chamfer).

use std::collections::{HashMap, HashSet};


use super::{check_edges, loose_mask, require_nonempty, OpResult};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::Soup;
use crate::mesh::{EdgeId, Mesh};
use crate::selection::ElementSelection;

/// Chamfer the given edges with a single segment of (approximately) `width`
/// measured perpendicular to each beveled edge within its adjacent faces.
///
/// Every endpoint of a beveled edge is replaced: each face corner at such a
/// vertex gets new corner vertex(es) – a miter point when both corner edges
/// are beveled, otherwise points slid along the un-beveled corner edges. A
/// strip quad replaces every beveled edge and a polygon closes the hole at
/// vertices where three or more boundary points meet. The result stays
/// manifold. Selected edges must be interior manifold edges.
///
/// The new bevel faces (strips and corner patches) are selected afterwards.
pub fn bevel_edges(mesh: &Mesh, edges: &[EdgeId], width: f32) -> CoreResult<OpResult> {
    require_nonempty(edges, "edges")?;
    check_edges(mesh, edges)?;
    if !(width.is_finite() && width > 0.0) {
        return Err(CoreError::param("width", "must be a positive finite number"));
    }
    let nv = mesh.vertex_count();
    let mut sel = vec![false; mesh.edge_count()];
    let mut bev = vec![false; nv];
    for &e in edges {
        let loops = mesh.edge_loops(e);
        if loops.len() != 2 {
            return Err(CoreError::InvalidSelection(format!(
                "edge {} has {} faces; only interior edges with exactly two faces can be beveled",
                e.0,
                loops.len()
            )));
        }
        if mesh.loop_vertex(loops[0]) == mesh.loop_vertex(loops[1]) {
            return Err(CoreError::NonManifold(format!(
                "faces around edge {} have inconsistent winding",
                e.0
            )));
        }
        sel[e.index()] = true;
        for v in mesh.edge_vertices(e) {
            bev[v.index()] = true;
        }
    }
    // Every edge around a beveled vertex must be manifold, and the width must
    // fit on the beveled edges.
    for v in mesh.vertex_ids().filter(|v| bev[v.index()]) {
        for &e in mesh.vertex_edges(v) {
            if mesh.edge_face_count(e) > 2 {
                return Err(CoreError::NonManifold(format!(
                    "vertex {} has non-manifold edge {}",
                    v.0, e.0
                )));
            }
        }
    }
    let min_len = edges
        .iter()
        .map(|&e| mesh.edge_length(e))
        .fold(f32::INFINITY, f32::min);
    if width >= 0.5 * min_len {
        return Err(CoreError::param(
            "width",
            format!(
                "too large: must be below half the shortest beveled edge ({:.6})",
                0.5 * min_len
            ),
        ));
    }

    let keep = loose_mask(mesh);
    let (positions, polys) = mesh.to_polygons();
    let mut soup = Soup {
        positions,
        faces: Vec::with_capacity(polys.len() + edges.len() * 2),
    };
    // Origin (beveled vertex) of each new vertex.
    let mut origin: HashMap<u32, u32> = HashMap::new();

    // ---- slide distances along un-beveled edges at beveled vertices ----
    // (vertex, edge) -> (sum, count) of distances requested by faces whose
    // other corner edge is beveled.
    let mut slide_adj: HashMap<(u32, u32), (f32, u32)> = HashMap::new();
    let mut slide_any: HashSet<(u32, u32)> = HashSet::new();
    let corner_dirs = |l| {
        let v = mesh.loop_vertex(l);
        let p = mesh.position(v);
        let prev = mesh.position(mesh.loop_vertex(mesh.loop_prev(l)));
        let next = mesh.position(mesh.loop_target(l));
        ((prev - p).normalize_or_zero(), (next - p).normalize_or_zero())
    };
    for f in mesh.face_ids() {
        for l in mesh.face_loops(f) {
            let v = mesh.loop_vertex(l);
            if !bev[v.index()] {
                continue;
            }
            let e_in = mesh.loop_edge(mesh.loop_prev(l));
            let e_out = mesh.loop_edge(l);
            let (d_prev, d_next) = corner_dirs(l);
            let s = d_prev.cross(d_next).length().max(1e-4);
            match (sel[e_in.index()], sel[e_out.index()]) {
                (true, false) => {
                    let ent = slide_adj.entry((v.0, e_out.0)).or_insert((0.0, 0));
                    ent.0 += width / s;
                    ent.1 += 1;
                }
                (false, true) => {
                    let ent = slide_adj.entry((v.0, e_in.0)).or_insert((0.0, 0));
                    ent.0 += width / s;
                    ent.1 += 1;
                }
                (false, false) => {
                    slide_any.insert((v.0, e_in.0));
                    slide_any.insert((v.0, e_out.0));
                }
                (true, true) => {}
            }
        }
    }
    let mut slide_vert: HashMap<(u32, u32), u32> = HashMap::new();
    let mut keys: Vec<(u32, u32)> = slide_adj.keys().copied().chain(slide_any.iter().copied()).collect();
    keys.sort_unstable();
    keys.dedup();
    for (v, e) in keys {
        let eid = EdgeId(e);
        let vid = crate::mesh::VertexId(v);
        let other = mesh.edge_other_vertex(eid, vid);
        let len = mesh.edge_length(eid);
        let mut d = match slide_adj.get(&(v, e)) {
            Some(&(sum, n)) => sum / n as f32,
            None => width,
        };
        let limit = if bev[other.index()] { 0.45 * len } else { 0.9 * len };
        d = d.min(limit);
        let dir = (mesh.position(other) - mesh.position(vid)).normalize_or_zero();
        let nvx = soup.add_vertex(mesh.position(vid) + dir * d);
        origin.insert(nvx, v);
        slide_vert.insert((v, e), nvx);
    }

    // ---- per corner replacement lists ----
    let nl = mesh.loop_count();
    let mut corner_first = vec![u32::MAX; nl];
    let mut corner_last = vec![u32::MAX; nl];
    for f in mesh.face_ids() {
        let n = mesh.face_normal(f);
        let mut face = Vec::with_capacity(mesh.face_len(f) + 2);
        for l in mesh.face_loops(f) {
            let v = mesh.loop_vertex(l);
            if !bev[v.index()] {
                face.push(v.0);
                corner_first[l.index()] = v.0;
                corner_last[l.index()] = v.0;
                continue;
            }
            let e_in = mesh.loop_edge(mesh.loop_prev(l));
            let e_out = mesh.loop_edge(l);
            let list: Vec<u32> = match (sel[e_in.index()], sel[e_out.index()]) {
                (true, true) => {
                    let (d_prev, d_next) = corner_dirs(l);
                    let bis = d_prev + d_next;
                    let p = mesh.position(v);
                    let q = if bis.length_squared() < 1e-10 {
                        p + n.cross(d_next).normalize_or_zero() * width
                    } else {
                        let cos_t = d_prev.dot(d_next).clamp(-1.0, 1.0);
                        let half_sin = ((1.0 - cos_t) * 0.5).sqrt().max(1e-3);
                        p + bis.normalize() * (width / half_sin)
                    };
                    let m = soup.add_vertex(q);
                    origin.insert(m, v.0);
                    vec![m]
                }
                (true, false) => vec![slide_vert[&(v.0, e_out.0)]],
                (false, true) => vec![slide_vert[&(v.0, e_in.0)]],
                (false, false) => vec![slide_vert[&(v.0, e_in.0)], slide_vert[&(v.0, e_out.0)]],
            };
            corner_first[l.index()] = list[0];
            corner_last[l.index()] = *list.last().expect("non-empty");
            face.extend(list);
        }
        soup.faces.push(face);
    }

    // ---- strips ----
    let mut bevel_faces: Vec<usize> = Vec::new();
    for e in mesh.edge_ids().filter(|e| sel[e.index()]) {
        let loops = mesh.edge_loops(e);
        let (l1, l2) = (loops[0], loops[1]);
        let la = corner_last[l1.index()];
        let lb = corner_first[mesh.loop_next(l1).index()];
        let rb = corner_last[l2.index()];
        let ra = corner_first[mesh.loop_next(l2).index()];
        bevel_faces.push(soup.faces.len());
        soup.faces.push(vec![lb, la, ra, rb]);
    }

    // ---- corner patches: close boundary cycles made only of new vertices
    // belonging to the same beveled vertex ----
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for f in &soup.faces {
        for i in 0..f.len() {
            directed.insert((f[i], f[(i + 1) % f.len()]));
        }
    }
    let mut succ: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &directed {
        if directed.contains(&(b, a)) {
            continue;
        }
        if let (Some(oa), Some(ob)) = (origin.get(&a), origin.get(&b)) {
            if oa == ob {
                // The patch traverses the hole edge in reverse: b -> a.
                succ.insert(b, a);
            }
        }
    }
    let mut starts: Vec<u32> = succ.keys().copied().collect();
    starts.sort_unstable();
    let mut used: HashSet<u32> = HashSet::new();
    for s in starts {
        if used.contains(&s) {
            continue;
        }
        let mut cycle = vec![s];
        let mut cur = s;
        let mut closed = false;
        while let Some(&n) = succ.get(&cur) {
            if n == s {
                closed = true;
                break;
            }
            if cycle.contains(&n) || cycle.len() > succ.len() {
                break;
            }
            cycle.push(n);
            cur = n;
        }
        if closed && cycle.len() >= 3 {
            used.extend(cycle.iter().copied());
            bevel_faces.push(soup.faces.len());
            soup.faces.push(cycle);
        }
    }

    let mut keep_all = keep;
    keep_all.resize(soup.positions.len(), false);
    soup.remove_unused_vertices(Some(&keep_all));
    let mesh = soup.into_mesh()?;
    let mut selection = ElementSelection::default();
    selection.faces.extend(bevel_faces.into_iter().map(|f| f as u32));
    Ok(OpResult { mesh, selection })
}

//! Topology operations: bridge, fill holes, dissolve, triangulate, poke,
//! triangles to quads, smoothing and splitting a mesh by faces.

use std::collections::{BTreeSet, HashMap, HashSet};

use glam::Vec3;

use super::{check_edges, check_faces, check_vertices, finite, loose_mask, OpResult};
use crate::error::{CoreError, CoreResult};
use crate::mesh::soup::Soup;
use crate::mesh::{triangulate_polygon, EdgeId, ElementKind, FaceId, Mesh, VertexId};
use crate::selection::ElementSelection;

/// Maximum smoothing iterations per operation.
pub const MAX_SMOOTH_ITERATIONS: u32 = 100;

/// Closed boundary loops (as vertex cycles following the boundary
/// half-edges) together with their edges.
fn closed_boundary_loops(mesh: &Mesh) -> Vec<(Vec<VertexId>, Vec<EdgeId>)> {
    mesh.boundary_loops()
        .into_iter()
        .filter_map(|lp| {
            if lp.len() < 3 {
                return None;
            }
            let edges: Option<Vec<EdgeId>> = (0..lp.len())
                .map(|i| mesh.find_edge(lp[i], lp[(i + 1) % lp.len()]))
                .collect();
            edges.map(|e| (lp, e))
        })
        .collect()
}

fn faces_selection(range: std::ops::Range<usize>) -> ElementSelection {
    let mut s = ElementSelection::default();
    s.faces.extend(range.map(|f| f as u32));
    s
}

/// Join two boundary loops with a ring of quads. `edges` must touch exactly
/// two closed boundary loops with the same number of vertices. The loops are
/// paired with the rotation that minimises the total bridge length; winding
/// follows the existing faces so the result stays consistently oriented.
/// The new faces are selected.
pub fn bridge_edge_loops(mesh: &Mesh, edges: &[EdgeId]) -> CoreResult<OpResult> {
    super::require_nonempty(edges, "edges")?;
    check_edges(mesh, edges)?;
    let selected: HashSet<EdgeId> = edges.iter().copied().collect();
    let loops: Vec<Vec<VertexId>> = closed_boundary_loops(mesh)
        .into_iter()
        .filter(|(_, es)| es.iter().any(|e| selected.contains(e)))
        .map(|(l, _)| l)
        .collect();
    if loops.len() != 2 {
        return Err(CoreError::InvalidSelection(format!(
            "bridge needs edges on exactly two closed boundary loops (found {}); \
             delete faces to open holes first",
            loops.len()
        )));
    }
    let (a, b) = (&loops[0], &loops[1]);
    if a.len() != b.len() {
        return Err(CoreError::InvalidSelection(format!(
            "boundary loops have different vertex counts ({} and {}); \
             add loop cuts so they match",
            a.len(),
            b.len()
        )));
    }
    let n = a.len();
    let pa: Vec<Vec3> = a.iter().map(|&v| mesh.position(v)).collect();
    let pb: Vec<Vec3> = b.iter().map(|&v| mesh.position(v)).collect();
    let bi = |o: usize, i: usize| (o + n * 2 - i % n) % n;
    let best = (0..n)
        .min_by(|&x, &y| {
            let cost = |o: usize| {
                (0..n)
                    .map(|i| pa[i].distance_squared(pb[bi(o, i)]))
                    .sum::<f32>()
            };
            cost(x).total_cmp(&cost(y))
        })
        .expect("n >= 3");
    let mut soup = Soup::from_mesh(mesh);
    let start = soup.faces.len();
    for i in 0..n {
        soup.faces.push(vec![
            a[(i + 1) % n].0,
            a[i].0,
            b[bi(best, i)].0,
            b[bi(best, i + 1)].0,
        ]);
    }
    let end = soup.faces.len();
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(start..end),
    })
}

/// Close boundary loops with one n-gon each (or a triangle fan around a new
/// center vertex with `fan`). With `edges` only loops touching those edges
/// are filled, otherwise every hole. The new faces are selected.
pub fn fill_holes(mesh: &Mesh, edges: Option<&[EdgeId]>, fan: bool) -> CoreResult<OpResult> {
    if let Some(e) = edges {
        check_edges(mesh, e)?;
    }
    let selected: Option<HashSet<EdgeId>> = edges.map(|e| e.iter().copied().collect());
    let loops: Vec<Vec<VertexId>> = closed_boundary_loops(mesh)
        .into_iter()
        .filter(|(_, es)| {
            selected
                .as_ref()
                .is_none_or(|s| es.iter().any(|e| s.contains(e)))
        })
        .map(|(l, _)| l)
        .collect();
    if loops.is_empty() {
        return Err(CoreError::InvalidSelection(
            "no closed boundary loop (hole) to fill".into(),
        ));
    }
    let mut soup = Soup::from_mesh(mesh);
    let start = soup.faces.len();
    for lp in loops {
        let rev: Vec<u32> = lp.iter().rev().map(|v| v.0).collect();
        if fan {
            let c = rev
                .iter()
                .map(|&v| soup.positions[v as usize])
                .sum::<Vec3>()
                / rev.len() as f32;
            let ci = soup.add_vertex(c);
            for i in 0..rev.len() {
                soup.faces.push(vec![ci, rev[i], rev[(i + 1) % rev.len()]]);
            }
        } else {
            soup.faces.push(rev);
        }
    }
    let end = soup.faces.len();
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(start..end),
    })
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = x;
        while self.0[c] != r {
            let n = self.0[c];
            self.0[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Outline of a group of polygons as a single cycle.
fn region_outline(polys: &[&Vec<u32>]) -> Option<Vec<u32>> {
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for f in polys {
        for i in 0..f.len() {
            directed.insert((f[i], f[(i + 1) % f.len()]));
        }
    }
    let mut succ: HashMap<u32, u32> = HashMap::new();
    let mut boundary = 0;
    for &(a, b) in &directed {
        if !directed.contains(&(b, a)) {
            boundary += 1;
            if succ.insert(a, b).is_some() {
                return None;
            }
        }
    }
    let &start = succ.keys().min()?;
    let mut cycle = vec![start];
    let mut cur = succ[&start];
    while cur != start {
        if cycle.len() > boundary {
            return None;
        }
        cycle.push(cur);
        cur = *succ.get(&cur)?;
    }
    (cycle.len() == boundary && cycle.len() >= 3).then_some(cycle)
}

/// Dissolve elements, merging the faces around them:
///
/// * faces – each edge-connected group of selected faces becomes one polygon,
/// * edges – the two faces of every selected edge are merged,
/// * vertices – all faces around each vertex are merged and the vertex is
///   removed.
///
/// For edges and vertices, leftover vertices with only two neighbours are
/// removed as well. A merged region must be a disk (one outline, no holes).
/// The merged faces are selected afterwards.
pub fn dissolve(mesh: &Mesh, kind: ElementKind, indices: &[u32]) -> CoreResult<OpResult> {
    super::require_nonempty(indices, "elements")?;
    let nf = mesh.face_count();
    let mut uf = UnionFind((0..nf).collect());
    let mut grouped = vec![false; nf];
    let mut candidates: BTreeSet<u32> = BTreeSet::new();
    match kind {
        ElementKind::Face => {
            let faces: Vec<FaceId> = indices.iter().map(|&i| FaceId(i)).collect();
            check_faces(mesh, &faces)?;
            let set: HashSet<u32> = indices.iter().copied().collect();
            for &f in &faces {
                grouped[f.index()] = true;
                for g in mesh.face_neighbors(f) {
                    if set.contains(&g.0) {
                        uf.union(f.index(), g.index());
                    }
                }
            }
        }
        ElementKind::Edge => {
            let edges: Vec<EdgeId> = indices.iter().map(|&i| EdgeId(i)).collect();
            check_edges(mesh, &edges)?;
            for &e in &edges {
                let fs = mesh.edge_faces(e);
                if fs.len() == 2 && fs[0] != fs[1] {
                    uf.union(fs[0].index(), fs[1].index());
                    grouped[fs[0].index()] = true;
                    grouped[fs[1].index()] = true;
                    candidates.extend(mesh.edge_vertices(e).map(|v| v.0));
                }
            }
        }
        ElementKind::Vertex => {
            let verts: Vec<VertexId> = indices.iter().map(|&i| VertexId(i)).collect();
            check_vertices(mesh, &verts)?;
            for &v in &verts {
                let fs = mesh.vertex_faces(v);
                for w in fs.windows(2) {
                    uf.union(w[0].index(), w[1].index());
                }
                for f in fs {
                    grouped[f.index()] = true;
                }
                candidates.insert(v.0);
            }
        }
    }
    let (positions, polys) = mesh.to_polygons();
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for f in (0..nf).filter(|&f| grouped[f]) {
        groups.entry(uf.find(f)).or_default().push(f);
    }
    let mut roots: Vec<usize> = groups.keys().copied().collect();
    roots.sort_unstable();
    let mut out: Vec<Vec<u32>> = Vec::with_capacity(nf);
    let mut merged_rows: Vec<Vec<u32>> = Vec::new();
    for (f, p) in polys.iter().enumerate() {
        if !grouped[f] || groups[&uf.find(f)].len() < 2 {
            out.push(p.clone());
        }
    }
    for r in roots {
        let members = &groups[&r];
        if members.len() < 2 {
            continue;
        }
        let ps: Vec<&Vec<u32>> = members.iter().map(|&f| &polys[f]).collect();
        let outline = region_outline(&ps).ok_or_else(|| {
            CoreError::InvalidSelection(format!(
                "dissolving would create a face with a hole or a pinched outline (faces {:?})",
                members.iter().take(8).collect::<Vec<_>>()
            ))
        })?;
        merged_rows.push(outline);
    }
    if merged_rows.is_empty() && kind != ElementKind::Vertex {
        return Err(CoreError::InvalidSelection(
            "nothing to dissolve: select at least two adjacent faces or an interior edge".into(),
        ));
    }
    let first_merged = out.len();
    out.extend(merged_rows);
    // Remove now-redundant two-neighbour vertices.
    if !candidates.is_empty() {
        let mut nbrs: HashMap<u32, HashSet<u32>> = HashMap::new();
        for f in &out {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                if candidates.contains(&a) {
                    nbrs.entry(a).or_default().insert(b);
                }
                if candidates.contains(&b) {
                    nbrs.entry(b).or_default().insert(a);
                }
            }
        }
        let drop: HashSet<u32> = nbrs
            .into_iter()
            .filter(|(_, n)| n.len() == 2)
            .map(|(v, _)| v)
            .collect();
        if !drop.is_empty() {
            let can_drop = out
                .iter()
                .all(|f| f.iter().filter(|v| !drop.contains(v)).count() >= 3);
            if can_drop {
                for f in &mut out {
                    f.retain(|v| !drop.contains(v));
                }
            }
        }
    }
    let n_out = out.len();
    let mut soup = Soup {
        positions,
        faces: out,
    };
    soup.remove_unused_vertices(Some(&loose_mask(mesh)));
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(first_merged..n_out),
    })
}

/// Triangulate the given faces (ear clipping). The resulting triangles of
/// the affected faces are selected.
pub fn triangulate_faces(mesh: &Mesh, faces: &[FaceId]) -> CoreResult<OpResult> {
    super::require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    let set: HashSet<u32> = faces.iter().map(|f| f.0).collect();
    let (positions, polys) = mesh.to_polygons();
    let mut out = Vec::with_capacity(polys.len() + faces.len() * 2);
    let mut tris = Vec::new();
    for (f, p) in polys.into_iter().enumerate() {
        if !set.contains(&(f as u32)) || p.len() == 3 {
            if set.contains(&(f as u32)) {
                tris.push(p);
            } else {
                out.push(p);
            }
            continue;
        }
        let pts: Vec<Vec3> = p.iter().map(|&v| positions[v as usize]).collect();
        for t in triangulate_polygon(&pts) {
            tris.push(t.iter().map(|&i| p[i]).collect());
        }
    }
    let start = out.len();
    out.extend(tris);
    let end = out.len();
    let soup = Soup {
        positions,
        faces: out,
    };
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(start..end),
    })
}

/// Replace each given face by a fan of triangles around a new center vertex
/// pushed `offset` along the face normal. The new triangles are selected.
pub fn poke_faces(mesh: &Mesh, faces: &[FaceId], offset: f32) -> CoreResult<OpResult> {
    super::require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    finite("offset", offset)?;
    let set: HashSet<u32> = faces.iter().map(|f| f.0).collect();
    let mut soup = Soup::from_mesh(mesh);
    let polys = std::mem::take(&mut soup.faces);
    let mut fans = Vec::new();
    for (f, p) in polys.into_iter().enumerate() {
        if !set.contains(&(f as u32)) {
            soup.faces.push(p);
            continue;
        }
        let fid = FaceId(f as u32);
        let c = soup.add_vertex(mesh.face_centroid(fid) + mesh.face_normal(fid) * offset);
        for i in 0..p.len() {
            fans.push(vec![c, p[i], p[(i + 1) % p.len()]]);
        }
    }
    let start = soup.faces.len();
    soup.faces.extend(fans);
    let end = soup.faces.len();
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(start..end),
    })
}

fn convex_quad(q: [Vec3; 4], n: Vec3) -> bool {
    (0..4).all(|i| {
        let a = q[(i + 1) % 4] - q[i];
        let b = q[(i + 2) % 4] - q[(i + 1) % 4];
        a.cross(b).dot(n) > 1e-12
    })
}

/// Greedily join pairs of adjacent triangles (among `faces`) into convex
/// quads when their normals differ by at most `max_angle_deg`; flattest
/// pairs first. The resulting quads (and unpaired triangles) are selected.
pub fn tris_to_quads(mesh: &Mesh, faces: &[FaceId], max_angle_deg: f32) -> CoreResult<OpResult> {
    super::require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    if !(max_angle_deg.is_finite() && (0.0..=180.0).contains(&max_angle_deg)) {
        return Err(CoreError::param("max_angle_deg", "must be in 0..=180"));
    }
    let cos_max = max_angle_deg.to_radians().cos();
    let tri: HashSet<u32> = faces
        .iter()
        .filter(|&&f| mesh.face_len(f) == 3)
        .map(|f| f.0)
        .collect();
    let mut cands: Vec<(f32, u32, u32, [u32; 4])> = Vec::new();
    for e in mesh.edge_ids() {
        let ls = mesh.edge_loops(e);
        if ls.len() != 2 {
            continue;
        }
        let (f1, f2) = (mesh.loop_face(ls[0]), mesh.loop_face(ls[1]));
        if f1 == f2 || !tri.contains(&f1.0) || !tri.contains(&f2.0) {
            continue;
        }
        let (n1, n2) = (mesh.face_normal(f1), mesh.face_normal(f2));
        let cos = n1.dot(n2);
        if cos < cos_max - 1e-6 {
            continue;
        }
        // tri1: a->b->c (shared a->b), tri2: b->a->d  =>  quad b, c, a, d
        let l1 = ls[0];
        let (a, b) = (mesh.loop_vertex(l1), mesh.loop_target(l1));
        let c = mesh.loop_target(mesh.loop_next(l1));
        let d = mesh.loop_target(mesh.loop_next(ls[1]));
        let q = [b, c, a, d];
        let qp = q.map(|v| mesh.position(v));
        if !convex_quad(qp, (n1 + n2).normalize_or_zero()) {
            continue;
        }
        cands.push((-cos, f1.0, f2.0, q.map(|v| v.0)));
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    let mut used: HashSet<u32> = HashSet::new();
    let (positions, polys) = mesh.to_polygons();
    let mut quads = Vec::new();
    for (_, f1, f2, q) in cands {
        if used.contains(&f1) || used.contains(&f2) {
            continue;
        }
        used.insert(f1);
        used.insert(f2);
        quads.push(q.to_vec());
    }
    let sel_set: HashSet<u32> = faces.iter().map(|f| f.0).collect();
    let mut out = Vec::with_capacity(polys.len());
    let mut rest_sel = Vec::new();
    for (f, p) in polys.into_iter().enumerate() {
        let f = f as u32;
        if used.contains(&f) {
            continue;
        }
        if sel_set.contains(&f) {
            rest_sel.push(p);
        } else {
            out.push(p);
        }
    }
    let start = out.len();
    out.extend(rest_sel);
    out.extend(quads);
    let end = out.len();
    let soup = Soup {
        positions,
        faces: out,
    };
    Ok(OpResult {
        mesh: soup.into_mesh()?,
        selection: faces_selection(start..end),
    })
}

/// Laplacian smoothing: each vertex moves `factor` of the way towards the
/// average of its neighbours, `iterations` times. Boundary vertices only
/// follow their boundary neighbours so open outlines keep their shape.
pub fn smooth_vertices(
    mesh: &Mesh,
    verts: &[VertexId],
    factor: f32,
    iterations: u32,
) -> CoreResult<Mesh> {
    super::require_nonempty(verts, "vertices")?;
    check_vertices(mesh, verts)?;
    if !(factor.is_finite() && (0.0..=1.0).contains(&factor)) {
        return Err(CoreError::param("factor", "must be in 0..=1"));
    }
    if !(1..=MAX_SMOOTH_ITERATIONS).contains(&iterations) {
        return Err(CoreError::param(
            "iterations",
            format!("must be in 1..={MAX_SMOOTH_ITERATIONS}"),
        ));
    }
    let mut uniq = verts.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    let nbrs: Vec<Vec<VertexId>> = uniq
        .iter()
        .map(|&v| {
            if mesh.is_boundary_vertex(v) {
                mesh.vertex_edges(v)
                    .iter()
                    .filter(|&&e| mesh.is_boundary_edge(e))
                    .map(|&e| mesh.edge_other_vertex(e, v))
                    .collect()
            } else {
                mesh.vertex_neighbors(v)
            }
        })
        .collect();
    let mut pos = mesh.positions();
    for _ in 0..iterations {
        let next: Vec<Vec3> = uniq
            .iter()
            .zip(&nbrs)
            .map(|(&v, ns)| {
                let p = pos[v.index()];
                if ns.is_empty() {
                    return p;
                }
                let avg = ns.iter().map(|n| pos[n.index()]).sum::<Vec3>() / ns.len() as f32;
                p + (avg - p) * factor
            })
            .collect();
        for (&v, p) in uniq.iter().zip(next) {
            pos[v.index()] = p;
        }
    }
    let mut out = mesh.clone();
    for &v in &uniq {
        out.set_position(v, pos[v.index()]);
    }
    out.recalc_normals();
    Ok(out)
}

/// Split a mesh into (remaining, extracted) by faces. Vertices are
/// duplicated where the two parts touch; unused vertices are dropped (loose
/// vertices stay with the remaining part).
pub fn split_faces(mesh: &Mesh, faces: &[FaceId]) -> CoreResult<(Mesh, Mesh)> {
    super::require_nonempty(faces, "faces")?;
    check_faces(mesh, faces)?;
    let set: HashSet<u32> = faces.iter().map(|f| f.0).collect();
    let (positions, polys) = mesh.to_polygons();
    let (mut rest, mut taken) = (Vec::new(), Vec::new());
    for (f, p) in polys.into_iter().enumerate() {
        if set.contains(&(f as u32)) {
            taken.push(p);
        } else {
            rest.push(p);
        }
    }
    let mut a = Soup {
        positions: positions.clone(),
        faces: rest,
    };
    a.remove_unused_vertices(Some(&loose_mask(mesh)));
    let mut b = Soup {
        positions,
        faces: taken,
    };
    b.remove_unused_vertices(None);
    Ok((a.into_mesh()?, b.into_mesh()?))
}

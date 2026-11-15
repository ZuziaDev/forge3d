//! Normal / winding repair.

use glam::DVec3;

use super::check_faces;
use crate::error::CoreResult;
use crate::mesh::{FaceId, Mesh};

/// Make face winding consistent across every connected component and orient
/// each component outward (or inward when `inside`), based on its signed
/// volume. Open components with (near) zero volume keep the orientation of
/// their first face. Stored normals are recomputed.
pub fn recalculate_normals(mesh: &Mesh, inside: bool) -> CoreResult<Mesh> {
    let nf = mesh.face_count();
    let mut flip = vec![false; nf];
    let mut visited = vec![false; nf];
    let (positions, mut polys) = mesh.to_polygons();
    let mut components: Vec<Vec<FaceId>> = Vec::new();
    for start in mesh.face_ids() {
        if visited[start.index()] {
            continue;
        }
        visited[start.index()] = true;
        let mut comp = vec![start];
        let mut i = 0;
        while i < comp.len() {
            let f = comp[i];
            i += 1;
            for l in mesh.face_loops(f) {
                let loops = mesh.edge_loops(mesh.loop_edge(l));
                if loops.len() != 2 {
                    continue;
                }
                let r = if loops[0] == l { loops[1] } else { loops[0] };
                let g = mesh.loop_face(r);
                if visited[g.index()] {
                    continue;
                }
                visited[g.index()] = true;
                let same_dir = mesh.loop_vertex(l) == mesh.loop_vertex(r);
                flip[g.index()] = flip[f.index()] ^ same_dir;
                comp.push(g);
            }
        }
        components.push(comp);
    }
    for (f, p) in polys.iter_mut().enumerate() {
        if flip[f] {
            p.reverse();
        }
    }
    for comp in &components {
        let pts: Vec<DVec3> = comp
            .iter()
            .flat_map(|f| {
                polys[f.index()]
                    .iter()
                    .map(|&v| positions[v as usize].as_dvec3())
            })
            .collect();
        let c = pts.iter().copied().sum::<DVec3>() / pts.len().max(1) as f64;
        let mut vol = 0.0f64;
        let mut scale = 0.0f64;
        for f in comp {
            let p = &polys[f.index()];
            let a = positions[p[0] as usize].as_dvec3() - c;
            for k in 1..p.len() - 1 {
                let b = positions[p[k] as usize].as_dvec3() - c;
                let d = positions[p[k + 1] as usize].as_dvec3() - c;
                let t = a.dot(b.cross(d));
                vol += t;
                scale += t.abs();
            }
        }
        let outward = vol >= 0.0;
        if scale > 0.0 && vol.abs() > 1e-9 * scale && outward == inside {
            for f in comp {
                polys[f.index()].reverse();
            }
        }
    }
    Mesh::from_polygons(positions, &polys)
}

/// Reverse the winding of the given faces.
pub fn flip_normals(mesh: &Mesh, faces: &[FaceId]) -> CoreResult<Mesh> {
    check_faces(mesh, faces)?;
    let mut m = mesh.clone();
    m.flip_faces(faces);
    Ok(m)
}

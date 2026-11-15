//! Polygon-soup utilities used by editing operations before rebuilding the
//! half-edge structure.

use std::collections::HashMap;

use glam::Vec3;

use super::Mesh;
use crate::error::CoreResult;

/// Marker for "vertex removed" in remap tables.
pub(crate) const REMOVED: u32 = u32::MAX;

/// Mutable positions + polygons.
#[derive(Debug, Clone, Default)]
pub(crate) struct Soup {
    pub positions: Vec<Vec3>,
    pub faces: Vec<Vec<u32>>,
}

impl Soup {
    pub fn from_mesh(m: &Mesh) -> Soup {
        let (positions, faces) = m.to_polygons();
        Soup { positions, faces }
    }

    pub fn add_vertex(&mut self, p: Vec3) -> u32 {
        self.positions.push(p);
        (self.positions.len() - 1) as u32
    }

    pub fn into_mesh(self) -> CoreResult<Mesh> {
        Mesh::from_polygons(self.positions, &self.faces)
    }

    /// Apply a vertex remap to all faces (`REMOVED` entries drop the face).
    pub fn remap_faces(&mut self, remap: &[u32]) {
        self.faces.retain_mut(|f| {
            for v in f.iter_mut() {
                *v = remap[*v as usize];
                if *v == REMOVED {
                    return false;
                }
            }
            true
        });
    }

    /// Clean faces: collapse consecutive duplicates, split polygons that revisit
    /// a vertex, drop polygons with < 3 vertices, collapse duplicate faces and
    /// remove pairs of mutually opposite faces (which cancel out).
    pub fn clean_faces(&mut self) {
        let mut out: Vec<Vec<u32>> = Vec::with_capacity(self.faces.len());
        for f in std::mem::take(&mut self.faces) {
            split_polygon(f, &mut out);
        }
        // Duplicates and opposite pairs.
        let mut index: HashMap<Vec<u32>, usize> = HashMap::with_capacity(out.len());
        let mut keep = vec![true; out.len()];
        for (i, f) in out.iter().enumerate() {
            let key = canonical(f);
            if index.contains_key(&key) {
                keep[i] = false; // identical duplicate
                continue;
            }
            let mut rev = f.clone();
            rev.reverse();
            let rkey = canonical(&rev);
            if let Some(&j) = index.get(&rkey) {
                if keep[j] {
                    keep[j] = false;
                    keep[i] = false;
                    index.remove(&rkey);
                    continue;
                }
            }
            index.insert(key, i);
        }
        self.faces = out
            .into_iter()
            .zip(keep)
            .filter_map(|(f, k)| k.then_some(f))
            .collect();
    }

    /// Remove vertices not referenced by any face, unless `keep[v]` is set.
    /// Returns the old→new remap.
    pub fn remove_unused_vertices(&mut self, keep: Option<&[bool]>) -> Vec<u32> {
        let mut used = vec![false; self.positions.len()];
        for f in &self.faces {
            for &v in f {
                used[v as usize] = true;
            }
        }
        if let Some(k) = keep {
            for (u, &k) in used.iter_mut().zip(k) {
                *u |= k;
            }
        }
        let mut remap = vec![REMOVED; self.positions.len()];
        let mut new_pos = Vec::with_capacity(self.positions.len());
        for (i, p) in self.positions.iter().enumerate() {
            if used[i] {
                remap[i] = new_pos.len() as u32;
                new_pos.push(*p);
            }
        }
        self.positions = new_pos;
        self.remap_faces(&remap);
        remap
    }
}

/// Compute a weld map: every vertex maps to the first vertex (in index order)
/// within `threshold` of it. Uses a uniform hash grid (O(n)).
pub(crate) fn weld_map(positions: &[Vec3], threshold: f32, subset: Option<&[bool]>) -> Vec<u32> {
    let cell = threshold.max(1e-12);
    let inv = 1.0 / cell;
    let key = |p: Vec3| -> (i64, i64, i64) {
        (
            (p.x * inv).floor() as i64,
            (p.y * inv).floor() as i64,
            (p.z * inv).floor() as i64,
        )
    };
    let t2 = threshold * threshold;
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut remap: Vec<u32> = (0..positions.len() as u32).collect();
    for (i, &p) in positions.iter().enumerate() {
        if let Some(s) = subset {
            if !s[i] {
                continue;
            }
        }
        let (kx, ky, kz) = key(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&(kx + dx, ky + dy, kz + dz)) {
                        for &r in list {
                            if positions[r as usize].distance_squared(p) <= t2 {
                                found = Some(r);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        match found {
            Some(r) => remap[i] = r,
            None => grid.entry((kx, ky, kz)).or_default().push(i as u32),
        }
    }
    remap
}

/// Rotate so the smallest index comes first.
fn canonical(f: &[u32]) -> Vec<u32> {
    let (mi, _) = f
        .iter()
        .enumerate()
        .min_by_key(|(_, &v)| v)
        .expect("non-empty face");
    let mut out = Vec::with_capacity(f.len());
    out.extend_from_slice(&f[mi..]);
    out.extend_from_slice(&f[..mi]);
    out
}

/// Remove consecutive duplicates and split a polygon at repeated vertices.
pub(crate) fn split_polygon(mut f: Vec<u32>, out: &mut Vec<Vec<u32>>) {
    f.dedup();
    while f.len() > 1 && f.first() == f.last() {
        f.pop();
    }
    if f.len() < 3 {
        return;
    }
    // Fast path: no repeated vertex.
    let mut sorted = f.clone();
    sorted.sort_unstable();
    if !sorted.windows(2).any(|w| w[0] == w[1]) {
        out.push(f);
        return;
    }
    let mut stack: Vec<u32> = Vec::with_capacity(f.len());
    for v in f {
        if let Some(pos) = stack.iter().position(|&s| s == v) {
            let sub: Vec<u32> = stack[pos..].to_vec();
            if sub.len() >= 3 {
                out.push(sub);
            }
            stack.truncate(pos + 1);
        } else {
            stack.push(v);
        }
    }
    if stack.len() >= 3 {
        out.push(stack);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_figure_eight() {
        let mut out = Vec::new();
        split_polygon(vec![0, 1, 2, 0, 3, 4], &mut out);
        assert_eq!(out, vec![vec![0, 1, 2], vec![0, 3, 4]]);
    }

    #[test]
    fn opposite_faces_cancel() {
        let mut s = Soup {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z],
            faces: vec![vec![0, 1, 2], vec![2, 1, 0], vec![0, 1, 3], vec![1, 3, 0]],
        };
        s.clean_faces();
        assert_eq!(s.faces, vec![vec![0, 1, 3]]);
    }
}

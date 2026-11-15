//! Shared helpers: atomic file writes, vertex welding, axis conversion,
//! crease-aware normals and export planning.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use forge3d_core::mesh::Mesh;
use forge3d_core::scene::Scene;
use glam::{Affine3A, Mat3, Quat, Vec3};
use uuid::Uuid;

use crate::error::{IoError, IoResult};
use crate::ExportOptions;

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Read a whole file, mapping errors to [`IoError`].
pub(crate) fn read_file(path: &Path) -> IoResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| IoError::io(path, &e))
}

/// Write `bytes` to `path` atomically: the data goes to a temporary file in
/// the same directory which is flushed to disk and then renamed over the
/// destination, so readers never see a half-written file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> IoResult<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| IoError::InvalidData(format!("'{}' is not a file path", path.display())))?
        .to_string_lossy()
        .into_owned();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{file_name}.{}-{nanos}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(IoError::io(path, &e));
    }
    Ok(())
}

/// Resolve the name of a sidecar resource (glTF `.bin`, OBJ `.mtl`) from the
/// in-memory resources or the resource directory. Absolute paths and parent
/// components are refused so a hostile file cannot read outside its folder.
pub(crate) fn load_resource(
    uri: &str,
    resources: &HashMap<String, Vec<u8>>,
    dir: Option<&Path>,
) -> IoResult<Option<Vec<u8>>> {
    if let Some(b) = resources.get(uri) {
        return Ok(Some(b.clone()));
    }
    let Some(dir) = dir else { return Ok(None) };
    let rel = Path::new(uri);
    let safe = !uri.is_empty()
        && !rel.is_absolute()
        && rel.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
    if !safe {
        return Err(IoError::InvalidData(format!(
            "external resource '{uri}' must be a relative path inside the file's folder"
        )));
    }
    let full = dir.join(rel);
    match std::fs::read(&full) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(IoError::io(full, &e)),
    }
}

/// Decode `%XX` escapes of a URI path.
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Meshes
// ---------------------------------------------------------------------------

/// Merge vertices with bit-identical positions. Returns the unique positions
/// and the old→new index map.
pub(crate) fn weld_exact(positions: &[Vec3]) -> (Vec<Vec3>, Vec<u32>) {
    let mut map: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
    let mut unique = Vec::new();
    let mut remap = Vec::with_capacity(positions.len());
    for p in positions {
        // `+ 0.0` turns -0.0 into 0.0 so both weld.
        let key = [
            (p.x + 0.0).to_bits(),
            (p.y + 0.0).to_bits(),
            (p.z + 0.0).to_bits(),
        ];
        let idx = *map.entry(key).or_insert_with(|| {
            unique.push(*p);
            (unique.len() - 1) as u32
        });
        remap.push(idx);
    }
    (unique, remap)
}

/// Build a half-edge mesh from an indexed polygon soup. Optionally welds
/// identical positions first; degenerate and duplicate faces are dropped and
/// vertices that no face uses are removed. The result may have no faces.
pub(crate) fn build_mesh(positions: Vec<Vec3>, faces: Vec<Vec<u32>>, weld: bool) -> IoResult<Mesh> {
    if let Some(i) = positions.iter().position(|p| !p.is_finite()) {
        return Err(IoError::InvalidData(format!(
            "vertex {i} has a non-finite coordinate"
        )));
    }
    let (positions, faces) = if weld {
        let (unique, remap) = weld_exact(&positions);
        let faces = faces
            .into_iter()
            .map(|f| f.into_iter().map(|v| remap[v as usize]).collect())
            .collect();
        (unique, faces)
    } else {
        (positions, faces)
    };
    let mut mesh = Mesh::from_polygons_cleaned(positions, faces)?;
    if !mesh.isolated_vertices().is_empty() {
        mesh.remove_isolated_vertices()?;
    }
    Ok(mesh)
}

// ---------------------------------------------------------------------------
// Axis conversion (Z-up <-> Y-up)
// ---------------------------------------------------------------------------

/// `(x, y, z) -> (x, z, -y)`.
pub(crate) fn pos_to_y_up(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

/// `(x, y, z) -> (x, -z, y)`.
pub(crate) fn pos_from_y_up(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.z, p.y)
}

fn basis_z_to_y() -> Affine3A {
    Affine3A::from_mat3(Mat3::from_cols(Vec3::X, -Vec3::Z, Vec3::Y))
}

/// Conjugate a transform into Y-up space: `C * M * C^-1`.
pub(crate) fn transform_to_y_up(m: &Affine3A) -> Affine3A {
    let c = basis_z_to_y();
    c * *m * c.inverse()
}

/// Conjugate a transform back into Z-up space.
pub(crate) fn transform_from_y_up(m: &Affine3A) -> Affine3A {
    let c = basis_z_to_y();
    c.inverse() * *m * c
}

/// Decompose into translation/rotation/scale if the matrix is free of shear
/// (and invertible); `None` means it must be stored as a matrix.
pub(crate) fn decompose_trs(m: &Affine3A) -> Option<(Vec3, Quat, Vec3)> {
    if !m.to_cols_array().iter().all(|v| v.is_finite()) {
        return None;
    }
    let (s, r, t) = m.to_scale_rotation_translation();
    if !(s.is_finite() && r.is_finite() && t.is_finite()) || s.abs().min_element() < 1e-12 {
        return None;
    }
    let back = Affine3A::from_scale_rotation_translation(s, r, t);
    let a = m.to_cols_array();
    let b = back.to_cols_array();
    let scale = a.iter().fold(1.0_f32, |acc, v| acc.max(v.abs()));
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (x - y).abs() <= 1e-5 * scale)
        .then_some((t, r, s))
}

// ---------------------------------------------------------------------------
// Normals
// ---------------------------------------------------------------------------

/// Vertices split by crease angle for exporting shading normals.
pub(crate) struct CornerNormals {
    /// Output vertices as `(source vertex, normal)`.
    pub vertices: Vec<(u32, Vec3)>,
    /// For every face, the output vertex of each corner (in face order).
    pub faces: Vec<Vec<u32>>,
}

/// Group the corners around each vertex into smoothing clusters (face normals
/// within `crease_deg` of the cluster's first face) and compute one
/// area-weighted normal per cluster. Hard edges therefore stay hard while
/// curved surfaces are shaded smoothly.
pub(crate) fn crease_normals(mesh: &Mesh, crease_deg: f32) -> CornerNormals {
    struct Cluster {
        reference: Vec3,
        sum: Vec3,
        out: u32,
    }
    let cos = crease_deg.to_radians().cos();
    let mut per_vertex: Vec<Vec<Cluster>> = (0..mesh.vertex_count()).map(|_| Vec::new()).collect();
    let mut vertices: Vec<(u32, Vec3)> = Vec::new();
    let mut faces = Vec::with_capacity(mesh.face_count());
    for f in mesh.face_ids() {
        let n = mesh.face_normal(f);
        let weighted = n * mesh.face_area(f).max(1e-20);
        let mut corners = Vec::with_capacity(mesh.face_len(f));
        for v in mesh.face_vertices(f) {
            let clusters = &mut per_vertex[v.index()];
            let found = clusters
                .iter_mut()
                .find(|c| c.reference.dot(n) >= cos || (n == Vec3::ZERO));
            let out = match found {
                Some(c) => {
                    c.sum += weighted;
                    c.out
                }
                None => {
                    let out = vertices.len() as u32;
                    vertices.push((v.0, Vec3::ZERO));
                    clusters.push(Cluster {
                        reference: n,
                        sum: weighted,
                        out,
                    });
                    out
                }
            };
            corners.push(out);
        }
        faces.push(corners);
    }
    for clusters in &per_vertex {
        for c in clusters {
            vertices[c.out as usize].1 = c.sum.try_normalize().unwrap_or(Vec3::Z);
        }
    }
    CornerNormals { vertices, faces }
}

// ---------------------------------------------------------------------------
// Export planning
// ---------------------------------------------------------------------------

/// One node selected for export.
pub(crate) struct PlannedNode {
    pub id: Uuid,
    /// Parent that is part of the export (`None` for export roots).
    pub parent: Option<Uuid>,
    /// Local transform; the world transform for export roots whose real
    /// parent is not exported.
    pub transform: Affine3A,
}

/// Select the nodes to export in pre-order.
pub(crate) fn plan_export(scene: &Scene, opts: &ExportOptions) -> IoResult<Vec<PlannedNode>> {
    let mut included: HashSet<Uuid> = HashSet::new();
    match &opts.nodes {
        Some(list) => {
            for &id in list {
                scene.node(id)?;
                included.insert(id);
                included.extend(scene.descendants(id));
            }
        }
        None => included.extend(scene.traverse()),
    }
    if !opts.include_hidden {
        let hidden: Vec<Uuid> = included
            .iter()
            .copied()
            .filter(|id| scene.get(*id).is_some_and(|n| !n.visible))
            .collect();
        for h in hidden {
            included.remove(&h);
            for d in scene.descendants(h) {
                included.remove(&d);
            }
        }
    }
    let mut out = Vec::new();
    for id in scene.traverse() {
        if !included.contains(&id) {
            continue;
        }
        let node = scene.node(id)?;
        let parent = node.parent().filter(|p| included.contains(p));
        let transform = if parent.is_some() || node.parent().is_none() {
            node.transform
        } else {
            scene.world_transform(id)?
        };
        out.push(PlannedNode {
            id,
            parent,
            transform,
        });
    }
    Ok(out)
}

/// Triangles/polygons of every exported mesh node in world space.
pub(crate) struct WorldMesh {
    pub name: String,
    pub id: Uuid,
    pub positions: Vec<Vec3>,
    /// Polygons, winding corrected for mirroring transforms.
    pub faces: Vec<Vec<u32>>,
    /// Smoothing-cluster normals in world space (empty unless requested).
    pub normals: Vec<Vec3>,
    /// Per face and corner, the index into `normals` (empty unless requested).
    pub face_normals: Vec<Vec<u32>>,
}

/// World-space geometry of all exported nodes that carry a non-empty mesh.
pub(crate) fn world_meshes(
    scene: &Scene,
    plan: &[PlannedNode],
    with_normals: bool,
) -> IoResult<Vec<WorldMesh>> {
    let mut out = Vec::new();
    for p in plan {
        let node = scene.node(p.id)?;
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        if mesh.face_count() == 0 {
            continue;
        }
        let world = scene.world_transform(p.id)?;
        let flip = world.matrix3.determinant() < 0.0;
        let positions = mesh
            .positions()
            .into_iter()
            .map(|v| world.transform_point3(v))
            .collect();
        let mut faces = mesh.face_index_lists();
        let (mut normals, mut face_normals) = (Vec::new(), Vec::new());
        if with_normals {
            let cn = crease_normals(mesh, 40.0);
            let nm = world.matrix3.inverse().transpose();
            normals = cn
                .vertices
                .iter()
                .map(|(_, n)| (nm * *n).try_normalize().unwrap_or(*n))
                .collect();
            face_normals = cn.faces;
        }
        if flip {
            faces.iter_mut().for_each(|f| f.reverse());
            face_normals.iter_mut().for_each(|f| f.reverse());
        }
        out.push(WorldMesh {
            name: node.name.clone(),
            id: p.id,
            positions,
            faces,
            normals,
            face_normals,
        });
    }
    Ok(out)
}

/// Replace characters that would break line based text formats.
pub(crate) fn one_line(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    t.trim().to_string()
}

/// Sanitize a name for use as a sidecar file stem.
pub(crate) fn file_stem_safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() {
        "scene".into()
    } else {
        t
    }
}

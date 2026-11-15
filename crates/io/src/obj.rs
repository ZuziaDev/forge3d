//! Wavefront OBJ / MTL import and export.

use std::collections::HashMap;
use std::fmt::Write as _;

use forge3d_core::scene::{Material, Node, Scene};
use glam::Vec3;
use uuid::Uuid;

use crate::error::{IoError, IoResult};
use crate::util::{
    build_mesh, load_resource, one_line, plan_export, pos_from_y_up, pos_to_y_up, world_meshes,
};
use crate::{ExportBundle, ExportOptions, ExportedFile, ImportOptions, Imported, UpAxis};

const FORMAT: &str = "OBJ";

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn material_key(m: &Material) -> String {
    format!(
        "{:?}|{:?}|{}|{}|{:?}",
        m.name, m.base_color, m.metallic, m.roughness, m.emissive
    )
}

/// Export meshes in world space, preserving polygons (quads stay quads).
pub(crate) fn export_obj(
    scene: &Scene,
    opts: &ExportOptions,
    stem: &str,
) -> IoResult<ExportBundle> {
    let plan = plan_export(scene, opts)?;
    let meshes = world_meshes(scene, &plan, opts.normals)?;
    if meshes.is_empty() {
        return Err(IoError::NothingToExport(
            "no object with mesh geometry".into(),
        ));
    }
    let conv = |p: Vec3| match opts.obj_up_axis {
        UpAxis::Y => pos_to_y_up(p),
        UpAxis::Z => p,
    };

    // Unique material names per distinct material.
    let mut mtl_names: HashMap<String, String> = HashMap::new();
    let mut used_names: HashMap<String, usize> = HashMap::new();
    let mut mtl_order: Vec<(String, Material)> = Vec::new();
    let mut node_mtl: HashMap<Uuid, String> = HashMap::new();
    for m in &meshes {
        if let Some(mat) = scene.node(m.id)?.material.as_ref() {
            let key = material_key(mat);
            let name = match mtl_names.get(&key) {
                Some(n) => n.clone(),
                None => {
                    let base = one_line(&mat.name).replace(' ', "_");
                    let base = if base.is_empty() {
                        "Material".into()
                    } else {
                        base
                    };
                    let count = used_names.entry(base.clone()).or_insert(0);
                    let name = if *count == 0 {
                        base.clone()
                    } else {
                        format!("{base}.{count:03}")
                    };
                    *count += 1;
                    mtl_names.insert(key, name.clone());
                    mtl_order.push((name.clone(), mat.clone()));
                    name
                }
            };
            node_mtl.insert(m.id, name);
        }
    }

    let mut out = String::new();
    out.push_str("# forge3d OBJ export\n");
    if !mtl_order.is_empty() {
        let _ = writeln!(out, "mtllib {stem}.mtl");
    }
    let (mut v_off, mut n_off) = (1usize, 1usize);
    for m in &meshes {
        let _ = writeln!(out, "o {}", one_line(&m.name));
        for p in &m.positions {
            let p = conv(*p);
            let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
        }
        for n in &m.normals {
            let n = conv(*n);
            let _ = writeln!(out, "vn {} {} {}", n.x, n.y, n.z);
        }
        if let Some(name) = node_mtl.get(&m.id) {
            let _ = writeln!(out, "usemtl {name}");
        }
        for (fi, f) in m.faces.iter().enumerate() {
            out.push('f');
            for (ci, &v) in f.iter().enumerate() {
                if m.face_normals.is_empty() {
                    let _ = write!(out, " {}", v as usize + v_off);
                } else {
                    let _ = write!(
                        out,
                        " {}//{}",
                        v as usize + v_off,
                        m.face_normals[fi][ci] as usize + n_off
                    );
                }
            }
            out.push('\n');
        }
        v_off += m.positions.len();
        n_off += m.normals.len();
    }
    let mut files = vec![ExportedFile {
        name: format!("{stem}.obj"),
        bytes: out.into_bytes(),
    }];
    if !mtl_order.is_empty() {
        let mut mtl = String::from("# forge3d MTL export\n");
        for (name, m) in &mtl_order {
            let _ = writeln!(mtl, "newmtl {name}");
            let _ = writeln!(
                mtl,
                "Kd {} {} {}",
                m.base_color[0], m.base_color[1], m.base_color[2]
            );
            let _ = writeln!(mtl, "d {}", m.base_color[3]);
            let _ = writeln!(
                mtl,
                "Ke {} {} {}",
                m.emissive[0], m.emissive[1], m.emissive[2]
            );
            let _ = writeln!(mtl, "Pm {}", m.metallic);
            let _ = writeln!(mtl, "Pr {}", m.roughness);
            let shininess = (1.0 - m.roughness).powi(2) * 1000.0;
            let _ = writeln!(mtl, "Ns {shininess}");
            let _ = writeln!(mtl, "illum 2");
            mtl.push('\n');
        }
        files.push(ExportedFile {
            name: format!("{stem}.mtl"),
            bytes: mtl.into_bytes(),
        });
    }
    Ok(ExportBundle { files })
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

fn parse_f32(tok: Option<&str>, line: usize, what: &str) -> IoResult<f32> {
    let t = tok.ok_or_else(|| IoError::parse_at(FORMAT, line, format!("missing {what}")))?;
    let v: f32 = t
        .parse()
        .map_err(|_| IoError::parse_at(FORMAT, line, format!("invalid {what} '{t}'")))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(IoError::parse_at(
            FORMAT,
            line,
            format!("{what} is not finite"),
        ))
    }
}

/// Iterate logical lines (joining `\` continuations) with their 1-based
/// starting line number, comments stripped.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut acc = String::new();
    let mut start = 0;
    for (i, raw) in text.lines().enumerate() {
        let raw = raw.split('#').next().unwrap_or("");
        if acc.is_empty() {
            start = i + 1;
        }
        if let Some(stripped) = raw.trim_end().strip_suffix('\\') {
            acc.push_str(stripped);
            acc.push(' ');
            continue;
        }
        acc.push_str(raw);
        let line = std::mem::take(&mut acc);
        let t = line.trim();
        if !t.is_empty() {
            out.push((start, t.to_string()));
        }
    }
    let t = acc.trim();
    if !t.is_empty() {
        out.push((start, t.to_string()));
    }
    out
}

/// Split a directive line into keyword and the rest.
fn split_keyword(line: &str) -> (&str, &str) {
    match line.split_once(char::is_whitespace) {
        Some((k, r)) => (k, r.trim()),
        None => (line, ""),
    }
}

/// Parse an `.mtl` text into materials by name.
pub(crate) fn parse_mtl(text: &str) -> IoResult<HashMap<String, Material>> {
    let mut out: HashMap<String, Material> = HashMap::new();
    let mut cur: Option<(String, Material, bool)> = None; // name, material, roughness explicitly set
    let mut flush = |cur: &mut Option<(String, Material, bool)>| {
        if let Some((n, m, _)) = cur.take() {
            out.insert(n, m);
        }
    };
    for (ln, line) in logical_lines(text) {
        let (kw, rest) = split_keyword(&line);
        let mut it = rest.split_whitespace();
        if kw == "newmtl" {
            flush(&mut cur);
            let name = rest.to_string();
            if name.is_empty() {
                return Err(IoError::parse_at("MTL", ln, "newmtl without a name"));
            }
            cur = Some((
                name.clone(),
                Material {
                    name,
                    ..Material::default()
                },
                false,
            ));
            continue;
        }
        let Some((_, m, rough_set)) = cur.as_mut() else {
            continue;
        };
        let c01 = |v: f32| v.clamp(0.0, 1.0);
        match kw {
            "Kd" => {
                let r = parse_f32(it.next(), ln, "Kd red")?;
                let g = parse_f32(it.next(), ln, "Kd green").unwrap_or(r);
                let b = parse_f32(it.next(), ln, "Kd blue").unwrap_or(r);
                m.base_color[..3].copy_from_slice(&[c01(r), c01(g), c01(b)]);
            }
            "d" => m.base_color[3] = c01(parse_f32(it.next(), ln, "d")?),
            "Tr" => m.base_color[3] = c01(1.0 - parse_f32(it.next(), ln, "Tr")?),
            "Ke" => {
                let r = parse_f32(it.next(), ln, "Ke red")?;
                let g = parse_f32(it.next(), ln, "Ke green").unwrap_or(r);
                let b = parse_f32(it.next(), ln, "Ke blue").unwrap_or(r);
                m.emissive = [r.max(0.0), g.max(0.0), b.max(0.0)];
            }
            "Pm" => m.metallic = c01(parse_f32(it.next(), ln, "Pm")?),
            "Pr" => {
                m.roughness = c01(parse_f32(it.next(), ln, "Pr")?);
                *rough_set = true;
            }
            "Ns" if !*rough_set => {
                let ns = parse_f32(it.next(), ln, "Ns")?.clamp(0.0, 1000.0);
                m.roughness = 1.0 - (ns / 1000.0).sqrt();
            }
            _ => {}
        }
    }
    flush(&mut cur);
    Ok(out)
}

struct Face {
    verts: Vec<u32>,
}

struct Object {
    name: Option<String>,
    /// (material name, faces) in first-use order.
    groups: Vec<(Option<String>, Vec<Face>)>,
}

impl Object {
    fn new(name: Option<String>) -> Object {
        Object {
            name,
            groups: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.groups.iter().all(|(_, f)| f.is_empty())
    }

    fn push(&mut self, material: &Option<String>, face: Face) {
        let i = match self.groups.iter().position(|(m, _)| m == material) {
            Some(i) => i,
            None => {
                self.groups.push((material.clone(), Vec::new()));
                self.groups.len() - 1
            }
        };
        self.groups[i].1.push(face);
    }
}

fn resolve_index(tok: &str, count: usize, line: usize) -> IoResult<u32> {
    let raw = tok.split('/').next().unwrap_or("");
    let i: i64 = raw
        .parse()
        .map_err(|_| IoError::parse_at(FORMAT, line, format!("invalid face index '{tok}'")))?;
    let resolved = if i > 0 {
        i - 1
    } else if i < 0 {
        count as i64 + i
    } else {
        return Err(IoError::parse_at(
            FORMAT,
            line,
            "face index 0 is invalid (indices are 1-based)",
        ));
    };
    if resolved < 0 || resolved as usize >= count {
        return Err(IoError::parse_at(
            FORMAT,
            line,
            format!(
                "face index {i} refers to a vertex that does not exist ({count} defined so far)"
            ),
        ));
    }
    Ok(resolved as u32)
}

/// Parse OBJ bytes (and the referenced MTL files) into a sub-scene.
pub(crate) fn import_obj(bytes: &[u8], opts: &ImportOptions, name: &str) -> IoResult<Imported> {
    let text = String::from_utf8_lossy(bytes);
    let lines = logical_lines(&text);
    let has_o = lines.iter().any(|(_, l)| split_keyword(l).0 == "o");
    let mut warnings = Vec::new();

    let mut positions: Vec<Vec3> = Vec::new();
    let mut objects: Vec<Object> = vec![Object::new(None)];
    let mut material: Option<String> = None;
    let mut mtl_files: Vec<String> = Vec::new();

    for (ln, line) in &lines {
        let ln = *ln;
        let (kw, rest) = split_keyword(line);
        match kw {
            "v" => {
                let mut it = rest.split_whitespace();
                let x = parse_f32(it.next(), ln, "x coordinate")?;
                let y = parse_f32(it.next(), ln, "y coordinate")?;
                let z = parse_f32(it.next(), ln, "z coordinate")?;
                positions.push(Vec3::new(x, y, z));
            }
            "f" => {
                let toks: Vec<&str> = rest.split_whitespace().collect();
                if toks.len() < 3 {
                    return Err(IoError::parse_at(
                        FORMAT,
                        ln,
                        format!("face has {} vertices; at least 3 are required", toks.len()),
                    ));
                }
                let verts = toks
                    .iter()
                    .map(|t| resolve_index(t, positions.len(), ln))
                    .collect::<IoResult<Vec<u32>>>()?;
                objects
                    .last_mut()
                    .expect("at least one object")
                    .push(&material, Face { verts });
            }
            "o" | "g" if kw == "o" || !has_o => {
                let n = (!rest.is_empty()).then(|| rest.to_string());
                let cur = objects.last_mut().expect("at least one object");
                if cur.is_empty() {
                    cur.name = n;
                } else {
                    objects.push(Object::new(n));
                }
            }
            "usemtl" => material = (!rest.is_empty()).then(|| rest.to_string()),
            "mtllib" => mtl_files.push(rest.to_string()),
            _ => {}
        }
    }

    let mut mtl: HashMap<String, Material> = HashMap::new();
    for f in &mtl_files {
        let mut candidates = vec![f.clone()];
        candidates.extend(f.split_whitespace().map(str::to_string));
        let mut found = false;
        for c in candidates {
            if let Some(b) = load_resource(&c, &opts.resources, opts.resource_dir.as_deref())? {
                mtl.extend(parse_mtl(&String::from_utf8_lossy(&b))?);
                found = true;
                break;
            }
        }
        if !found {
            warnings.push(format!(
                "material library '{f}' was not found; default materials are used"
            ));
        }
    }

    let mut scene = Scene::new();
    let default_name = opts.name.clone().unwrap_or_else(|| name.to_string());
    for obj in objects {
        if obj.is_empty() {
            continue;
        }
        let obj_name = obj.name.clone().unwrap_or_else(|| default_name.clone());
        let mut built: Vec<(Option<String>, forge3d_core::mesh::Mesh)> = Vec::new();
        for (mat, faces) in obj.groups {
            // Compact the global vertex indices into a local list.
            let mut local: HashMap<u32, u32> = HashMap::new();
            let mut pos: Vec<Vec3> = Vec::new();
            let polys: Vec<Vec<u32>> = faces
                .iter()
                .map(|f| {
                    f.verts
                        .iter()
                        .map(|&g| {
                            *local.entry(g).or_insert_with(|| {
                                let p = positions[g as usize];
                                pos.push(match opts.obj_up_axis {
                                    UpAxis::Y => pos_from_y_up(p),
                                    UpAxis::Z => p,
                                });
                                (pos.len() - 1) as u32
                            })
                        })
                        .collect()
                })
                .collect();
            let mesh = build_mesh(pos, polys, opts.weld)?;
            if mesh.face_count() == 0 {
                warnings.push(format!(
                    "object '{obj_name}': all faces were degenerate and dropped"
                ));
                continue;
            }
            built.push((mat, mesh));
        }
        let resolve = |m: &Option<String>| {
            m.as_ref().map(|n| {
                mtl.get(n).cloned().unwrap_or_else(|| Material {
                    name: n.clone(),
                    ..Material::default()
                })
            })
        };
        match built.len() {
            0 => {}
            1 => {
                let (m, mesh) = built.remove(0);
                let mut node = Node::new(obj_name).with_mesh(mesh);
                node.material = resolve(&m);
                scene.add_node(node, None)?;
            }
            _ => {
                let parent = scene.add_node(Node::new(obj_name.clone()), None)?;
                for (k, (m, mesh)) in built.into_iter().enumerate() {
                    let label = m.clone().unwrap_or_else(|| format!("part {}", k + 1));
                    let mut node = Node::new(format!("{obj_name} ({label})")).with_mesh(mesh);
                    node.material = resolve(&m);
                    scene.add_node(node, Some(parent))?;
                }
            }
        }
    }
    if scene.is_empty() {
        return Err(IoError::NothingToImport(
            "the OBJ file contains no faces".into(),
        ));
    }
    Ok(Imported::new(scene, name.to_string(), warnings))
}

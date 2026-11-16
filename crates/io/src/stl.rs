//! STL import (binary + ASCII, auto-detected) and export.

use std::fmt::Write as _;

use forge3d_core::scene::{Node, Scene};
use glam::Vec3;

use crate::error::{IoError, IoResult};
use crate::util::{build_mesh, one_line, plan_export};
use crate::{ExportBundle, ExportOptions, ExportedFile, ImportOptions, Imported};

const FORMAT: &str = "STL";
const HEADER_LEN: usize = 80;
const TRI_BYTES: usize = 50;

fn facet_normal(t: &[Vec3; 3]) -> Vec3 {
    (t[1] - t[0])
        .cross(t[2] - t[0])
        .try_normalize()
        .unwrap_or(Vec3::ZERO)
}

/// Export all meshes of the (sub)scene as one merged solid in world space
/// (Z-up, as expected by slicers).
pub(crate) fn export_stl(
    scene: &Scene,
    opts: &ExportOptions,
    stem: &str,
) -> IoResult<ExportBundle> {
    let plan = plan_export(scene, opts)?;
    let mut tris: Vec<[Vec3; 3]> = Vec::new();
    for p in &plan {
        let Some(mesh) = scene.node(p.id)?.evaluated_mesh() else {
            continue;
        };
        let world = scene.world_transform(p.id)?;
        let flip = world.matrix3.determinant() < 0.0;
        let pos: Vec<Vec3> = mesh
            .positions()
            .into_iter()
            .map(|v| world.transform_point3(v))
            .collect();
        for [a, b, c] in mesh.triangulate() {
            let (a, b, c) = (pos[a as usize], pos[b as usize], pos[c as usize]);
            tris.push(if flip { [a, c, b] } else { [a, b, c] });
        }
    }
    if tris.is_empty() {
        return Err(IoError::NothingToExport(
            "no object with mesh geometry".into(),
        ));
    }
    let bytes = if opts.stl_ascii {
        let mut s = String::new();
        let name = one_line(stem);
        let _ = writeln!(s, "solid {name}");
        for t in &tris {
            let n = facet_normal(t);
            let _ = writeln!(s, "  facet normal {} {} {}", n.x, n.y, n.z);
            let _ = writeln!(s, "    outer loop");
            for v in t {
                let _ = writeln!(s, "      vertex {} {} {}", v.x, v.y, v.z);
            }
            let _ = writeln!(s, "    endloop");
            let _ = writeln!(s, "  endfacet");
        }
        let _ = writeln!(s, "endsolid {name}");
        s.into_bytes()
    } else {
        let count = u32::try_from(tris.len())
            .map_err(|_| IoError::InvalidData("too many triangles for binary STL".into()))?;
        let mut b = Vec::with_capacity(HEADER_LEN + 4 + tris.len() * TRI_BYTES);
        // The header must not start with "solid" or readers mistake the file for ASCII.
        let mut header = b"forge3d binary STL".to_vec();
        header.resize(HEADER_LEN, 0);
        b.extend_from_slice(&header);
        b.extend_from_slice(&count.to_le_bytes());
        for t in &tris {
            let n = facet_normal(t);
            for c in n.to_array() {
                b.extend_from_slice(&c.to_le_bytes());
            }
            for v in t {
                for c in v.to_array() {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
            b.extend_from_slice(&0u16.to_le_bytes());
        }
        b
    };
    Ok(ExportBundle {
        files: vec![ExportedFile {
            name: format!("{stem}.stl"),
            bytes,
        }],
    })
}

fn looks_like_ascii(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.len() >= 5 && t[..5].eq_ignore_ascii_case("solid") && !head.contains(&0)
}

fn rd_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn parse_binary(bytes: &[u8]) -> IoResult<Vec<[Vec3; 3]>> {
    if bytes.len() < HEADER_LEN + 4 {
        return Err(IoError::parse(
            FORMAT,
            "file is too short for a binary STL header",
        ));
    }
    let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let need = count
        .checked_mul(TRI_BYTES)
        .and_then(|n| n.checked_add(HEADER_LEN + 4))
        .ok_or_else(|| IoError::parse(FORMAT, "triangle count overflows"))?;
    if bytes.len() < need {
        return Err(IoError::parse(
            FORMAT,
            format!(
                "truncated binary STL: header announces {count} triangles ({need} bytes) but the file has {} bytes",
                bytes.len()
            ),
        ));
    }
    let mut tris = Vec::with_capacity(count);
    for i in 0..count {
        let base = HEADER_LEN + 4 + i * TRI_BYTES + 12; // skip the stored normal
        let mut t = [Vec3::ZERO; 3];
        for (k, v) in t.iter_mut().enumerate() {
            let at = base + k * 12;
            *v = Vec3::new(
                rd_f32(bytes, at),
                rd_f32(bytes, at + 4),
                rd_f32(bytes, at + 8),
            );
        }
        tris.push(t);
    }
    Ok(tris)
}

struct Solid {
    name: Option<String>,
    tris: Vec<[Vec3; 3]>,
}

fn parse_ascii(text: &str) -> IoResult<Vec<Solid>> {
    let mut solids: Vec<Solid> = Vec::new();
    let mut cur: Option<Solid> = None;
    let mut facet: Option<Vec<Vec3>> = None;
    for (i, raw) in text.lines().enumerate() {
        let ln = i + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let (kw, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k.to_ascii_lowercase(), r.trim()),
            None => (line.to_ascii_lowercase(), ""),
        };
        match kw.as_str() {
            "solid" => {
                if let Some(s) = cur.take() {
                    solids.push(s);
                }
                cur = Some(Solid {
                    name: (!rest.is_empty()).then(|| rest.to_string()),
                    tris: Vec::new(),
                });
            }
            "endsolid" => {
                if let Some(s) = cur.take() {
                    solids.push(s);
                }
            }
            "facet" => {
                if facet.is_some() {
                    return Err(IoError::parse_at(
                        FORMAT,
                        ln,
                        "'facet' inside another facet",
                    ));
                }
                facet = Some(Vec::with_capacity(3));
            }
            "vertex" => {
                let f = facet
                    .as_mut()
                    .ok_or_else(|| IoError::parse_at(FORMAT, ln, "'vertex' outside of a facet"))?;
                let mut c = [0.0f32; 3];
                let mut it = rest.split_whitespace();
                for slot in c.iter_mut() {
                    let t = it.next().ok_or_else(|| {
                        IoError::parse_at(FORMAT, ln, "vertex needs 3 coordinates")
                    })?;
                    *slot = t.parse().map_err(|_| {
                        IoError::parse_at(FORMAT, ln, format!("invalid coordinate '{t}'"))
                    })?;
                }
                f.push(Vec3::from_array(c));
            }
            "endfacet" => {
                let f = facet
                    .take()
                    .ok_or_else(|| IoError::parse_at(FORMAT, ln, "'endfacet' without 'facet'"))?;
                if f.len() != 3 {
                    return Err(IoError::parse_at(
                        FORMAT,
                        ln,
                        format!("facet has {} vertices; exactly 3 are required", f.len()),
                    ));
                }
                let s = cur.get_or_insert(Solid {
                    name: None,
                    tris: Vec::new(),
                });
                s.tris.push([f[0], f[1], f[2]]);
            }
            _ => {}
        }
    }
    if facet.is_some() {
        return Err(IoError::parse(FORMAT, "unterminated facet at end of file"));
    }
    if let Some(s) = cur.take() {
        solids.push(s);
    }
    Ok(solids)
}

fn solid_node(tris: &[[Vec3; 3]], name: String, opts: &ImportOptions) -> IoResult<Option<Node>> {
    let positions: Vec<Vec3> = tris.iter().flat_map(|t| t.iter().copied()).collect();
    let faces: Vec<Vec<u32>> = (0..tris.len() as u32)
        .map(|i| vec![3 * i, 3 * i + 1, 3 * i + 2])
        .collect();
    let mesh = build_mesh(positions, faces, opts.weld)?;
    Ok((mesh.face_count() > 0).then(|| Node::new(name).with_mesh(mesh)))
}

/// Import binary or ASCII STL (auto-detected) into a sub-scene.
pub(crate) fn import_stl(bytes: &[u8], opts: &ImportOptions, name: &str) -> IoResult<Imported> {
    let default_name = opts.name.clone().unwrap_or_else(|| name.to_string());
    let binary_size_matches = bytes.len() >= HEADER_LEN + 4 && {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        n.checked_mul(TRI_BYTES)
            .and_then(|v| v.checked_add(HEADER_LEN + 4))
            == Some(bytes.len())
    };
    let mut scene = Scene::new();
    let mut warnings = Vec::new();
    if !binary_size_matches && looks_like_ascii(bytes) {
        let text = String::from_utf8_lossy(bytes);
        for (i, solid) in parse_ascii(&text)?.into_iter().enumerate() {
            let n = solid.name.unwrap_or_else(|| default_name.clone());
            match solid_node(&solid.tris, n.clone(), opts)? {
                Some(node) => {
                    scene.add_node(node, None)?;
                }
                None => warnings.push(format!("solid {} ('{n}') has no usable triangles", i + 1)),
            }
        }
    } else {
        let tris = parse_binary(bytes)?;
        if bytes.len() > HEADER_LEN + 4 + tris.len() * TRI_BYTES {
            warnings.push("ignored trailing bytes after the last triangle".to_string());
        }
        if let Some(node) = solid_node(&tris, default_name, opts)? {
            scene.add_node(node, None)?;
        }
    }
    if scene.is_empty() {
        return Err(IoError::NothingToImport(
            "the STL file contains no triangles".into(),
        ));
    }
    Ok(Imported::new(scene, name.to_string(), warnings))
}

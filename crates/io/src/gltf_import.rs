//! glTF 2.0 / GLB import.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use forge3d_core::mesh::Mesh;
use forge3d_core::scene::{Material, Node, Scene};
use glam::{Affine3A, Mat4, Quat, Vec3};
use gltf::buffer::Source;
use gltf::mesh::Mode;
use uuid::Uuid;

use crate::error::{IoError, IoResult};
use crate::util::{build_mesh, load_resource, percent_decode, pos_from_y_up, transform_from_y_up};
use crate::{ImportOptions, Imported};

const FORMAT: &str = "glTF";

fn decode_data_uri(uri: &str) -> IoResult<Vec<u8>> {
    let rest = &uri["data:".len()..];
    let (header, payload) = rest
        .split_once(',')
        .ok_or_else(|| IoError::parse(FORMAT, "malformed data URI (missing ',')"))?;
    if header.split(';').any(|p| p.eq_ignore_ascii_case("base64")) {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| IoError::parse(FORMAT, format!("invalid base64 buffer: {e}")))
    } else {
        Ok(percent_decode(payload).into_bytes())
    }
}

fn load_buffers(
    doc: &gltf::Document,
    blob: &mut Option<Vec<u8>>,
    opts: &ImportOptions,
) -> IoResult<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for buffer in doc.buffers() {
        let data = match buffer.source() {
            Source::Bin => blob.take().ok_or_else(|| {
                IoError::parse(
                    FORMAT,
                    "buffer refers to the GLB binary chunk but there is none",
                )
            })?,
            Source::Uri(uri) if uri.starts_with("data:") => decode_data_uri(uri)?,
            Source::Uri(uri) => {
                let name = percent_decode(uri);
                load_resource(&name, &opts.resources, opts.resource_dir.as_deref())?
                    .ok_or_else(|| IoError::FileNotFound(name.into()))?
            }
        };
        if data.len() < buffer.length() {
            return Err(IoError::parse(
                FORMAT,
                format!(
                    "buffer {} is {} bytes but declares {}",
                    buffer.index(),
                    data.len(),
                    buffer.length()
                ),
            ));
        }
        out.push(data);
    }
    Ok(out)
}

fn convert_material(m: &gltf::Material) -> Material {
    let pbr = m.pbr_metallic_roughness();
    let clamp01 = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let strength = m.emissive_strength().unwrap_or(1.0);
    let e = m.emissive_factor();
    Material {
        name: m.name().unwrap_or("Material").to_string(),
        base_color: pbr.base_color_factor().map(clamp01),
        metallic: clamp01(pbr.metallic_factor()),
        roughness: clamp01(pbr.roughness_factor()),
        emissive: e.map(|c| {
            let v = c * strength;
            if v.is_finite() {
                v.max(0.0)
            } else {
                0.0
            }
        }),
    }
}

fn node_transform(n: &gltf::Node) -> Affine3A {
    let local = match n.transform() {
        gltf::scene::Transform::Matrix { matrix } => {
            Affine3A::from_mat4(Mat4::from_cols_array_2d(&matrix))
        }
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => {
            let q = Quat::from_array(rotation);
            let q = if q.length_squared() > 0.0 {
                q.normalize()
            } else {
                Quat::IDENTITY
            };
            Affine3A::from_scale_rotation_translation(
                Vec3::from_array(scale),
                q,
                Vec3::from_array(translation),
            )
        }
    };
    transform_from_y_up(&local)
}

fn node_visible(n: &gltf::Node) -> bool {
    n.extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
        .and_then(|v| v.pointer("/forge3d/visible").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

/// Triangle soup collected for one material of a glTF mesh.
struct Group {
    material: Option<usize>,
    positions: Vec<Vec3>,
    faces: Vec<Vec<u32>>,
}

fn collect_groups(
    mesh: &gltf::Mesh,
    buffers: &[Vec<u8>],
    warnings: &mut Vec<String>,
) -> IoResult<Vec<Group>> {
    let mut groups: Vec<Group> = Vec::new();
    for prim in mesh.primitives() {
        let mode = prim.mode();
        if !matches!(
            mode,
            Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
        ) {
            warnings.push(format!(
                "mesh '{}': skipped a {:?} primitive (only triangles are imported)",
                mesh.name().unwrap_or("?"),
                mode
            ));
            continue;
        }
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.as_slice()));
        let Some(pos) = reader.read_positions() else {
            warnings.push(format!(
                "mesh '{}': skipped a primitive without POSITION",
                mesh.name().unwrap_or("?")
            ));
            continue;
        };
        let local: Vec<Vec3> = pos.map(|p| pos_from_y_up(Vec3::from_array(p))).collect();
        let indices: Vec<u32> = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..local.len() as u32).collect(),
        };
        if let Some(bad) = indices.iter().find(|&&i| i as usize >= local.len()) {
            return Err(IoError::InvalidData(format!(
                "mesh '{}': index {bad} is out of range for {} vertices",
                mesh.name().unwrap_or("?"),
                local.len()
            )));
        }
        let tris: Vec<[u32; 3]> = match mode {
            Mode::Triangles => indices
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect(),
            Mode::TriangleStrip => (0..indices.len().saturating_sub(2))
                .map(|i| {
                    if i % 2 == 0 {
                        [indices[i], indices[i + 1], indices[i + 2]]
                    } else {
                        [indices[i + 1], indices[i], indices[i + 2]]
                    }
                })
                .collect(),
            _ => (1..indices.len().saturating_sub(1))
                .map(|i| [indices[0], indices[i], indices[i + 1]])
                .collect(),
        };
        let material = prim.material().index();
        let group = match groups.iter().position(|g| g.material == material) {
            Some(i) => &mut groups[i],
            None => {
                groups.push(Group {
                    material,
                    positions: Vec::new(),
                    faces: Vec::new(),
                });
                groups.last_mut().expect("just pushed")
            }
        };
        let base = group.positions.len() as u32;
        group.positions.extend(local);
        group.faces.extend(
            tris.into_iter()
                .map(|t| t.iter().map(|i| i + base).collect()),
        );
    }
    Ok(groups)
}

/// Mesh + material built from a group.
struct Built {
    mesh: Mesh,
    material: Option<Material>,
}

fn build_groups(
    gmesh: &gltf::Mesh,
    doc: &gltf::Document,
    buffers: &[Vec<u8>],
    opts: &ImportOptions,
    warnings: &mut Vec<String>,
) -> IoResult<Vec<Built>> {
    let mut out = Vec::new();
    for g in collect_groups(gmesh, buffers, warnings)? {
        let mesh = build_mesh(g.positions, g.faces, opts.weld)?;
        if mesh.face_count() == 0 {
            warnings.push(format!(
                "mesh '{}': a primitive had no non-degenerate triangles",
                gmesh.name().unwrap_or("?")
            ));
            continue;
        }
        let material = g
            .material
            .and_then(|i| doc.materials().nth(i))
            .map(|m| convert_material(&m));
        out.push(Built { mesh, material });
    }
    Ok(out)
}

/// Parse `.gltf` / `.glb` bytes into a sub-scene.
pub(crate) fn import_gltf(bytes: &[u8], opts: &ImportOptions, name: &str) -> IoResult<Imported> {
    let gltf::Gltf { document, mut blob } =
        gltf::Gltf::from_slice(bytes).map_err(|e| IoError::parse(FORMAT, e.to_string()))?;
    let version = document.as_json().asset.version.as_str();
    if !version.starts_with("2.") && version != "2" {
        return Err(IoError::parse(
            FORMAT,
            format!("unsupported glTF version '{version}' (only 2.x is supported)"),
        ));
    }
    let buffers = load_buffers(&document, &mut blob, opts)?;
    let mut warnings = Vec::new();
    if document.animations().next().is_some() {
        warnings.push("animations are not imported".to_string());
    }
    if document.skins().next().is_some() {
        warnings.push("skins are not imported".to_string());
    }

    // Root nodes of the default scene (or all parentless nodes).
    let roots: Vec<gltf::Node> = match document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        Some(s) => s.nodes().collect(),
        None => {
            let children: HashSet<usize> = document
                .nodes()
                .flat_map(|n| n.children().map(|c| c.index()).collect::<Vec<_>>())
                .collect();
            document
                .nodes()
                .filter(|n| !children.contains(&n.index()))
                .collect()
        }
    };

    let mut scene = Scene::new();
    let mut mesh_cache: HashMap<usize, Vec<(Mesh, Option<Material>)>> = HashMap::new();
    let mut visited: HashSet<usize> = HashSet::new();
    let mut stack: Vec<(gltf::Node, Option<Uuid>)> =
        roots.into_iter().rev().map(|n| (n, None)).collect();
    while let Some((gnode, parent)) = stack.pop() {
        if !visited.insert(gnode.index()) {
            return Err(IoError::InvalidData(format!(
                "node {} is referenced more than once (the node graph must be a forest)",
                gnode.index()
            )));
        }
        let mesh_name = gnode.mesh().and_then(|m| m.name().map(str::to_string));
        let node_name = gnode
            .name()
            .map(str::to_string)
            .or_else(|| mesh_name.clone())
            .unwrap_or_else(|| format!("Node {}", gnode.index()));
        let mut node = Node::new(node_name.clone()).with_transform(node_transform(&gnode));
        node.visible = node_visible(&gnode);
        let mut extra: Vec<Built> = Vec::new();
        if let Some(gmesh) = gnode.mesh() {
            if let std::collections::hash_map::Entry::Vacant(e) = mesh_cache.entry(gmesh.index()) {
                let built = build_groups(&gmesh, &document, &buffers, opts, &mut warnings)?;
                e.insert(built.into_iter().map(|b| (b.mesh, b.material)).collect());
            }
            let mut parts = mesh_cache[&gmesh.index()].clone();
            if parts.len() == 1 {
                let (mesh, mat) = parts.remove(0);
                node.mesh = Some(mesh);
                node.material = mat;
            } else {
                // Several materials: the node becomes a group with one child per material.
                extra.extend(
                    parts
                        .into_iter()
                        .map(|(mesh, material)| Built { mesh, material }),
                );
            }
        }
        let id = scene.add_node(node, parent)?;
        for (k, b) in extra.into_iter().enumerate() {
            let label = b
                .material
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("part {}", k + 1));
            let mut child = Node::new(format!("{node_name} ({label})")).with_mesh(b.mesh);
            child.material = b.material;
            scene.add_node(child, Some(id))?;
        }
        let kids: Vec<gltf::Node> = gnode.children().collect();
        for c in kids.into_iter().rev() {
            stack.push((c, Some(id)));
        }
    }
    if scene.is_empty() {
        return Err(IoError::NothingToImport(
            "the glTF file contains no nodes".into(),
        ));
    }
    Ok(Imported::new(scene, name.to_string(), warnings))
}

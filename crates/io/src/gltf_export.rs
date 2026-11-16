//! glTF 2.0 / GLB export.

use std::collections::HashMap;

use base64::Engine;
use forge3d_core::mesh::Mesh;
use forge3d_core::scene::{Material, Scene};
use glam::Vec3;
use serde_json::{json, Map, Number, Value};
use uuid::Uuid;

use crate::error::{IoError, IoResult};
use crate::util::{
    crease_normals, decompose_trs, plan_export, pos_to_y_up, transform_to_y_up, PlannedNode,
};
use crate::{ExportBundle, ExportOptions, ExportedFile};

const COMPONENT_U16: u32 = 5123;
const COMPONENT_U32: u32 = 5125;
const COMPONENT_F32: u32 = 5126;
const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// Render a finite `f32` as the shortest JSON number that parses back to the
/// same `f32`.
pub(crate) fn num(v: f32) -> Value {
    let v = if v.is_finite() { v } else { 0.0 };
    v.to_string()
        .parse::<Number>()
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn nums(v: &[f32]) -> Value {
    Value::Array(v.iter().map(|&x| num(x)).collect())
}

#[derive(Default)]
struct BinBuilder {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl BinBuilder {
    fn pad4(&mut self) {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
    }

    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        self.pad4();
        let offset = self.data.len();
        self.data.extend_from_slice(bytes);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
            "target": target,
        }));
        self.views.len() - 1
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, ty: &str) -> usize {
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": ty,
        }));
        self.accessors.len() - 1
    }
}

fn material_key(m: &Material) -> Vec<u32> {
    let mut k: Vec<u32> = m.name.bytes().map(u32::from).collect();
    k.push(u32::MAX);
    k.extend(m.base_color.iter().map(|v| v.to_bits()));
    k.push(m.metallic.to_bits());
    k.push(m.roughness.to_bits());
    k.extend(m.emissive.iter().map(|v| v.to_bits()));
    k
}

/// Returns the material JSON and whether it needs `KHR_materials_emissive_strength`.
fn material_json(m: &Material) -> (Value, bool) {
    let mut obj = Map::new();
    obj.insert("name".into(), Value::String(m.name.clone()));
    obj.insert(
        "pbrMetallicRoughness".into(),
        json!({
            "baseColorFactor": nums(&m.base_color),
            "metallicFactor": num(m.metallic),
            "roughnessFactor": num(m.roughness),
        }),
    );
    let peak = m.emissive.iter().copied().fold(0.0_f32, f32::max);
    let mut needs_strength = false;
    if peak > 0.0 {
        if peak > 1.0 {
            let scaled = m.emissive.map(|c| c / peak);
            obj.insert("emissiveFactor".into(), nums(&scaled));
            obj.insert(
                "extensions".into(),
                json!({ "KHR_materials_emissive_strength": { "emissiveStrength": num(peak) } }),
            );
            needs_strength = true;
        } else {
            obj.insert("emissiveFactor".into(), nums(&m.emissive));
        }
    }
    if m.base_color[3] < 1.0 {
        obj.insert("alphaMode".into(), Value::String("BLEND".into()));
    }
    (Value::Object(obj), needs_strength)
}

/// Add the triangulated mesh to the binary buffer; returns the glTF mesh JSON
/// or `None` when the mesh has no triangles.
fn mesh_json(
    mesh: &Mesh,
    crease_deg: f32,
    name: &str,
    material: Option<usize>,
    with_normals: bool,
    bin: &mut BinBuilder,
) -> Option<Value> {
    let mut positions: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    if with_normals {
        let cn = crease_normals(mesh, crease_deg);
        for (v, n) in &cn.vertices {
            positions.push(pos_to_y_up(mesh.position(forge3d_core::mesh::VertexId(*v))));
            normals.push(pos_to_y_up(*n));
        }
        let mut cached_face = None;
        let mut face_verts: Vec<u32> = Vec::new();
        for (f, tri) in mesh.triangulate_with_faces() {
            if cached_face != Some(f) {
                face_verts = mesh.face_vertices(f).map(|v| v.0).collect();
                cached_face = Some(f);
            }
            for v in tri {
                let corner = face_verts
                    .iter()
                    .position(|&x| x == v)
                    .expect("triangle corner belongs to its face");
                indices.push(cn.faces[f.index()][corner]);
            }
        }
    } else {
        positions.extend(mesh.positions().into_iter().map(pos_to_y_up));
        for t in mesh.triangulate() {
            indices.extend_from_slice(&t);
        }
    }
    if indices.is_empty() || positions.is_empty() {
        return None;
    }

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut pbytes = Vec::with_capacity(positions.len() * 12);
    for p in &positions {
        min = min.min(*p);
        max = max.max(*p);
        for c in p.to_array() {
            pbytes.extend_from_slice(&c.to_le_bytes());
        }
    }
    let pview = bin.view(&pbytes, TARGET_ARRAY_BUFFER);
    let pacc = bin.accessor(pview, COMPONENT_F32, positions.len(), "VEC3");
    bin.accessors[pacc]["min"] = nums(&min.to_array());
    bin.accessors[pacc]["max"] = nums(&max.to_array());

    let mut attributes = Map::new();
    attributes.insert("POSITION".into(), json!(pacc));
    if with_normals {
        let mut nbytes = Vec::with_capacity(normals.len() * 12);
        for n in &normals {
            for c in n.to_array() {
                nbytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        let nview = bin.view(&nbytes, TARGET_ARRAY_BUFFER);
        let nacc = bin.accessor(nview, COMPONENT_F32, normals.len(), "VEC3");
        attributes.insert("NORMAL".into(), json!(nacc));
    }

    let (component, ibytes) = if positions.len() <= u16::MAX as usize {
        let mut b = Vec::with_capacity(indices.len() * 2);
        for &i in &indices {
            b.extend_from_slice(&(i as u16).to_le_bytes());
        }
        (COMPONENT_U16, b)
    } else {
        let mut b = Vec::with_capacity(indices.len() * 4);
        for &i in &indices {
            b.extend_from_slice(&i.to_le_bytes());
        }
        (COMPONENT_U32, b)
    };
    let iview = bin.view(&ibytes, TARGET_ELEMENT_ARRAY_BUFFER);
    let iacc = bin.accessor(iview, component, indices.len(), "SCALAR");

    let mut primitive = Map::new();
    primitive.insert("attributes".into(), Value::Object(attributes));
    primitive.insert("indices".into(), json!(iacc));
    primitive.insert("mode".into(), json!(4));
    if let Some(m) = material {
        primitive.insert("material".into(), json!(m));
    }
    Some(json!({ "name": name, "primitives": [Value::Object(primitive)] }))
}

fn node_json(
    planned: &PlannedNode,
    scene: &Scene,
    mesh_index: Option<usize>,
    children: &[usize],
) -> IoResult<Value> {
    let node = scene.node(planned.id)?;
    let mut obj = Map::new();
    obj.insert("name".into(), Value::String(node.name.clone()));
    let m = transform_to_y_up(&planned.transform);
    match decompose_trs(&m) {
        Some((t, r, s)) => {
            if t != Vec3::ZERO {
                obj.insert("translation".into(), nums(&t.to_array()));
            }
            if r.xyz().length_squared() > 1e-14 {
                let r = if r.w < 0.0 { -r } else { r };
                obj.insert("rotation".into(), nums(&r.to_array()));
            }
            if (s - Vec3::ONE).abs().max_element() > 0.0 {
                obj.insert("scale".into(), nums(&s.to_array()));
            }
        }
        None => {
            obj.insert("matrix".into(), nums(&glam::Mat4::from(m).to_cols_array()));
        }
    }
    if let Some(mi) = mesh_index {
        obj.insert("mesh".into(), json!(mi));
    }
    if !children.is_empty() {
        obj.insert("children".into(), json!(children));
    }
    if !node.visible {
        obj.insert("extras".into(), json!({ "forge3d": { "visible": false } }));
    }
    Ok(Value::Object(obj))
}

/// Export the scene as `.gltf` (embedded or external buffer) or `.glb`.
pub(crate) fn export_gltf(
    scene: &Scene,
    opts: &ExportOptions,
    glb: bool,
    stem: &str,
) -> IoResult<ExportBundle> {
    let plan = plan_export(scene, opts)?;
    let index_of: HashMap<Uuid, usize> = plan.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

    let mut bin = BinBuilder::default();
    let mut materials: Vec<Value> = Vec::new();
    let mut material_ids: HashMap<Vec<u32>, usize> = HashMap::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut uses_emissive_strength = false;

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); plan.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        match p.parent {
            Some(parent) => children[index_of[&parent]].push(i),
            None => roots.push(i),
        }
    }

    let mut nodes: Vec<Value> = Vec::with_capacity(plan.len());
    for (i, p) in plan.iter().enumerate() {
        let node = scene.node(p.id)?;
        let mut mesh_index = None;
        if let Some(mesh) = node.evaluated_mesh() {
            let material = node.material.as_ref().map(|m| {
                *material_ids.entry(material_key(m)).or_insert_with(|| {
                    let (j, needs) = material_json(m);
                    uses_emissive_strength |= needs;
                    materials.push(j);
                    materials.len() - 1
                })
            });
            if let Some(mj) = mesh_json(
                &mesh,
                node.shading_crease_deg(),
                &node.name,
                material,
                opts.normals,
                &mut bin,
            ) {
                meshes.push(mj);
                mesh_index = Some(meshes.len() - 1);
            }
        }
        nodes.push(node_json(p, scene, mesh_index, &children[i])?);
    }
    bin.pad4();

    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({ "version": "2.0", "generator": "forge3d" }),
    );
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{ "name": stem, "nodes": roots }]));
    if !nodes.is_empty() {
        root.insert("nodes".into(), Value::Array(nodes));
    }
    if !meshes.is_empty() {
        root.insert("meshes".into(), Value::Array(meshes));
    }
    if !materials.is_empty() {
        root.insert("materials".into(), Value::Array(materials));
    }
    if uses_emissive_strength {
        root.insert(
            "extensionsUsed".into(),
            json!(["KHR_materials_emissive_strength"]),
        );
    }
    let has_bin = !bin.data.is_empty();
    let mut files = Vec::new();
    if has_bin {
        let uri = if glb {
            None
        } else if opts.embed_buffers {
            Some(format!(
                "data:application/octet-stream;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&bin.data)
            ))
        } else {
            Some(format!("{stem}.bin"))
        };
        let mut buffer = Map::new();
        buffer.insert("byteLength".into(), json!(bin.data.len()));
        if let Some(u) = &uri {
            buffer.insert("uri".into(), Value::String(u.clone()));
        }
        root.insert("buffers".into(), json!([Value::Object(buffer)]));
        root.insert("bufferViews".into(), Value::Array(bin.views.clone()));
        root.insert("accessors".into(), Value::Array(bin.accessors.clone()));
    }
    let root = Value::Object(root);
    let json_bytes = if opts.pretty_json && !glb {
        serde_json::to_vec_pretty(&root)
    } else {
        serde_json::to_vec(&root)
    }
    .map_err(|e| IoError::InvalidData(format!("cannot serialize glTF JSON: {e}")))?;

    if glb {
        files.push(ExportedFile {
            name: format!("{stem}.glb"),
            bytes: pack_glb(json_bytes, has_bin.then_some(bin.data.as_slice())),
        });
    } else {
        files.push(ExportedFile {
            name: format!("{stem}.gltf"),
            bytes: json_bytes,
        });
        if has_bin && !opts.embed_buffers {
            files.push(ExportedFile {
                name: format!("{stem}.bin"),
                bytes: bin.data,
            });
        }
    }
    Ok(ExportBundle { files })
}

/// Assemble a GLB container: 12 byte header, JSON chunk padded with spaces,
/// optional BIN chunk padded with zeros (both 4-byte aligned).
fn pack_glb(mut json: Vec<u8>, bin: Option<&[u8]>) -> Vec<u8> {
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let bin_len = bin.map_or(0, |b| b.len().div_ceil(4) * 4);
    let mut total = 12 + 8 + json.len();
    if bin.is_some() {
        total += 8 + bin_len;
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json);
    if let Some(b) = bin {
        out.extend_from_slice(&(bin_len as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(b);
        out.resize(total, 0);
    }
    out
}

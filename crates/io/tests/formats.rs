mod common;

use common::*;
use forge3d_core::prelude::*;
use forge3d_core::scene::Node;
use forge3d_io::*;
use glam::{Affine3A, Vec3};

fn opts() -> ExportOptions {
    ExportOptions::default()
}

// ---------------------------------------------------------------------------
// glTF / GLB
// ---------------------------------------------------------------------------

#[test]
fn gltf_embedded_round_trip() {
    let doc = sample_doc();
    let b = export_bytes(&doc.scene, ExportFormat::Gltf, &opts()).unwrap();
    assert_eq!(b.files.len(), 1);
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Gltf,
        "x",
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    assert_scenes_match(&doc.scene, &imp.scene, true);
}

#[test]
fn glb_round_trip_and_validity() {
    let doc = sample_doc();
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &opts()).unwrap();
    let bytes = &b.primary().bytes;

    // Container structure.
    assert_eq!(&bytes[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
        bytes.len()
    );
    assert_eq!(bytes.len() % 4, 0);
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    assert_eq!(json_len % 4, 0);
    assert_eq!(&bytes[16..20], b"JSON");
    let bin_at = 20 + json_len;
    assert_eq!(&bytes[bin_at + 4..bin_at + 8], b"BIN\0");

    // The gltf crate validates the document on parse.
    let g = gltf::Gltf::from_slice(bytes).expect("valid glb");
    assert!(g.blob.is_some());
    let doc_g = g.document;
    assert_eq!(doc_g.nodes().len(), doc.scene.len());
    assert_eq!(doc_g.materials().len(), 3);
    for mesh in doc_g.meshes() {
        for prim in mesh.primitives() {
            let pos = prim.get(&gltf::Semantic::Positions).unwrap();
            assert!(
                pos.min().is_some() && pos.max().is_some(),
                "POSITION min/max"
            );
            assert!(prim.get(&gltf::Semantic::Normals).is_some());
        }
    }
    for view in doc_g.views() {
        assert_eq!(view.offset() % 4, 0, "bufferView alignment");
        assert!(view.target().is_some());
    }

    let imp = import_bytes(bytes, ImportFormat::Glb, "x", &ImportOptions::default()).unwrap();
    assert_scenes_match(&doc.scene, &imp.scene, true);
}

#[test]
fn gltf_without_normals_and_ascii_json_pretty() {
    let doc = sample_doc();
    let o = ExportOptions {
        normals: false,
        pretty_json: true,
        ..opts()
    };
    let b = export_bytes(&doc.scene, ExportFormat::Gltf, &o).unwrap();
    let g = gltf::Gltf::from_slice(&b.primary().bytes).unwrap();
    assert!(g.document.meshes().all(|m| m
        .primitives()
        .all(|p| p.get(&gltf::Semantic::Normals).is_none())));
}

#[test]
fn gltf_normals_keep_cube_edges_hard() {
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &opts()).unwrap();
    let g = gltf::Gltf::from_slice(&b.primary().bytes).unwrap();
    let prim = g
        .document
        .meshes()
        .next()
        .unwrap()
        .primitives()
        .next()
        .unwrap();
    // 6 faces x 4 corners: every corner has its own face normal.
    assert_eq!(prim.get(&gltf::Semantic::Positions).unwrap().count(), 24);
    // ... but importing welds it back to the 8 cube corners.
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "c",
        &ImportOptions::default(),
    )
    .unwrap();
    let mesh = imp.scene.nodes().next().unwrap().mesh.as_ref().unwrap();
    assert_eq!(mesh.vertex_count(), 8);
    assert_eq!(mesh.face_count(), 12);
    assert!(mesh.is_closed());
}

#[test]
fn gltf_external_bin_files() {
    let dir = tempfile::tempdir().unwrap();
    let doc = sample_doc();
    let o = ExportOptions {
        embed_buffers: false,
        ..opts()
    };
    let written = export_scene(&doc.scene, dir.path().join("model.gltf"), &o).unwrap();
    assert_eq!(written.len(), 2);
    assert!(dir.path().join("model.bin").exists());
    let imp = import_file(dir.path().join("model.gltf"), &ImportOptions::default()).unwrap();
    assert_scenes_match(&doc.scene, &imp.scene, true);

    // Without the sidecar the error is structured.
    std::fs::remove_file(dir.path().join("model.bin")).unwrap();
    let e = import_file(dir.path().join("model.gltf"), &ImportOptions::default()).unwrap_err();
    assert_eq!(e.code(), "file_not_found");

    // In-memory resources work too.
    let b = export_bytes(&doc.scene, ExportFormat::Gltf, &o).unwrap();
    let mut io = ImportOptions::default();
    io.resources
        .insert("scene.bin".into(), b.files[1].bytes.clone());
    let imp = import_bytes(&b.files[0].bytes, ImportFormat::Gltf, "m", &io).unwrap();
    assert_scenes_match(&doc.scene, &imp.scene, true);
}

#[test]
fn gltf_import_converts_y_up_and_reads_trs() {
    // One triangle, node translated by (1,2,3) in Y-up space.
    let positions: Vec<u8> = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&positions)
    };
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
        "nodes":[{{"name":"Tri","mesh":0,"translation":[1,2,3]}}],
        "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
        "buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{b64}"}}],
        "bufferViews":[{{"buffer":0,"byteLength":36}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
                       "min":[0,0,0],"max":[1,1,0]}}]}}"#
    );
    let imp = import_bytes(
        json.as_bytes(),
        ImportFormat::Gltf,
        "t",
        &ImportOptions::default(),
    )
    .unwrap();
    let n = imp.scene.nodes().next().unwrap();
    assert_eq!(n.name, "Tri");
    // Y-up (1,2,3) -> Z-up (1,-3,2)
    assert!(n
        .transform
        .translation
        .abs_diff_eq(Vec3::new(1.0, -3.0, 2.0).into(), 1e-6));
    // Y-up (0,1,0) -> Z-up (0,0,1)
    let ps = n.mesh.as_ref().unwrap().positions();
    assert!(ps.iter().any(|p| p.abs_diff_eq(Vec3::Z, 1e-6)));
    assert!(n.material.is_none());
}

#[test]
fn gltf_import_strips_and_multiple_materials() {
    let verts: Vec<u8> = [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
    ]
    .iter()
    .flatten()
    .flat_map(|f| f.to_le_bytes())
    .collect();
    let idx: Vec<u8> = [0u16, 1, 2, 3]
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let mut all = verts.clone();
    all.extend_from_slice(&idx);
    let b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&all)
    };
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
        "nodes":[{{"name":"Two","mesh":0,"matrix":[1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,5,1]}}],
        "materials":[{{"name":"A","pbrMetallicRoughness":{{"baseColorFactor":[1,0,0,1]}}}},
                     {{"name":"B","pbrMetallicRoughness":{{"metallicFactor":1}}}}],
        "meshes":[{{"primitives":[
            {{"attributes":{{"POSITION":0}},"indices":1,"mode":5,"material":0}},
            {{"attributes":{{"POSITION":0}},"indices":1,"mode":5,"material":1}}]}}],
        "buffers":[{{"byteLength":{len},"uri":"data:application/octet-stream;base64,{b64}"}}],
        "bufferViews":[{{"buffer":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":8}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
                     {{"bufferView":1,"componentType":5123,"count":4,"type":"SCALAR"}}]}}"#,
        len = all.len()
    );
    let imp = import_bytes(
        json.as_bytes(),
        ImportFormat::Gltf,
        "t",
        &ImportOptions::default(),
    )
    .unwrap();
    // No `scenes`: all parentless nodes are roots. Two materials -> parent + 2 children.
    assert_eq!(imp.node_count(), 3);
    let root = imp.scene.node(imp.roots()[0]).unwrap();
    assert!(root.mesh.is_none());
    assert_eq!(root.children().len(), 2);
    // matrix translation (0,0,5) in Y-up -> (0,-5,0).
    assert!(root
        .transform
        .translation
        .abs_diff_eq(Vec3::new(0.0, -5.0, 0.0).into(), 1e-6));
    let kids: Vec<&Node> = root
        .children()
        .iter()
        .map(|c| imp.scene.node(*c).unwrap())
        .collect();
    assert_eq!(kids[0].material.as_ref().unwrap().name, "A");
    assert_eq!(kids[1].material.as_ref().unwrap().metallic, 1.0);
    for k in kids {
        assert_eq!(k.mesh.as_ref().unwrap().face_count(), 2); // strip of 4 -> 2 triangles
        assert_eq!(k.mesh.as_ref().unwrap().vertex_count(), 4);
    }
}

#[test]
fn gltf_emissive_above_one_round_trips() {
    let doc = sample_doc();
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &opts()).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "x",
        &ImportOptions::default(),
    )
    .unwrap();
    let lamp = imp.scene.find_by_name("Lamp").unwrap();
    let e = imp
        .scene
        .node(lamp)
        .unwrap()
        .material
        .as_ref()
        .unwrap()
        .emissive;
    assert!((e[0] - 4.0).abs() < 1e-5 && (e[1] - 2.0).abs() < 1e-5 && (e[2] - 1.0).abs() < 1e-5);
}

#[test]
fn gltf_export_subset_and_hidden() {
    let doc = sample_doc();
    let base = doc.scene.find_by_name("Base").unwrap();
    let o = ExportOptions {
        nodes: Some(vec![base]),
        ..opts()
    };
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &o).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "x",
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(imp.node_count(), 2); // Base + Ball

    let o = ExportOptions {
        include_hidden: false,
        ..opts()
    };
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &o).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "x",
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(imp.scene.find_by_name("Floor").is_none());

    // A nested node exported on its own keeps its world transform.
    let ball = doc.scene.find_by_name("Ball").unwrap();
    let o = ExportOptions {
        nodes: Some(vec![ball]),
        ..opts()
    };
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &o).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "x",
        &ImportOptions::default(),
    )
    .unwrap();
    let n = imp.scene.nodes().next().unwrap();
    assert_affine_eq(
        &n.transform,
        &doc.scene.world_transform(ball).unwrap(),
        "ball world",
    );
}

#[test]
fn empty_scene_exports_valid_gltf() {
    let b = export_bytes(&Scene::new(), ExportFormat::Glb, &opts()).unwrap();
    gltf::Gltf::from_slice(&b.primary().bytes).unwrap();
    let e = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "e",
        &ImportOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code(), "nothing_to_import");
}

// ---------------------------------------------------------------------------
// OBJ
// ---------------------------------------------------------------------------

/// Meshes in world space by node name.
fn world_meshes(scene: &Scene) -> Vec<(String, Vec<Vec3>, usize)> {
    let mut v: Vec<_> = scene
        .traverse()
        .into_iter()
        .filter_map(|id| {
            let n = scene.node(id).unwrap();
            let m = n.mesh.as_ref()?;
            let w = scene.world_transform(id).unwrap();
            Some((
                n.name.clone(),
                m.positions()
                    .into_iter()
                    .map(|p| w.transform_point3(p))
                    .collect(),
                m.face_count(),
            ))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[test]
fn obj_round_trip_preserves_quads_names_and_materials() {
    let doc = sample_doc();
    let b = export_bytes(&doc.scene, ExportFormat::Obj, &opts()).unwrap();
    assert_eq!(b.files.len(), 2);
    let text = String::from_utf8(b.files[0].bytes.clone()).unwrap();
    assert!(text.contains("mtllib scene.mtl"));
    assert!(text.contains("\no Base\n"));
    assert!(text
        .lines()
        .any(|l| l.starts_with("f ") && l.split_whitespace().count() == 5));

    let mut io = ImportOptions::default();
    io.resources
        .insert("scene.mtl".into(), b.files[1].bytes.clone());
    let imp = import_bytes(&b.files[0].bytes, ImportFormat::Obj, "x", &io).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);

    let (orig, back) = (world_meshes(&doc.scene), world_meshes(&imp.scene));
    assert_eq!(orig.len(), back.len());
    for (o, i) in orig.iter().zip(&back) {
        assert_eq!(o.0, i.0, "object name");
        assert_eq!(o.2, i.2, "face count of {}", o.0);
        assert_positions_eq(o.1.clone(), i.1.clone(), &o.0);
    }
    for name in ["Base", "Ball", "Lamp"] {
        let a = doc
            .scene
            .node(doc.scene.find_by_name(name).unwrap())
            .unwrap();
        let b = imp
            .scene
            .node(imp.scene.find_by_name(name).unwrap())
            .unwrap();
        assert_material_eq(&a.material, &b.material, name);
    }
    // Cube stays 6 quads.
    let base = imp
        .scene
        .node(imp.scene.find_by_name("Base").unwrap())
        .unwrap();
    let m = base.mesh.as_ref().unwrap();
    assert_eq!((m.vertex_count(), m.face_count()), (8, 6));
    assert!(m.face_ids().all(|f| m.face_len(f) == 4));
}

#[test]
fn obj_z_up_option_round_trips() {
    let doc = sample_doc();
    let o = ExportOptions {
        obj_up_axis: UpAxis::Z,
        normals: false,
        ..opts()
    };
    let b = export_bytes(&doc.scene, ExportFormat::Obj, &o).unwrap();
    let io = ImportOptions {
        obj_up_axis: UpAxis::Z,
        ..ImportOptions::default()
    };
    let imp = import_bytes(&b.files[0].bytes, ImportFormat::Obj, "x", &io).unwrap();
    for (o, i) in world_meshes(&doc.scene)
        .iter()
        .zip(&world_meshes(&imp.scene))
    {
        assert_positions_eq(o.1.clone(), i.1.clone(), &o.0);
    }
}

#[test]
fn obj_import_forms_negative_indices_and_groups() {
    let src = "\
# comment
mtllib missing.mtl
o Quad
v 0 0 0
v 1 0 0
v 1 0 -1
v 0 0 -1
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vn 0 1 0
f 1/1/1 2/2/1 3/3/1 4/4/1
o Tri
v 5 0 0
v 6 0 0
v 5 1 0
f -3 -2 -1
v 6 1 0
g Strip
f 5//1 7//1 8//1
";
    let imp = import_bytes(
        src.as_bytes(),
        ImportFormat::Obj,
        "file",
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(imp.warnings.len(), 1); // missing.mtl
    let names: Vec<String> = imp
        .scene
        .traverse()
        .iter()
        .map(|i| imp.scene.node(*i).unwrap().name.clone())
        .collect();
    // `g` is part of the object `Tri` because the file uses `o`.
    assert_eq!(names, ["Quad", "Tri"]);
    let quad = imp
        .scene
        .node(imp.roots()[0])
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    assert_eq!((quad.vertex_count(), quad.face_count()), (4, 1));
    assert_eq!(quad.face_len(quad.face_ids().next().unwrap()), 4);
    // Y-up (1,0,-1) -> Z-up (1,1,0)
    assert!(quad
        .positions()
        .iter()
        .any(|p| p.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-6)));
    let tri = imp
        .scene
        .node(imp.roots()[1])
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    assert_eq!(tri.face_count(), 2);
}

#[test]
fn obj_g_groups_become_objects_without_o() {
    let src = "g A\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\ng B\nv 0 0 1\nv 1 0 1\nv 0 1 1\nf 4 5 6\n";
    let imp = import_bytes(
        src.as_bytes(),
        ImportFormat::Obj,
        "f",
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(imp.node_count(), 2);
}

#[test]
fn obj_usemtl_splits_object_by_material_and_parses_mtl() {
    let obj = "mtllib m.mtl\no Thing\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\n\
               usemtl Red\nf 1 2 3\nusemtl Green\nf 1 3 4\nf 1 4 5\n";
    let mtl = "newmtl Red\nKd 1 0 0\nd 0.5\nPm 0.3\nPr 0.7\nKe 0.1 0.2 0.3\n\
               newmtl Green\nKd 0 1 0\nNs 250\n";
    let mut io = ImportOptions::default();
    io.resources.insert("m.mtl".into(), mtl.as_bytes().to_vec());
    let imp = import_bytes(obj.as_bytes(), ImportFormat::Obj, "f", &io).unwrap();
    assert_eq!(imp.node_count(), 3);
    let parent = imp.scene.node(imp.roots()[0]).unwrap();
    assert_eq!(parent.name, "Thing");
    let red = imp
        .scene
        .node(parent.children()[0])
        .unwrap()
        .material
        .clone()
        .unwrap();
    assert_eq!(red.base_color, [1.0, 0.0, 0.0, 0.5]);
    assert_eq!((red.metallic, red.roughness), (0.3, 0.7));
    assert_eq!(red.emissive, [0.1, 0.2, 0.3]);
    let green = imp
        .scene
        .node(parent.children()[1])
        .unwrap()
        .material
        .clone()
        .unwrap();
    assert!((green.roughness - 0.5).abs() < 1e-6); // Ns 250 -> 1 - sqrt(.25)
}

#[test]
fn obj_errors_are_structured() {
    let cases = [
        ("v 0 0 0\nf 1 2\n", "parse_error"),
        ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n", "parse_error"),
        ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n", "parse_error"),
        ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 x\n", "parse_error"),
        ("v 0 0\n", "parse_error"),
        ("v 0 0 nan\n", "parse_error"),
        ("v 0 0 0\n", "nothing_to_import"),
        ("", "nothing_to_import"),
    ];
    for (src, code) in cases {
        let e = import_bytes(
            src.as_bytes(),
            ImportFormat::Obj,
            "f",
            &ImportOptions::default(),
        )
        .unwrap_err();
        assert_eq!(e.code(), code, "{src:?} -> {e}");
        assert!(!e.hint().is_empty());
    }
    let e = import_bytes(
        b"v 0 0 0\nf 1 2\n",
        ImportFormat::Obj,
        "f",
        &ImportOptions::default(),
    )
    .unwrap_err();
    assert!(e.to_string().contains("line 2"), "{e}");
}

#[test]
fn obj_export_without_meshes_fails() {
    let mut s = Scene::new();
    s.add_node(Node::new("Empty"), None).unwrap();
    let e = export_bytes(&s, ExportFormat::Obj, &opts()).unwrap_err();
    assert_eq!(e.code(), "nothing_to_export");
}

// ---------------------------------------------------------------------------
// STL
// ---------------------------------------------------------------------------

fn cube_scene() -> Scene {
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    doc.scene
}

#[test]
fn stl_binary_round_trip_welds() {
    let scene = cube_scene();
    let b = export_bytes(&scene, ExportFormat::Stl, &opts()).unwrap();
    let bytes = &b.primary().bytes;
    assert_eq!(bytes.len(), 84 + 12 * 50);
    assert!(!bytes.starts_with(b"solid"));
    let imp = import_bytes(bytes, ImportFormat::Stl, "cube", &ImportOptions::default()).unwrap();
    let node = imp.scene.nodes().next().unwrap();
    assert_eq!(node.name, "cube");
    let m = node.mesh.as_ref().unwrap();
    assert_eq!((m.vertex_count(), m.face_count()), (8, 12));
    assert!(m.is_closed());
    assert!((m.signed_volume() - 8.0).abs() < 1e-3);
}

#[test]
fn stl_ascii_round_trip_and_no_weld_option() {
    let scene = cube_scene();
    let o = ExportOptions {
        stl_ascii: true,
        ..opts()
    };
    let b = export_bytes(&scene, ExportFormat::Stl, &o).unwrap();
    let text = String::from_utf8(b.primary().bytes.clone()).unwrap();
    assert!(text.starts_with("solid scene"));
    assert_eq!(text.matches("facet normal").count(), 12);
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Stl,
        "c",
        &ImportOptions::default(),
    )
    .unwrap();
    let m = imp.scene.nodes().next().unwrap().mesh.as_ref().unwrap();
    assert_eq!((m.vertex_count(), m.face_count()), (8, 12));
    assert_eq!(imp.scene.nodes().next().unwrap().name, "scene");

    let io = ImportOptions {
        weld: false,
        ..ImportOptions::default()
    };
    let imp = import_bytes(&b.primary().bytes, ImportFormat::Stl, "c", &io).unwrap();
    assert_eq!(
        imp.scene
            .nodes()
            .next()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .vertex_count(),
        36
    );
}

#[test]
fn stl_ascii_multiple_solids_and_binary_with_solid_header() {
    let ascii =
        "solid one\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
                 endloop\nendfacet\nendsolid one\nsolid two\nfacet normal 0 0 1\nouter loop\n\
                 vertex 0 0 1\nvertex 1 0 1\nvertex 0 1 1\nendloop\nendfacet\nendsolid two\n";
    let imp = import_bytes(
        ascii.as_bytes(),
        ImportFormat::Stl,
        "f",
        &ImportOptions::default(),
    )
    .unwrap();
    let names: Vec<_> = imp
        .scene
        .traverse()
        .iter()
        .map(|i| imp.scene.node(*i).unwrap().name.clone())
        .collect();
    assert_eq!(names, ["one", "two"]);

    // Binary file whose header starts with "solid" is still detected as binary.
    let scene = cube_scene();
    let mut bytes = export_bytes(&scene, ExportFormat::Stl, &opts())
        .unwrap()
        .files
        .remove(0)
        .bytes;
    bytes[..5].copy_from_slice(b"solid");
    let imp = import_bytes(&bytes, ImportFormat::Stl, "f", &ImportOptions::default()).unwrap();
    assert_eq!(
        imp.scene
            .nodes()
            .next()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .face_count(),
        12
    );
}

#[test]
fn stl_mirrored_node_keeps_outward_normals() {
    let mut scene = cube_scene();
    let id = scene.roots()[0];
    scene.node_mut(id).unwrap().transform = Affine3A::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let b = export_bytes(&scene, ExportFormat::Stl, &opts()).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Stl,
        "f",
        &ImportOptions::default(),
    )
    .unwrap();
    let m = imp.scene.nodes().next().unwrap().mesh.as_ref().unwrap();
    assert!((m.signed_volume() - 8.0).abs() < 1e-3);
}

#[test]
fn stl_malformed_input() {
    let imp = |b: &[u8]| import_bytes(b, ImportFormat::Stl, "f", &ImportOptions::default());
    // Truncated binary.
    let scene = cube_scene();
    let good = export_bytes(&scene, ExportFormat::Stl, &opts())
        .unwrap()
        .files
        .remove(0)
        .bytes;
    assert_eq!(
        imp(&good[..good.len() - 10]).unwrap_err().code(),
        "parse_error"
    );
    assert_eq!(imp(&good[..40]).unwrap_err().code(), "parse_error");
    assert_eq!(imp(b"").unwrap_err().code(), "parse_error");
    // ASCII with a broken facet.
    let bad =
        "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nendloop\nendfacet\n";
    assert_eq!(imp(bad.as_bytes()).unwrap_err().code(), "parse_error");
    let bad = "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 a\n";
    assert_eq!(imp(bad.as_bytes()).unwrap_err().code(), "parse_error");
    // No triangles.
    assert_eq!(
        imp(b"solid x\nendsolid x\n").unwrap_err().code(),
        "nothing_to_import"
    );
    // Binary with NaN coordinates.
    let mut nan = good.clone();
    nan[96..100].copy_from_slice(&f32::NAN.to_le_bytes());
    assert_eq!(imp(&nan).unwrap_err().code(), "invalid_data");
}

// ---------------------------------------------------------------------------
// .f3d
// ---------------------------------------------------------------------------

#[test]
fn f3d_round_trip_equals_state() {
    let doc = sample_doc();
    let bytes = project_to_bytes(&doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains(r#""format":"forge3d-project""#));
    assert!(text.contains(r#""version":1"#));
    let back = project_from_bytes(&bytes).unwrap();
    assert_eq!(back.state(), doc.state());
    assert!(!back.can_undo());
}

#[test]
fn f3d_save_load_file_is_atomic_and_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.f3d");
    let doc = sample_doc();
    save_project(&doc, &path).unwrap();
    // Overwrite with a different document: the file is replaced, no temp files remain.
    let mut doc2 = sample_doc();
    doc2.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    save_project(&doc2, &path).unwrap();
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(load_project(&path).unwrap().state(), doc2.state());
}

#[test]
fn f3d_via_export_and_import_api() {
    let dir = tempfile::tempdir().unwrap();
    let doc = sample_doc();
    let path = dir.path().join("scene.f3d");
    export_scene(&doc.scene, &path, &opts()).unwrap();
    let imp = import_file(&path, &ImportOptions::default()).unwrap();
    assert_eq!(imp.node_count(), doc.scene.len());
    // Fresh ids so it can be merged into the same document.
    assert!(imp.scene.nodes().all(|n| !doc.scene.contains(n.id())));
    assert_scenes_match(&doc.scene, &imp.scene, false);
}

#[test]
fn f3d_rejects_bad_versions_and_magic() {
    let doc = sample_doc();
    let good: serde_json::Value = serde_json::from_slice(&project_to_bytes(&doc).unwrap()).unwrap();
    let with = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut v = good.clone();
        f(&mut v);
        project_from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err()
    };
    for bad in [
        serde_json::json!(2),
        serde_json::json!(999),
        serde_json::json!(0),
        serde_json::json!("1"),
        serde_json::json!(1.5),
        serde_json::json!(-1),
    ] {
        let e = with(&|v| v["version"] = bad.clone());
        assert_eq!(e.code(), "version_mismatch", "{bad}");
    }
    let e = with(&|v| v["version"] = serde_json::json!(2));
    assert!(
        e.to_string().contains("unsupported project version 2"),
        "{e}"
    );
    assert!(e.hint().contains("newer"));
    assert_eq!(
        with(&|v| v["format"] = "other".into()).code(),
        "invalid_project"
    );
    assert_eq!(
        with(&|v| {
            v.as_object_mut().unwrap().remove("format");
        })
        .code(),
        "invalid_project"
    );
    assert_eq!(
        with(&|v| {
            v.as_object_mut().unwrap().remove("version");
        })
        .code(),
        "invalid_project"
    );
    assert_eq!(
        with(&|v| {
            v.as_object_mut().unwrap().remove("state");
        })
        .code(),
        "invalid_project"
    );
    assert_eq!(
        with(&|v| v["state"] = serde_json::json!({"scene": 5})).code(),
        "parse_error"
    );
    assert_eq!(
        project_from_bytes(b"[1,2]").unwrap_err().code(),
        "invalid_project"
    );
    assert_eq!(
        project_from_bytes(b"not json").unwrap_err().code(),
        "parse_error"
    );
    assert_eq!(project_from_bytes(b"").unwrap_err().code(), "parse_error");
}

#[test]
fn f3d_rejects_corrupt_scene() {
    let doc = sample_doc();
    let mut v: serde_json::Value =
        serde_json::from_slice(&project_to_bytes(&doc).unwrap()).unwrap();
    // A root that does not exist.
    v["state"]["scene"]["roots"] = serde_json::json!(["00000000-0000-0000-0000-000000000001"]);
    let e = project_from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert_eq!(e.code(), "parse_error");
}

#[test]
fn load_missing_file() {
    let e = load_project("/definitely/not/here.f3d").unwrap_err();
    assert_eq!(e.code(), "file_not_found");
    let e = import_file("/definitely/not/here.obj", &ImportOptions::default()).unwrap_err();
    assert_eq!(e.code(), "file_not_found");
}

// ---------------------------------------------------------------------------
// Unified API
// ---------------------------------------------------------------------------

#[test]
fn format_detection() {
    for (p, f) in [
        ("a.gltf", ExportFormat::Gltf),
        ("a.GLB", ExportFormat::Glb),
        ("dir/a.obj", ExportFormat::Obj),
        ("a.b.STL", ExportFormat::Stl),
        ("a.f3d", ExportFormat::F3d),
    ] {
        assert_eq!(ExportFormat::from_path(p.as_ref()).unwrap(), f);
        assert_eq!(f.extension(), p.rsplit('.').next().unwrap().to_lowercase());
    }
    assert_eq!(
        ImportFormat::from_path("x.glb".as_ref()).unwrap(),
        ImportFormat::Glb
    );
    for p in ["a.fbx", "noext", "a."] {
        let e = ExportFormat::from_path(p.as_ref()).unwrap_err();
        assert_eq!(e.code(), "unsupported_format");
        assert_eq!(
            import_file(p, &ImportOptions::default())
                .unwrap_err()
                .code(),
            "unsupported_format"
        );
        assert_eq!(
            export_scene(&Scene::new(), p, &opts()).unwrap_err().code(),
            "unsupported_format"
        );
    }
    let r = IoError::UnsupportedFormat("x".into()).report();
    assert_eq!(r.code, "unsupported_format");
    assert!(!r.hint.is_empty());
}

#[test]
fn export_scene_writes_every_format_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let doc = sample_doc();
    for ext in ["gltf", "glb", "obj", "stl", "f3d"] {
        let p = dir.path().join(format!("m.{ext}"));
        let written = export_scene(&doc.scene, &p, &opts()).unwrap();
        assert_eq!(written[0], p);
        let imp = import_file(&p, &ImportOptions::default()).unwrap();
        assert!(imp.mesh_count() >= 1, "{ext}");
    }
    assert!(dir.path().join("m.mtl").exists());
}

#[test]
fn export_subset_of_unknown_node_is_core_error() {
    let o = ExportOptions {
        nodes: Some(vec![uuid::Uuid::new_v4()]),
        ..opts()
    };
    let e = export_bytes(&Scene::new(), ExportFormat::Glb, &o).unwrap_err();
    assert_eq!(e.code(), "node_not_found");
}

// ---------------------------------------------------------------------------
// Undoable import
// ---------------------------------------------------------------------------

#[test]
fn import_is_one_undoable_history_entry() {
    let src = sample_doc();
    let b = export_bytes(&src.scene, ExportFormat::Glb, &opts()).unwrap();

    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"},"name":"Base"}"#,
        Origin::Gui,
    )
    .unwrap();
    let before = doc.state();
    let history_before = doc.history_list().len();

    let imported = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "sample",
        &ImportOptions::default(),
    )
    .unwrap();
    let roots = imported.roots().to_vec();
    let n = imported.node_count();
    let summary = doc
        .execute(import_command(imported, None), Origin::Ai)
        .unwrap();
    assert_eq!(summary.created.len(), n);
    assert!(summary.selection_changed);
    assert_eq!(doc.scene.len(), 1 + n);
    assert_eq!(doc.history_list().len(), history_before + 1);
    assert_eq!(
        doc.history_list().last().unwrap().label,
        format!("AI: Import 'sample' ({n} objects)")
    );
    // Name clash with the existing "Base" is resolved, hierarchy is intact.
    assert!(doc.scene.find_by_name("Base.001").is_some());
    let ball = doc.scene.find_by_name("Ball").unwrap();
    let ball_parent = doc.scene.node(ball).unwrap().parent().unwrap();
    assert_eq!(doc.scene.node(ball_parent).unwrap().name, "Base.001");
    // Imported roots are selected.
    assert_eq!(doc.selection.objects(), roots.as_slice());

    let after = doc.state();
    doc.undo().unwrap();
    assert_eq!(doc.state(), before);
    doc.redo().unwrap();
    assert_eq!(doc.state(), after);
}

#[test]
fn import_under_parent_and_failure_is_atomic() {
    let b = export_bytes(&sample_doc().scene, ExportFormat::Obj, &opts()).unwrap();
    let mut doc = Document::new();
    doc.execute_json(r#"{"op":"create_empty","name":"Holder"}"#, Origin::Gui)
        .unwrap();
    let holder = doc.selection.active().unwrap();
    let imp = import_bytes(
        &b.files[0].bytes,
        ImportFormat::Obj,
        "o",
        &ImportOptions::default(),
    )
    .unwrap();
    doc.execute(import_command(imp.clone(), Some(holder)), Origin::Gui)
        .unwrap();
    assert_eq!(
        doc.scene.node(holder).unwrap().children().len(),
        imp.roots().len()
    );

    let before = doc.state();
    let e = doc
        .execute(import_command(imp, Some(uuid::Uuid::new_v4())), Origin::Gui)
        .unwrap_err();
    assert_eq!(e.code(), "batch_failed");
    assert_eq!(doc.state(), before);
}

#[test]
fn import_into_scene_directly() {
    let b = export_bytes(&sample_doc().scene, ExportFormat::Glb, &opts()).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Glb,
        "s",
        &ImportOptions::default(),
    )
    .unwrap();
    let n = imp.node_count();
    let mut scene = Scene::new();
    let roots = import_into(&mut scene, imp, None).unwrap();
    assert_eq!(scene.len(), n);
    assert_eq!(scene.roots(), roots.as_slice());
    scene.check_hierarchy().unwrap();
}

#[test]
fn import_file_into_document_helper() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cube.stl");
    export_scene(&cube_scene(), &p, &opts()).unwrap();
    let mut doc = Document::new();
    let s = import_file_into_document(&mut doc, &p, &ImportOptions::default(), None, Origin::Cli)
        .unwrap();
    assert_eq!(s.created.len(), 1);
    assert_eq!(s.created[0].name, "cube");
    assert_eq!(doc.history_list()[0].label, "CLI: Import 'cube' (1 object)");
}

// ---------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------

#[test]
fn truncated_and_garbled_inputs_never_panic() {
    let doc = sample_doc();
    let cases: Vec<(ImportFormat, Vec<u8>)> = vec![
        (
            ImportFormat::Glb,
            export_bytes(&doc.scene, ExportFormat::Glb, &opts())
                .unwrap()
                .files
                .remove(0)
                .bytes,
        ),
        (
            ImportFormat::Gltf,
            export_bytes(&doc.scene, ExportFormat::Gltf, &opts())
                .unwrap()
                .files
                .remove(0)
                .bytes,
        ),
        (
            ImportFormat::Obj,
            export_bytes(&doc.scene, ExportFormat::Obj, &opts())
                .unwrap()
                .files
                .remove(0)
                .bytes,
        ),
        (
            ImportFormat::Stl,
            export_bytes(&doc.scene, ExportFormat::Stl, &opts())
                .unwrap()
                .files
                .remove(0)
                .bytes,
        ),
        (ImportFormat::F3d, project_to_bytes(&doc).unwrap()),
    ];
    for (fmt, bytes) in cases {
        let step = (bytes.len() / 150).max(1);
        for len in (0..bytes.len()).step_by(step) {
            let _ = import_bytes(&bytes[..len], fmt, "t", &ImportOptions::default());
        }
        // Flip bytes at a few positions.
        for pos in (0..bytes.len()).step_by((bytes.len() / 60).max(1)) {
            let mut b = bytes.clone();
            b[pos] ^= 0xA5;
            let _ = import_bytes(&b, fmt, "t", &ImportOptions::default());
        }
    }
}

#[test]
fn gltf_malformed_inputs_are_parse_errors() {
    let io = ImportOptions::default();
    for src in [
        &b"not json at all"[..],
        b"{}",
        b"{\"asset\":{\"version\":\"1.0\"},\"nodes\":[{}]}",
        b"glTF",
        b"",
    ] {
        let e = import_bytes(src, ImportFormat::Gltf, "t", &io).unwrap_err();
        assert_eq!(
            e.code(),
            "parse_error",
            "{:?}",
            String::from_utf8_lossy(src)
        );
    }
    // Index out of range.
    let mut data: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    data.extend_from_slice(&[0, 1, 9, 0]);
    let b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&data)
    };
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"mesh":0}}],
        "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
        "buffers":[{{"byteLength":40,"uri":"data:application/octet-stream;base64,{b64}"}}],
        "bufferViews":[{{"buffer":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":4}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
                     {{"bufferView":1,"componentType":5121,"count":3,"type":"SCALAR"}}]}}"#
    );
    let e = import_bytes(json.as_bytes(), ImportFormat::Gltf, "t", &io).unwrap_err();
    assert_eq!(e.code(), "invalid_data", "{e}");
    // Escaping the resource folder is refused.
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{}],"buffers":[{"byteLength":4,"uri":"../secret.bin"}]}"#;
    let dir = tempfile::tempdir().unwrap();
    let io = ImportOptions {
        resource_dir: Some(dir.path().to_path_buf()),
        ..ImportOptions::default()
    };
    let e = import_bytes(json.as_bytes(), ImportFormat::Gltf, "t", &io).unwrap_err();
    assert_eq!(e.code(), "invalid_data");
}

fn glb_vertex_count(doc: &Document) -> usize {
    let b = export_bytes(&doc.scene, ExportFormat::Glb, &opts()).unwrap();
    let g = gltf::Gltf::from_slice(&b.primary().bytes).unwrap();
    let prim = g
        .document
        .meshes()
        .next()
        .unwrap()
        .primitives()
        .next()
        .unwrap();
    prim.get(&gltf::Semantic::Positions).unwrap().count()
}

#[test]
fn export_uses_evaluated_mesh_and_object_shading() {
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    assert_eq!(glb_vertex_count(&doc), 24);
    // Fully smooth: one normal per cube corner.
    doc.execute_json(r#"{"op":"set_shading","smooth":true}"#, Origin::Gui)
        .unwrap();
    assert_eq!(glb_vertex_count(&doc), 8);
    // Auto smooth below 90 degrees keeps the cube edges hard again.
    doc.execute_json(
        r#"{"op":"set_shading","smooth":true,"auto_smooth_angle_deg":30}"#,
        Origin::Gui,
    )
    .unwrap();
    assert_eq!(glb_vertex_count(&doc), 24);
    // Modifiers are applied on export (array of 2 separate cubes).
    doc.execute_json(
        r#"{"op":"add_modifier","modifier":{"type":"array","count":2,"offset":[3,0,0]}}"#,
        Origin::Gui,
    )
    .unwrap();
    assert_eq!(glb_vertex_count(&doc), 48);
    let b = export_bytes(&doc.scene, ExportFormat::Stl, &opts()).unwrap();
    let imp = import_bytes(
        &b.primary().bytes,
        ImportFormat::Stl,
        "c",
        &ImportOptions::default(),
    )
    .unwrap();
    let mesh = imp.scene.nodes().next().unwrap().mesh.as_ref().unwrap();
    assert_eq!(mesh.face_count(), 24);
}

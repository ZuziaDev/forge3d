//! Integration tests for every tool: happy path + structured error path.

mod common;

use common::{approx, env};
use serde_json::json;

#[tokio::test]
async fn tool_list_has_schemas_for_every_tool() {
    let e = env();
    let defs = e.tools.list();
    let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
    for n in [
        "scene_get",
        "object_create",
        "object_transform",
        "object_delete",
        "object_duplicate",
        "object_rename",
        "object_parent",
        "mesh_edit",
        "mesh_select",
        "mesh_get",
        "boolean_op",
        "material_set",
        "undo",
        "redo",
        "history_list",
        "viewport_render",
        "file_import",
        "file_export",
        "project_save",
        "project_load",
        "batch",
    ] {
        assert!(names.contains(&n), "missing tool {n}");
    }
    for d in &defs {
        assert_eq!(d.input_schema["type"], "object", "{}", d.name);
        assert!(d.description.len() > 40, "{}", d.name);
    }
    let create = defs.iter().find(|d| d.name == "object_create").unwrap();
    let props = &create.input_schema["properties"];
    assert!(props["primitive"]["description"]
        .as_str()
        .unwrap()
        .contains("cube"));
    assert!(props["translation"].is_object());
}

#[tokio::test]
async fn unknown_tool_and_bad_arguments() {
    let e = env();
    let err = e.err("make_cube", json!({})).await;
    assert_eq!(err.code, "unknown_tool");
    assert!(err.hint.contains("object_create"));

    let err = e
        .err(
            "object_create",
            json!({"primitive": {"type": "cube"}, "colour": 1}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
    assert!(err.message.contains("colour"), "{}", err.message);

    let err = e
        .err(
            "object_create",
            json!({"primitive": {"type": "dodecahedron"}}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
    assert!(err.message.contains("primitive"), "{}", err.message);

    let err = e.err("scene_get", json!([1, 2])).await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn scene_get_happy_and_error() {
    let e = env();
    let id = e.cube("Box").await;
    let s = e.scene().await;
    assert_eq!(s["object_count"], 1);
    let o = &s["objects"][0];
    assert_eq!(o["id"], id);
    assert_eq!(o["name"], "Box");
    assert_eq!(o["vertex_count"], 8);
    assert_eq!(o["edge_count"], 12);
    assert_eq!(o["face_count"], 6);
    approx(&o["world_bounds"]["min"], [-1.0, -1.0, -1.0]);
    approx(&o["scale"], [1.0, 1.0, 1.0]);
    assert_eq!(s["selection"]["active"], id);
    assert!(s["conventions"].as_str().unwrap().contains("Z is up"));

    let err = e.err("scene_get", json!({"include_hidden": "yes"})).await;
    assert_eq!(err.code, "invalid_arguments");
    assert!(err.message.contains("include_hidden"));
}

#[tokio::test]
async fn object_create_happy_and_errors() {
    let e = env();
    let v = e
        .ok(
            "object_create",
            json!({
                "primitive": {"type": "cylinder", "radius": 0.5, "depth": 2, "segments": 12},
                "name": "Pillar",
                "translation": [1, 2, 1],
                "rotation_euler_deg": [0, 0, 90],
                "material": {"name": "Stone", "base_color": [0.5, 0.5, 0.5, 1]}
            }),
        )
        .await;
    assert_eq!(v["object"]["name"], "Pillar");
    approx(&v["object"]["translation"], [1.0, 2.0, 1.0]);
    approx(&v["object"]["rotation_euler_deg"], [0.0, 0.0, 90.0]);
    assert_eq!(v["object"]["material"]["name"], "Stone");
    assert_eq!(v["changes"]["created"][0]["name"], "Pillar");
    assert_eq!(v["history_entry"]["label"], "AI: Create cylinder 'Pillar'");
    assert_eq!(v["history_entry"]["origin"], "ai");

    // Empty group node + child referenced by name.
    let g = e
        .ok(
            "object_create",
            json!({"name": "Group", "translation": [0, 0, 1]}),
        )
        .await;
    assert_eq!(g["object"]["has_mesh"], false);
    let c = e
        .ok(
            "object_create",
            json!({"primitive": {"type": "cube"}, "parent": "Group"}),
        )
        .await;
    assert_eq!(c["object"]["parent"], g["object"]["id"]);

    let err = e
        .err(
            "object_create",
            json!({"primitive": {"type": "cube"}, "parent": "Nope"}),
        )
        .await;
    assert_eq!(err.code, "object_not_found");
    assert!(err.hint.contains("scene_get"));
    assert!(err.hint.contains("by id or by unique"));
    assert!(err.hint.contains("'Pillar'"));

    let err = e
        .err(
            "object_create",
            json!({"primitive": {"type": "cube", "size": -1}}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");

    let err = e
        .err(
            "object_create",
            json!({"primitive": {"type": "cube"}, "scale": [1, 0, 1]}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");
}

#[tokio::test]
async fn object_transform_absolute_relative_and_errors() {
    let e = env();
    e.cube("A").await;
    let v = e
        .ok(
            "object_transform",
            json!({"objects": ["A"], "mode": "absolute", "translation": [1, 2, 3], "scale": [2, 2, 2]}),
        )
        .await;
    approx(&v["objects"][0]["translation"], [1.0, 2.0, 3.0]);
    approx(&v["objects"][0]["scale"], [2.0, 2.0, 2.0]);
    assert!(v["history_entry"]["label"]
        .as_str()
        .unwrap()
        .starts_with("AI: "));

    let v = e
        .ok(
            "object_transform",
            json!({"objects": ["A"], "translation": [1, 0, 0], "rotation_euler_deg": [0, 0, 90]}),
        )
        .await;
    approx(&v["objects"][0]["translation"], [2.0, 2.0, 3.0]);
    approx(&v["objects"][0]["rotation_euler_deg"], [0.0, 0.0, 90.0]);
    // One call = one history entry even though it is two sub-commands.
    assert_eq!(e.history_labels().await.len(), 3);

    let v = e
        .ok(
            "object_transform",
            json!({"objects": ["A"], "rotation_axis": [0, 0, 1], "rotation_angle_deg": -90}),
        )
        .await;
    approx(&v["objects"][0]["rotation_euler_deg"], [0.0, 0.0, 0.0]);

    let err = e.err("object_transform", json!({"objects": ["A"]})).await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e
        .err(
            "object_transform",
            json!({"objects": ["B"], "translation": [0, 0, 1]}),
        )
        .await;
    assert_eq!(err.code, "object_not_found");
    let err = e
        .err(
            "object_transform",
            json!({"objects": ["A"], "mode": "absolute", "rotation_axis": [0, 0, 1], "rotation_angle_deg": 5}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn object_delete_duplicate_rename_parent() {
    let e = env();
    let a = e.cube("A").await;
    e.cube("B").await;

    // duplicate
    let v = e
        .ok(
            "object_duplicate",
            json!({"objects": ["A"], "offset": [3, 0, 0]}),
        )
        .await;
    assert_eq!(v["objects"][0]["name"], "A.001");
    approx(&v["objects"][0]["translation"], [3.0, 0.0, 0.0]);
    assert_eq!(v["changes"]["created"].as_array().unwrap().len(), 1);

    // rename
    let v = e
        .ok("object_rename", json!({"object": a, "name": "Base"}))
        .await;
    assert_eq!(v["object"]["name"], "Base");
    let v = e
        .ok("object_rename", json!({"object": "B", "name": "Base"}))
        .await;
    assert_eq!(v["object"]["name"], "Base.001", "names are made unique");
    let err = e
        .err("object_rename", json!({"object": "A.001", "name": "  "}))
        .await;
    assert_eq!(err.code, "invalid_arguments");

    // parent
    let v = e
        .ok(
            "object_parent",
            json!({"objects": ["A.001"], "parent": "Base"}),
        )
        .await;
    assert_eq!(v["objects"][0]["parent"], a);
    let err = e
        .err(
            "object_parent",
            json!({"objects": ["Base"], "parent": "A.001"}),
        )
        .await;
    assert_eq!(err.code, "hierarchy_cycle");
    let v = e
        .ok(
            "object_parent",
            json!({"objects": ["A.001"], "parent": null}),
        )
        .await;
    assert!(v["objects"][0]["parent"].is_null());

    // delete
    let v = e
        .ok("object_delete", json!({"objects": ["Base.001"]}))
        .await;
    assert_eq!(v["changes"]["deleted"][0]["name"], "Base.001");
    assert_eq!(e.scene().await["object_count"], 2);
    let err = e.err("object_delete", json!({"objects": []})).await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e
        .err("object_delete", json!({"objects": ["Base.001"]}))
        .await;
    assert_eq!(err.code, "object_not_found");

    // duplicate with nothing selected
    e.ok(
        "mesh_select",
        json!({"by": {"type": "objects", "objects": []}}),
    )
    .await;
    let err = e.err("object_duplicate", json!({})).await;
    assert_eq!(err.code, "empty_selection");
    assert!(err.hint.contains("mesh_select") || err.hint.contains("explicitly"));
}

#[tokio::test]
async fn ambiguous_names_are_reported() {
    use forge3d_api::{DocumentHost, Forge3dTools, Workspace};
    use forge3d_core::{command::Origin, document::Document, scene::Node};
    // Names are kept unique by the commands, but loaded files may contain
    // duplicates: build such a document directly.
    let mut doc = Document::new();
    let a = doc.scene.add_node(Node::new("Twin"), None).unwrap();
    doc.scene.add_node(Node::new("Twin"), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let tools = Forge3dTools::new(
        DocumentHost::spawn(doc),
        Workspace::new(dir.path()).unwrap(),
        Origin::Ai,
    );
    let err = tools
        .call("object_rename", json!({"object": "Twin", "name": "x"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "ambiguous_object");
    assert_eq!(err.details.unwrap()["ids"].as_array().unwrap().len(), 2);
    assert!(err.hint.contains("id"));
    // Ids always work.
    let v = tools
        .call(
            "object_rename",
            json!({"object": a.to_string(), "name": "Solo"}),
        )
        .await
        .unwrap();
    assert_eq!(v["object"]["name"], "Solo");
}

#[tokio::test]
async fn mesh_select_and_edit_chain() {
    let e = env();
    e.cube("Box").await;
    let v = e
        .ok(
            "mesh_select",
            json!({"object": "Box", "by": {"type": "direction", "direction": [0, 0, 1]}}),
        )
        .await;
    assert_eq!(v["kind"], "face");
    assert_eq!(v["selected_count"], 1);
    assert_eq!(v["selection"]["mode"], "face");
    let top = v["indices"][0].as_u64().unwrap();

    // extrude uses the selection
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "Box", "operation": {"type": "extrude", "distance": 0.5}}),
        )
        .await;
    assert_eq!(v["mesh"]["vertex_count"], 12);
    assert_eq!(v["mesh"]["face_count"], 10);
    assert_eq!(v["changes"]["modified"][0]["vertex_count"], 12);
    assert!(v["history_entry"]["label"]
        .as_str()
        .unwrap()
        .starts_with("AI: Extrude"));
    assert_eq!(
        v["selection"]["counts"]["faces"], 1,
        "the cap stays selected"
    );
    let o = e.object("Box").await;
    approx(&o["world_bounds"]["max"], [1.0, 1.0, 1.5]);

    // inset + explicit faces
    let v = e
        .ok(
            "mesh_edit",
            json!({"operation": {"type": "inset", "faces": [top], "thickness": 0.2}}),
        )
        .await;
    assert!(v["mesh"]["face_count"].as_u64().unwrap() > 10);

    // other operations
    e.ok(
        "mesh_edit",
        json!({"operation": {"type": "subdivide", "levels": 1}}),
    )
    .await;
    e.ok(
        "mesh_edit",
        json!({"operation": {"type": "recalculate_normals"}}),
    )
    .await;
    e.ok("mesh_edit", json!({"operation": {"type": "flip_normals"}}))
        .await;

    // errors
    let err = e
        .err(
            "mesh_edit",
            json!({"operation": {"type": "extrude", "faces": [99999]}}),
        )
        .await;
    assert_eq!(err.code, "element_out_of_range");
    assert!(err.hint.contains("mesh_get") || err.hint.contains("mesh_select"));
    let err = e
        .err(
            "mesh_edit",
            json!({"operation": {"type": "subdivide", "levels": 9}}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e
        .err("mesh_edit", json!({"operation": {"type": "explode"}}))
        .await;
    assert_eq!(err.code, "invalid_arguments");
    let g = e.ok("object_create", json!({"name": "Empty"})).await;
    let err = e
        .err(
            "mesh_edit",
            json!({"object": g["object"]["id"], "operation": {"type": "subdivide"}}),
        )
        .await;
    assert_eq!(err.code, "node_has_no_mesh");
}

#[tokio::test]
async fn mesh_edit_loop_cut_bevel_mirror_merge_delete_transform() {
    let e = env();
    e.cube("C").await;
    let edges = e
        .ok("mesh_get", json!({"object": "C", "include": ["edges"]}))
        .await;
    assert_eq!(edges["edges"]["total"], 12);
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "C", "operation": {"type": "loop_cut", "edge": 0, "cuts": 2}}),
        )
        .await;
    assert!(v["mesh"]["face_count"].as_u64().unwrap() > 6);

    e.cube("D").await;
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "D", "operation": {"type": "bevel", "edges": [0], "width": 0.1}}),
        )
        .await;
    assert_eq!(v["mesh"]["face_count"], 7);

    e.cube("M").await;
    e.ok(
        "mesh_select",
        json!({"object": "M", "kind": "vertex", "by": {"type": "box", "min": [-2, -2, -2], "max": [2, 2, 2]}}),
    )
    .await;
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "M", "operation": {"type": "transform", "translation": [2, 0, 0]}}),
        )
        .await;
    assert_eq!(v["mesh"]["vertex_count"], 8);
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "M", "operation": {"type": "mirror", "axis": "x"}}),
        )
        .await;
    assert_eq!(v["mesh"]["vertex_count"], 16);
    let v = e
        .ok("mesh_edit", json!({"object": "M", "operation": {"type": "merge", "vertices": [0, 1], "mode": "center"}}))
        .await;
    assert_eq!(v["mesh"]["vertex_count"], 15);
    let v = e
        .ok(
            "mesh_edit",
            json!({"object": "M", "operation": {"type": "delete", "kind": "face", "indices": [0]}}),
        )
        .await;
    assert!(v["mesh"]["face_count"].as_u64().unwrap() < 12);
    let err = e
        .err(
            "mesh_edit",
            json!({"object": "M", "operation": {"type": "transform"}}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn mesh_select_variants_and_errors() {
    let e = env();
    e.cube("Box").await;
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "all"}, "kind": "vertex"}),
        )
        .await;
    assert_eq!(v["selected_count"], 8);
    assert_eq!(v["selection"]["mode"], "vertex");
    let v = e.ok("mesh_select", json!({"by": {"type": "indices", "indices": [0, 1]}, "kind": "vertex", "action": "remove"})).await;
    assert_eq!(v["selected_count"], 6);
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "invert"}, "kind": "vertex"}),
        )
        .await;
    assert_eq!(v["selected_count"], 2);
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "box", "min": [-2, -2, 0.5], "max": [2, 2, 2]}, "kind": "face"}),
        )
        .await;
    assert_eq!(v["indices"].as_array().unwrap().len(), 1);
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "edge_loop", "edge": 0, "ring": true}}),
        )
        .await;
    assert_eq!(v["kind"], "edge");
    assert!(v["selected_count"].as_u64().unwrap() >= 2);
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "normal", "reference": 0}, "kind": "face"}),
        )
        .await;
    assert_eq!(v["selected_count"], 1);
    let v = e
        .ok(
            "mesh_select",
            json!({"by": {"type": "all"}, "kind": "face", "max_indices": 2}),
        )
        .await;
    assert_eq!(v["indices"].as_array().unwrap().len(), 2);
    assert_eq!(v["truncated"], true);
    let v = e.ok("mesh_select", json!({"by": {"type": "none"}})).await;
    assert_eq!(v["selected_count"], 0);
    let v = e.ok("mesh_select", json!({"mode": "object"})).await;
    assert_eq!(v["selection"]["mode"], "object");
    // Selection steps are recorded as AI history entries.
    assert!(e
        .history_labels()
        .await
        .iter()
        .any(|l| l.starts_with("AI: Select")));

    let err = e
        .err(
            "mesh_select",
            json!({"by": {"type": "indices", "indices": [42]}, "kind": "vertex"}),
        )
        .await;
    assert_eq!(err.code, "element_out_of_range");
    let err = e
        .err(
            "mesh_select",
            json!({"by": {"type": "direction", "direction": [0, 0, 0]}}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e.err("mesh_select", json!({})).await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn mesh_get_paging_normals_validation() {
    let e = env();
    e.ok(
        "object_create",
        json!({"primitive": {"type": "uv_sphere", "segments": 64, "rings": 32}, "name": "Ball", "translation": [0, 0, 5]}),
    )
    .await;
    let v = e
        .ok("mesh_get", json!({"object": "Ball", "limit": 100}))
        .await;
    let total_v = v["vertices"]["total"].as_u64().unwrap();
    assert!(total_v > 1000);
    assert_eq!(v["vertices"]["items"].as_array().unwrap().len(), 100);
    assert_eq!(v["vertices"]["next_offset"], 100);
    assert_eq!(v["faces"]["items"][0]["index"], 0);
    assert_eq!(v["validation"]["is_closed"], true);
    assert_eq!(v["validation"]["is_valid"], true);
    assert!(v["vertices"]["items"][0].get("normal").is_none());

    // hard cap
    let v = e
        .ok(
            "mesh_get",
            json!({"object": "Ball", "limit": 50000, "include": ["vertices"]}),
        )
        .await;
    assert_eq!(v["vertices"]["items"].as_array().unwrap().len(), 1000);
    assert_eq!(v["limit_capped"], true);
    assert!(v.get("faces").is_none());

    // last page
    let off = total_v - 10;
    let v = e
        .ok("mesh_get", json!({"object": "Ball", "offset": off, "limit": 100, "include": ["vertices"], "normals": true, "space": "world", "validate": false}))
        .await;
    assert_eq!(v["vertices"]["items"].as_array().unwrap().len(), 10);
    assert!(v["vertices"]["next_offset"].is_null());
    assert!(v["vertices"]["items"][0]["normal"].is_array());
    assert!(v.get("validation").is_none());
    // world space: the ball sits at z = 5
    let z = v["vertices"]["items"][0]["position"][2].as_f64().unwrap();
    assert!(z > 3.9, "{z}");

    let v = e
        .ok(
            "mesh_get",
            json!({"object": "Ball", "offset": total_v + 5, "include": ["vertices"]}),
        )
        .await;
    assert_eq!(v["vertices"]["items"].as_array().unwrap().len(), 0);

    let err = e
        .err("mesh_get", json!({"object": "Ball", "limit": 0}))
        .await;
    assert_eq!(err.code, "invalid_arguments");
    e.ok("object_create", json!({"name": "Group"})).await;
    let err = e.err("mesh_get", json!({"object": "Group"})).await;
    assert_eq!(err.code, "node_has_no_mesh");
}

#[tokio::test]
async fn boolean_op_happy_and_errors() {
    let e = env();
    e.cube("A").await;
    e.ok(
        "object_create",
        json!({"primitive": {"type": "cube", "size": 1}, "name": "Cutter", "translation": [1, 0.3, 0.2]}),
    )
    .await;
    let v = e
        .ok(
            "boolean_op",
            json!({"object": "A", "other": "Cutter", "operation": "difference"}),
        )
        .await;
    assert_eq!(v["object"]["name"], "A");
    assert!(v["object"]["face_count"].as_u64().unwrap() > 6);
    assert_eq!(v["changes"]["deleted"][0]["name"], "Cutter");
    assert_eq!(e.scene().await["object_count"], 1);

    let err = e
        .err(
            "boolean_op",
            json!({"object": "A", "other": "A", "operation": "union"}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e
        .err(
            "boolean_op",
            json!({"object": "A", "other": "Ghost", "operation": "union"}),
        )
        .await;
    assert_eq!(err.code, "object_not_found");
    let err = e
        .err(
            "boolean_op",
            json!({"object": "A", "other": "A", "operation": "xor"}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn material_set_happy_and_errors() {
    let e = env();
    e.cube("A").await;
    e.cube("B").await;
    let v = e
        .ok(
            "material_set",
            json!({"objects": ["A", "B"], "material": {"name": "Red", "base_color": [1, 0, 0, 1], "metallic": 0.5}}),
        )
        .await;
    assert_eq!(v["objects"].as_array().unwrap().len(), 2);
    assert_eq!(v["objects"][0]["material"]["name"], "Red");
    assert_eq!(v["changes"]["modified"].as_array().unwrap().len(), 2);
    let v = e
        .ok("material_set", json!({"objects": ["A"], "clear": true}))
        .await;
    assert!(v["objects"][0]["material"].is_null());

    let err = e
        .err(
            "material_set",
            json!({"objects": ["A"], "material": {"roughness": 2.0}}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");
    assert!(err.hint.contains("roughness"));
    let err = e.err("material_set", json!({"objects": ["A"]})).await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn undo_redo_history_with_ai_labels() {
    let e = env();
    e.cube("A").await;
    e.ok(
        "object_transform",
        json!({"objects": ["A"], "translation": [0, 0, 1]}),
    )
    .await;
    let labels = e.history_labels().await;
    assert_eq!(labels.len(), 2);
    assert!(labels.iter().all(|l| l.starts_with("AI: ")), "{labels:?}");

    let v = e.ok("undo", json!({})).await;
    assert_eq!(v["undone"].as_array().unwrap().len(), 1);
    approx(&e.object("A").await["translation"], [0.0, 0.0, 0.0]);
    let v = e.ok("redo", json!({})).await;
    assert_eq!(v["redone"][0]["label"], labels[1]);
    approx(&e.object("A").await["translation"], [0.0, 0.0, 1.0]);

    let v = e.ok("undo", json!({"steps": 5})).await;
    assert_eq!(v["undone"].as_array().unwrap().len(), 2);
    assert!(v["stopped_early"]["code"] == "nothing_to_undo");
    assert_eq!(e.scene().await["object_count"], 0);

    let h = e.ok("history_list", json!({"limit": 1})).await;
    assert_eq!(h["total"], 2);
    assert_eq!(h["entries"].as_array().unwrap().len(), 1);
    assert_eq!(h["truncated"], true);
    assert_eq!(h["entries"][0]["undone"], true);

    let err = e.err("undo", json!({})).await;
    assert_eq!(err.code, "nothing_to_undo");
    e.ok("redo", json!({"steps": 2})).await;
    let err = e.err("redo", json!({})).await;
    assert_eq!(err.code, "nothing_to_redo");
    let err = e.err("undo", json!({"steps": 0})).await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e.err("history_list", json!({"limit": "all"})).await;
    assert_eq!(err.code, "invalid_arguments");
}

#[tokio::test]
async fn viewport_render_returns_png_of_requested_size() {
    let e = env();
    e.cube("A").await;
    let out = e
        .tools
        .call_rich(
            "viewport_render",
            json!({"width": 320, "height": 200, "camera": {"kind": "auto", "view": "front", "focus": ["A"]}, "wireframe": true}),
        )
        .await
        .unwrap();
    assert_eq!(out.images.len(), 1);
    let img = &out.images[0];
    assert_eq!(img.mime_type, "image/png");
    let decoder = png::Decoder::new(std::io::Cursor::new(img.data.clone()));
    let reader = decoder.read_info().unwrap();
    assert_eq!(reader.info().width, 320);
    assert_eq!(reader.info().height, 200);
    assert!(out.value["description"]
        .as_str()
        .unwrap()
        .contains("320x200"));

    // via `call` the image is embedded as base64
    let v = e
        .ok("viewport_render", json!({"width": 64, "height": 48, "camera": {"kind": "look_at", "eye": [5, -5, 4], "target": [0, 0, 0]}}))
        .await;
    assert_eq!(v["images"][0]["mime_type"], "image/png");
    assert!(v["images"][0]["data_base64"].as_str().unwrap().len() > 100);
    e.ok("viewport_render", json!({"width": 64, "height": 64, "camera": {"kind": "orbit", "distance": 8, "yaw_deg": 30}})).await;

    let err = e.err("viewport_render", json!({"width": 5000})).await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e
        .err(
            "viewport_render",
            json!({"camera": {"kind": "auto", "focus": ["Ghost"]}}),
        )
        .await;
    assert_eq!(err.code, "object_not_found");
    let err = e
        .err("viewport_render", json!({"width": 64, "height": 64, "camera": {"kind": "look_at", "eye": [0, 0, 0], "target": [0, 0, 0]}}))
        .await;
    assert_eq!(err.code, "invalid_camera");
}

#[tokio::test]
async fn file_export_import_and_project_roundtrip() {
    let e = env();
    e.cube("A").await;
    e.ok(
        "object_create",
        json!({"primitive": {"type": "torus"}, "name": "Ring", "translation": [3, 0, 0]}),
    )
    .await;

    let v = e.ok("file_export", json!({"path": "out/scene.glb"})).await;
    assert_eq!(v["files"][0]["path"], "out/scene.glb");
    assert!(e.dir.path().join("out/scene.glb").exists());
    let v = e
        .ok(
            "file_export",
            json!({"path": "ring.obj", "objects": ["Ring"]}),
        )
        .await;
    assert_eq!(v["files"][0]["path"], "ring.obj");
    e.ok(
        "file_export",
        json!({"path": "a.stl", "objects": ["A"], "stl_ascii": true}),
    )
    .await;

    let v = e.ok("file_import", json!({"path": "out/scene.glb"})).await;
    assert_eq!(v["objects"].as_array().unwrap().len(), 2);
    assert!(v["history_entry"]["label"]
        .as_str()
        .unwrap()
        .starts_with("AI: Import"));
    assert_eq!(e.scene().await["object_count"], 4);
    e.ok("undo", json!({})).await;
    assert_eq!(e.scene().await["object_count"], 2);

    let v = e
        .ok("project_save", json!({"path": "proj/model.f3d"}))
        .await;
    assert_eq!(v["objects"], 2);
    e.ok("object_delete", json!({"objects": ["A", "Ring"]}))
        .await;
    assert_eq!(e.scene().await["object_count"], 0);

    let v = e
        .ok("project_load", json!({"path": "proj/model.f3d"}))
        .await;
    assert_eq!(v["scene"]["object_count"], 2);
    assert_eq!(v["changes"]["created"].as_array().unwrap().len(), 2);
    assert!(v["history_entry"]["label"]
        .as_str()
        .unwrap()
        .starts_with("AI: Load project"));
    // Loading is undoable.
    e.ok("undo", json!({})).await;
    assert_eq!(e.scene().await["object_count"], 0);
    e.ok("redo", json!({})).await;
    assert_eq!(e.scene().await["object_count"], 2);
    let v = e
        .ok(
            "project_load",
            json!({"path": "proj/model.f3d", "clear_history": true}),
        )
        .await;
    assert_eq!(v["history_cleared"], true);
    assert_eq!(e.ok("history_list", json!({})).await["total"], 0);

    // errors
    let err = e.err("file_import", json!({"path": "missing.glb"})).await;
    assert_eq!(err.code, "file_not_found");
    let err = e.err("file_export", json!({"path": "scene.xyz"})).await;
    assert_eq!(err.code, "unsupported_format");
    let err = e.err("project_save", json!({"path": "scene.glb"})).await;
    assert_eq!(err.code, "invalid_arguments");
    std::fs::write(e.dir.path().join("bad.f3d"), b"not json").unwrap();
    let err = e.err("project_load", json!({"path": "bad.f3d"})).await;
    assert!(
        err.code == "parse_error" || err.code == "invalid_project",
        "{err:?}"
    );
    let err = e
        .err(
            "file_import",
            json!({"path": "bad.f3d", "parent": "Nobody"}),
        )
        .await;
    assert_eq!(err.code, "object_not_found");
}

#[tokio::test]
async fn path_traversal_and_symlink_escapes_are_rejected() {
    let e = env();
    e.cube("A").await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.glb"), b"x").unwrap();

    for path in [
        "../escape.glb",
        "sub/../../escape.glb",
        "new/../../escape.glb",
    ] {
        let err = e.err("file_export", json!({"path": path})).await;
        assert_eq!(err.code, "path_not_allowed", "{path}: {err:?}");
        assert!(err
            .hint
            .contains(&e.tools.workspace().root().display().to_string()));
    }
    let abs = outside.path().join("abs.glb");
    let err = e
        .err("file_export", json!({"path": abs.to_str().unwrap()}))
        .await;
    assert_eq!(err.code, "path_not_allowed");
    let err = e
        .err(
            "file_import",
            json!({"path": outside.path().join("secret.glb").to_str().unwrap()}),
        )
        .await;
    assert_eq!(err.code, "path_not_allowed");
    assert!(!abs.exists());

    // Absolute paths inside the workspace are fine.
    let inside = e.tools.workspace().root().join("inside.glb");
    e.ok("file_export", json!({"path": inside.to_str().unwrap()}))
        .await;

    #[cfg(unix)]
    {
        // Directory symlink pointing outside.
        std::os::unix::fs::symlink(outside.path(), e.dir.path().join("link")).unwrap();
        let err = e.err("file_export", json!({"path": "link/x.glb"})).await;
        assert_eq!(err.code, "path_not_allowed");
        let err = e
            .err("file_import", json!({"path": "link/secret.glb"}))
            .await;
        assert_eq!(err.code, "path_not_allowed");
        // File symlink pointing outside.
        std::os::unix::fs::symlink(
            outside.path().join("secret.glb"),
            e.dir.path().join("s.glb"),
        )
        .unwrap();
        let err = e.err("file_import", json!({"path": "s.glb"})).await;
        assert_eq!(err.code, "path_not_allowed");
        let err = e.err("project_save", json!({"path": "link/p.f3d"})).await;
        assert_eq!(err.code, "path_not_allowed");
        // Dangling symlink (would be created outside on write).
        std::os::unix::fs::symlink(
            outside.path().join("nothing.glb"),
            e.dir.path().join("dangling.glb"),
        )
        .unwrap();
        let err = e.err("file_export", json!({"path": "dangling.glb"})).await;
        assert_eq!(err.code, "path_not_allowed");
        assert!(!outside.path().join("nothing.glb").exists());
        assert!(!outside.path().join("x.glb").exists());
    }
}

#[tokio::test]
async fn batch_success_with_name_references() {
    let e = env();
    let v = e
        .ok(
            "batch",
            json!({
                "description": "Build a stool",
                "steps": [
                    {"tool": "object_create", "arguments": {"primitive": {"type": "cylinder", "radius": 0.4, "depth": 0.1}, "name": "Seat", "translation": [0, 0, 0.5]}},
                    {"tool": "object_create", "arguments": {"primitive": {"type": "cylinder", "radius": 0.05, "depth": 0.5}, "name": "Leg", "parent": "Seat", "translation": [0.25, 0, -0.25]}},
                    {"tool": "object_duplicate", "arguments": {"objects": ["Leg"], "offset": [-0.5, 0, 0]}},
                    {"tool": "material_set", "arguments": {"objects": ["Seat"], "material": {"base_color": [0.6, 0.4, 0.2, 1]}}},
                    {"tool": "mesh_select", "arguments": {"object": "Seat", "by": {"type": "direction", "direction": [0, 0, 1]}}},
                    {"tool": "mesh_edit", "arguments": {"object": "Seat", "operation": {"type": "inset", "thickness": 0.05}}},
                    {"command": {"op": "subdivide", "levels": 1}}
                ]
            }),
        )
        .await;
    assert_eq!(v["steps"].as_array().unwrap().len(), 7);
    assert_eq!(v["steps"][1]["object"]["name"], "Leg");
    assert_eq!(v["history_entry"]["label"], "AI: Build a stool");
    assert_eq!(v["changes"]["created"].as_array().unwrap().len(), 3);
    let labels = e.history_labels().await;
    assert_eq!(
        labels,
        vec!["AI: Build a stool".to_string()],
        "one history entry"
    );
    assert_eq!(e.scene().await["object_count"], 3);
    // The ids reported by the steps are the ids in the scene.
    let leg = e.object("Leg").await;
    assert_eq!(leg["id"], v["steps"][1]["object"]["id"]);
    // One undo removes everything.
    e.ok("undo", json!({})).await;
    assert_eq!(e.scene().await["object_count"], 0);
    e.ok("redo", json!({})).await;
    assert_eq!(e.scene().await["object_count"], 3);
    assert!(e.object("Seat").await["face_count"].as_u64().unwrap() > 40);
}

#[tokio::test]
async fn batch_failure_rolls_back_and_names_index() {
    let e = env();
    e.cube("Keep").await;
    let before = e.scene().await;
    let err = e
        .err(
            "batch",
            json!({"steps": [
                {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "Temp"}},
                {"tool": "object_transform", "arguments": {"objects": ["Keep", "Temp"], "translation": [1, 0, 0]}},
                {"tool": "mesh_edit", "arguments": {"object": "Temp", "operation": {"type": "extrude", "faces": [77]}}},
                {"tool": "object_delete", "arguments": {"objects": ["Keep"]}}
            ]}),
        )
        .await;
    assert_eq!(err.code, "batch_failed");
    let d = err.details.clone().unwrap();
    assert_eq!(d["index"], 2);
    assert_eq!(d["step"], "mesh_edit");
    assert_eq!(d["step_error"]["code"], "element_out_of_range");
    assert!(err.message.contains("step 2"));
    let after = e.scene().await;
    assert_eq!(after["objects"], before["objects"], "rolled back");
    assert_eq!(after["selection"], before["selection"]);
    assert_eq!(
        e.history_labels().await.len(),
        1,
        "no history entry for the failed batch"
    );

    // Planning errors (unknown object) also name the index.
    let err = e
        .err(
            "batch",
            json!({"steps": [
                {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}}},
                {"tool": "object_rename", "arguments": {"object": "Nobody", "name": "x"}}
            ]}),
        )
        .await;
    assert_eq!(err.details.unwrap()["index"], 1);
    assert_eq!(e.scene().await["object_count"], 1);

    // Shape errors.
    let err = e
        .err(
            "batch",
            json!({"steps": [{"tool": "undo", "arguments": {}}]}),
        )
        .await;
    assert_eq!(err.code, "batch_failed");
    assert!(err.hint.contains("object_create"));
    let err = e
        .err("batch", json!({"steps": [{"command": {"op": "levitate"}}]}))
        .await;
    assert_eq!(
        err.details.unwrap()["step_error"]["code"],
        "invalid_command"
    );
    let err = e.err("batch", json!({"steps": []})).await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e.err("batch", json!({"steps": [{}]})).await;
    assert_eq!(err.code, "batch_failed");
}

#[tokio::test]
async fn cli_origin_labels() {
    use forge3d_api::{DocumentHost, Forge3dTools, Workspace};
    use forge3d_core::{command::Origin, document::Document};
    let dir = tempfile::tempdir().unwrap();
    let tools = Forge3dTools::new(
        DocumentHost::spawn(Document::new()),
        Workspace::new(dir.path()).unwrap(),
        Origin::Cli,
    );
    let v = tools
        .call("object_create", json!({"primitive": {"type": "cube"}}))
        .await
        .unwrap();
    assert_eq!(v["history_entry"]["label"], "CLI: Create cube 'Cube'");
}

//! Modeling tools added in v0.1.1: new mesh_edit operations, modifier_edit,
//! object_shading, object_join and mesh_measure (happy + error path each).

mod common;

use common::env;
use serde_json::json;

async fn edit(e: &common::Env, object: &str, op: serde_json::Value) -> serde_json::Value {
    e.ok("mesh_edit", json!({"object": object, "operation": op}))
        .await
}

#[tokio::test]
async fn new_tools_are_listed_with_schemas() {
    let e = env();
    let defs = e.tools.list();
    for n in ["modifier_edit", "object_shading", "object_join", "mesh_measure"] {
        let d = defs.iter().find(|d| d.name == n).unwrap_or_else(|| panic!("{n}"));
        assert_eq!(d.input_schema["type"], "object", "{n}");
    }
    let mesh_edit = defs.iter().find(|d| d.name == "mesh_edit").unwrap();
    let schema = mesh_edit.input_schema.to_string();
    for op in [
        "solidify",
        "bridge_edge_loops",
        "fill_holes",
        "dissolve",
        "triangulate",
        "tris_to_quads",
        "poke",
        "smooth",
        "separate",
        "segments",
    ] {
        assert!(schema.contains(op), "mesh_edit schema lacks {op}");
    }
    let m = defs.iter().find(|d| d.name == "modifier_edit").unwrap();
    let schema = m.input_schema.to_string();
    for k in ["mirror", "array", "subdivision", "solidify", "bevel", "triangulate", "apply"] {
        assert!(schema.contains(k), "modifier_edit schema lacks {k}");
    }
}

#[tokio::test]
async fn mesh_edit_new_operations_happy_paths() {
    let e = env();
    e.cube("C").await;
    let v = edit(&e, "C", json!({"type": "bevel", "edges": [0], "width": 0.2, "segments": 3})).await;
    assert_eq!(v["mesh"]["face_count"], 9);
    assert_eq!(v["selection"]["counts"]["faces"], 3);
    let v = edit(&e, "C", json!({"type": "triangulate"})).await;
    // Only the selected (new bevel) faces are triangulated: 9 - 3 + 6.
    assert_eq!(v["mesh"]["face_count"], 12);
    let v = edit(&e, "C", json!({"type": "tris_to_quads", "max_angle_deg": 1})).await;
    assert!(v["mesh"]["face_count"].as_u64().unwrap() <= 9 + 4);
    let v = edit(&e, "C", json!({"type": "smooth", "factor": 0.2, "iterations": 2})).await;
    assert!(v["mesh"]["vertex_count"].as_u64().unwrap() > 8);

    e.cube("P").await;
    let v = edit(&e, "P", json!({"type": "poke", "faces": [0], "offset": 0.5})).await;
    assert_eq!(v["mesh"]["face_count"], 9);
    let v = edit(&e, "P", json!({"type": "dissolve", "kind": "vertex", "indices": [8]})).await;
    assert_eq!(v["mesh"]["face_count"], 6);

    e.cube("H").await;
    edit(&e, "H", json!({"type": "delete", "kind": "face", "indices": [0]})).await;
    let v = edit(&e, "H", json!({"type": "fill_holes"})).await;
    assert_eq!(v["mesh"]["face_count"], 6);
    let v = edit(&e, "H", json!({"type": "solidify", "thickness": 0.1})).await;
    assert_eq!(v["mesh"]["face_count"], 12);

    let v = edit(&e, "H", json!({"type": "separate", "faces": [0, 1, 2, 3, 4, 5], "name": "Inner"})).await;
    assert_eq!(v["mesh"]["face_count"], 6);
    assert_eq!(v["created"][0]["name"], "Inner");
    assert_eq!(v["created"][0]["face_count"], 6);

    let labels = e.history_labels().await;
    assert!(labels.iter().any(|l| l.starts_with("AI: Bevel 1 edge (3 segments)")), "{labels:?}");
}

#[tokio::test]
async fn bridge_tool_and_error_paths() {
    let e = env();
    e.ok(
        "object_create",
        json!({"primitive": {"type": "cylinder", "segments": 8}, "name": "T"}),
    )
    .await;
    let mesh = e
        .ok("mesh_get", json!({"object": "T", "include": ["faces"]}))
        .await;
    let caps: Vec<u64> = mesh["faces"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["vertices"].as_array().unwrap().len() == 8)
        .map(|f| f["index"].as_u64().unwrap())
        .collect();
    assert_eq!(caps.len(), 2, "{mesh}");
    edit(&e, "T", json!({"type": "delete", "kind": "face", "indices": caps})).await;
    let all: Vec<u32> = (0..24).collect();
    let v = edit(&e, "T", json!({"type": "bridge_edge_loops", "edges": all})).await;
    assert_eq!(v["mesh"]["face_count"], 16);

    // error paths: structured codes with hints
    e.cube("C").await;
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "bridge_edge_loops", "edges": [0]}}))
        .await;
    assert_eq!(err.code, "invalid_selection");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "fill_holes"}}))
        .await;
    assert_eq!(err.code, "invalid_selection");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "bevel", "edges": [0], "segments": 40}}))
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "smooth", "factor": 3}}))
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "dissolve", "kind": "face", "indices": [0,1,2,3,4,5]}}))
        .await;
    assert_eq!(err.code, "invalid_selection");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "separate", "faces": [0,1,2,3,4,5]}}))
        .await;
    assert_eq!(err.code, "invalid_selection");
    let err = e
        .err("mesh_edit", json!({"object": "C", "operation": {"type": "solidify", "thickness": 0}}))
        .await;
    assert_eq!(err.code, "invalid_parameter");
}

#[tokio::test]
async fn modifier_edit_stack_lifecycle() {
    let e = env();
    e.cube("C").await;
    let v = e
        .ok(
            "modifier_edit",
            json!({"object": "C", "action": "add", "modifier": {"type": "subdivision", "levels": 2}}),
        )
        .await;
    assert_eq!(v["object"]["modifiers"][0]["type"], "subdivision");
    assert_eq!(v["object"]["face_count"], 6);
    assert_eq!(v["object"]["evaluated"]["face_count"], 96);
    let v = e
        .ok(
            "modifier_edit",
            json!({"object": "C", "action": "add", "modifier": {"type": "mirror", "axis": "x", "merge": false}, "index": 0}),
        )
        .await;
    assert_eq!(v["object"]["modifiers"][0]["type"], "mirror");
    assert_eq!(v["object"]["evaluated"]["face_count"], 192);
    let v = e
        .ok("modifier_edit", json!({"object": "C", "action": "set", "index": 0, "enabled": false}))
        .await;
    assert_eq!(v["object"]["modifiers"][0]["enabled"], false);
    assert_eq!(v["object"]["evaluated"]["face_count"], 96);
    let v = e
        .ok("modifier_edit", json!({"object": "C", "action": "move", "from": 1, "to": 0}))
        .await;
    assert_eq!(v["object"]["modifiers"][0]["type"], "subdivision");
    let v = e
        .ok("modifier_edit", json!({"object": "C", "action": "remove", "index": 1}))
        .await;
    assert_eq!(v["object"]["modifiers"].as_array().unwrap().len(), 1);
    let v = e
        .ok("modifier_edit", json!({"object": "C", "action": "apply"}))
        .await;
    assert_eq!(v["object"]["face_count"], 96);
    assert!(v["object"].get("modifiers").is_none());
    let labels = e.history_labels().await;
    assert!(labels.contains(&"AI: Apply all modifiers on 'C'".to_string()), "{labels:?}");
    e.ok("undo", json!({})).await;
    assert_eq!(e.object("C").await["face_count"], 6);
}

#[tokio::test]
async fn modifier_edit_errors() {
    let e = env();
    e.cube("C").await;
    let err = e
        .err("modifier_edit", json!({"object": "C", "action": "remove", "index": 0}))
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e
        .err(
            "modifier_edit",
            json!({"object": "C", "action": "add", "modifier": {"type": "array", "count": 0}}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");
    let err = e
        .err(
            "modifier_edit",
            json!({"object": "C", "action": "add", "modifier": {"type": "warp"}}),
        )
        .await;
    assert_eq!(err.code, "invalid_arguments");
    let err = e
        .err("modifier_edit", json!({"object": "Nope", "action": "apply"}))
        .await;
    assert_eq!(err.code, "object_not_found");
    let err = e.err("modifier_edit", json!({"object": "C", "action": "apply"})).await;
    assert_eq!(err.code, "invalid_selection");
}

#[tokio::test]
async fn object_shading_and_join() {
    let e = env();
    e.cube("A").await;
    e.cube("B").await;
    let v = e
        .ok(
            "object_shading",
            json!({"objects": ["A"], "smooth": true, "auto_smooth_angle_deg": 30}),
        )
        .await;
    assert_eq!(v["objects"][0]["shade_smooth"], true);
    assert_eq!(v["objects"][0]["auto_smooth_angle_deg"], 30.0);
    let err = e
        .err(
            "object_shading",
            json!({"objects": ["A"], "smooth": true, "auto_smooth_angle_deg": -1}),
        )
        .await;
    assert_eq!(err.code, "invalid_parameter");

    e.ok(
        "object_transform",
        json!({"objects": ["B"], "translation": [3, 0, 0]}),
    )
    .await;
    let v = e
        .ok("object_join", json!({"objects": ["A", "B"], "into": "A"}))
        .await;
    assert_eq!(v["objects"][0]["face_count"], 12);
    assert_eq!(v["joined"], json!(["B"]));
    let s = e.scene().await;
    assert_eq!(s["objects"].as_array().unwrap().len(), 1);
    let err = e.err("object_join", json!({"objects": ["A"]})).await;
    assert_eq!(err.code, "invalid_selection");
}

#[tokio::test]
async fn mesh_measure_reports_world_dimensions_area_volume() {
    let e = env();
    e.cube("C").await;
    e.ok(
        "object_transform",
        json!({"objects": ["C"], "mode": "absolute", "scale": [1, 2, 0.5]}),
    )
    .await;
    let v = e.ok("mesh_measure", json!({"objects": ["C"]})).await;
    let o = &v["objects"][0];
    assert_eq!(o["dimensions"], json!([2.0, 4.0, 1.0]));
    assert_eq!(o["volume"], 8.0);
    assert_eq!(o["surface_area"], 2.0 * (8.0 + 2.0 + 4.0));
    assert_eq!(o["closed"], true);
    // With a mirror modifier the evaluated mesh is measured by default.
    e.ok(
        "modifier_edit",
        json!({"object": "C", "action": "add", "modifier": {"type": "array", "count": 2, "offset": [3, 0, 0]}}),
    )
    .await;
    let v = e.ok("mesh_measure", json!({"objects": ["C"]})).await;
    assert_eq!(v["objects"][0]["volume"], 16.0);
    assert_eq!(v["objects"][0]["evaluated"], true);
    let v = e
        .ok("mesh_measure", json!({"objects": ["C"], "base_mesh": true}))
        .await;
    assert_eq!(v["objects"][0]["volume"], 8.0);
    let err = e.err("mesh_measure", json!({"objects": ["Nope"]})).await;
    assert_eq!(err.code, "object_not_found");
}

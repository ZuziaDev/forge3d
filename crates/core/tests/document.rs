mod common;

use std::collections::BTreeSet;

use common::*;
use forge3d_core::prelude::*;
use uuid::Uuid;

/// A document with a cube "A" (2x2x2), a uv sphere "B" at x=1 and an empty "G".
fn setup() -> (Document, Uuid, Uuid, Uuid) {
    let mut doc = Document::new();
    let a = doc
        .execute_spec(
            spec(r#"{"op":"create_primitive","primitive":{"type":"cube"},"name":"A"}"#),
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let b = doc
        .execute_spec(
            spec(r#"{"op":"create_primitive","primitive":{"type":"uv_sphere","segments":12,"rings":6},"name":"B","translation":[1,0,0]}"#),
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let g = doc
        .execute_spec(spec(r#"{"op":"create_empty","name":"G"}"#), Origin::Gui)
        .unwrap()
        .created[0]
        .id;
    doc.execute_spec(
        spec(&format!(r#"{{"op":"select_objects","objects":["{a}"]}}"#)),
        Origin::Gui,
    )
    .unwrap();
    doc.clear_history();
    (doc, a, b, g)
}

/// Execute, then check undo restores the exact prior state and redo the exact
/// post state (twice).
#[track_caller]
fn roundtrip(doc: &mut Document, json: &str) -> ChangeSummary {
    let before = doc.state();
    let summary = doc
        .execute_spec(spec(json), Origin::Cli)
        .unwrap_or_else(|e| panic!("{json}: {e} ({})", e.hint()));
    let after = doc.state();
    for _ in 0..2 {
        doc.undo().unwrap();
        assert!(
            doc.state() == before,
            "undo did not restore state for {json}"
        );
        doc.redo().unwrap();
        assert!(
            doc.state() == after,
            "redo did not reproduce state for {json}"
        );
    }
    summary
}

fn op_of(json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["op"].as_str().unwrap().to_string()
}

#[test]
fn undo_redo_every_command() {
    let (mut doc, a, b, g) = setup();
    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut run = |doc: &mut Document, json: String| {
        covered.insert(op_of(&json));
        roundtrip(doc, &json)
    };

    // creation
    run(&mut doc, r#"{"op":"create_primitive","primitive":{"type":"torus"},"translation":[0,3,0],"rotation_euler_deg":[90,0,0],"scale":[1,2,1]}"#.into());
    run(&mut doc, r#"{"op":"create_mesh","name":"Tri","mesh":{"positions":[[0,0,0],[1,0,0],[0,1,0]],"faces":[[0,1,2]]}}"#.into());
    run(&mut doc, r#"{"op":"create_empty","name":"Group"}"#.into());
    // transforms
    run(
        &mut doc,
        format!(
            r#"{{"op":"set_transform","objects":["{a}"],"translation":[1,2,3],"rotation_euler_deg":[0,0,45]}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"translate","objects":["{a}","{b}"],"offset":[0,0,1]}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"translate","objects":["{a}"],"offset":[1,0,0],"space":"local"}}"#),
    );
    run(
        &mut doc,
        format!(
            r#"{{"op":"rotate","objects":["{b}"],"axis":[0,0,1],"angle_deg":30,"pivot":[0,0,0]}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"scale","objects":["{a}"],"factor":[2,1,1]}}"#),
    );
    // object management
    run(
        &mut doc,
        format!(r#"{{"op":"reparent","object":"{b}","parent":"{g}"}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"duplicate_objects","objects":["{g}"],"offset":[0,5,0]}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"rename_object","object":"{a}","name":"Box"}}"#),
    );
    run(
        &mut doc,
        format!(
            r#"{{"op":"set_material","objects":["{a}"],"material":{{"name":"Red","base_color":[1,0,0,1],"metallic":0.5,"roughness":0.3}}}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"set_visibility","objects":["{b}"],"visible":false}}"#),
    );
    // selection
    run(&mut doc, r#"{"op":"set_select_mode","mode":"face"}"#.into());
    run(
        &mut doc,
        format!(r#"{{"op":"select_objects","objects":["{a}"],"action":"set"}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"set_active_object","object":"{a}"}}"#),
    );
    run(&mut doc, format!(r#"{{"op":"select_all","object":"{a}"}}"#));
    run(
        &mut doc,
        format!(r#"{{"op":"select_none","object":"{a}"}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"select_elements","object":"{a}","indices":[1]}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"select_invert","object":"{a}"}}"#),
    );
    run(
        &mut doc,
        format!(
            r#"{{"op":"select_by_direction","object":"{a}","direction":[0,0,1],"space":"local"}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"select_by_normal","object":"{a}","reference":1,"max_angle_deg":5}}"#),
    );
    run(
        &mut doc,
        format!(
            r#"{{"op":"select_in_box","object":"{a}","kind":"vertex","min":[-2,-2,0],"max":[2,2,2],"space":"local","action":"add"}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"select_edge_loop","object":"{a}","edge":0,"ring":true}}"#),
    );
    // mesh ops (explicit elements and selection-driven)
    run(
        &mut doc,
        format!(r#"{{"op":"extrude","object":"{a}","faces":[1],"distance":0.5}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"inset","object":"{a}","thickness":0.2,"depth":0.1}}"#),
    ); // uses extruded selection
    run(
        &mut doc,
        format!(
            r#"{{"op":"transform_elements","object":"{a}","translation":[0,0,0.3],"scale":[0.5,0.5,1]}}"#
        ),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"bevel","object":"{a}","edges":[0],"width":0.05}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"loop_cut","object":"{a}","edge":1,"cuts":2}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"subdivide","object":"{a}","levels":1}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"merge_vertices","object":"{a}","vertices":[0,1],"mode":"center"}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"merge_vertices","object":"{a}","mode":"distance","threshold":0.001}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"delete_elements","object":"{a}","kind":"face","indices":[0,1]}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"mirror","object":"{a}","axis":"x"}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"flip_normals","object":"{a}","faces":[0]}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"recalculate_normals","object":"{a}"}}"#),
    );
    run(
        &mut doc,
        format!(
            r#"{{"op":"set_mesh_data","object":"{a}","mesh":{{"positions":[[0,0,0],[1,0,0],[1,1,0],[0,1,0]],"faces":[[0,1,2,3]]}}}}"#
        ),
    );
    // boolean between two fresh cubes
    let c1 = doc
        .execute_spec(
            spec(r#"{"op":"create_primitive","primitive":{"type":"cube"},"name":"C1"}"#),
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let c2 = doc
        .execute_spec(spec(r#"{"op":"create_primitive","primitive":{"type":"cube"},"name":"C2","translation":[1,1,1]}"#), Origin::Gui)
        .unwrap()
        .created[0]
        .id;
    let s = run(
        &mut doc,
        format!(r#"{{"op":"boolean","object":"{c1}","other":"{c2}","operation":"difference"}}"#),
    );
    assert_eq!(s.deleted.len(), 1);
    assert_approx(doc.scene.mesh(c1).unwrap().signed_volume(), 7.0, 1e-3);
    // batch + deletes
    run(
        &mut doc,
        r#"{"op":"batch","description":"Make tower","commands":[
        {"op":"create_primitive","primitive":{"type":"cylinder"}},
        {"op":"extrude","faces":[8],"distance":1.0},
        {"op":"scale","factor":[1,1,2]}]}"#
            .to_string(),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"delete_objects","objects":["{g}"],"keep_children":true}}"#),
    );
    run(
        &mut doc,
        format!(r#"{{"op":"delete_objects","objects":["{c1}"]}}"#),
    );

    // Every CommandSpec variant in the JSON schema must have been exercised.
    let schema = CommandSpec::json_schema().to_string();
    let all: BTreeSet<String> = schema_ops(&schema);
    assert!(all.len() >= 38, "{all:?}");
    let missing: Vec<_> = all.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "commands without undo/redo test: {missing:?}"
    );
}

/// Collect `"op": {"const": "..."}` / enum values from the schema.
fn schema_ops(schema: &str) -> BTreeSet<String> {
    let v: serde_json::Value = serde_json::from_str(schema).unwrap();
    let mut out = BTreeSet::new();
    let variants = v["oneOf"].as_array().expect("oneOf");
    for var in variants {
        let op = &var["properties"]["op"];
        if let Some(c) = op["const"].as_str() {
            out.insert(c.to_string());
        } else if let Some(e) = op["enum"].as_array() {
            out.insert(e[0].as_str().unwrap().to_string());
        }
    }
    out
}

#[test]
fn batch_rolls_back_on_failure() {
    let (mut doc, a, _, _) = setup();
    let before = doc.state();
    let err = doc
        .execute_spec(
            spec(&format!(
                r#"{{"op":"batch","commands":[
                {{"op":"create_primitive","primitive":{{"type":"cube"}}}},
                {{"op":"translate","objects":["{a}"],"offset":[5,0,0]}},
                {{"op":"extrude","object":"{a}","faces":[1],"distance":1}},
                {{"op":"extrude","object":"{a}","faces":[999]}}]}}"#
            )),
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(err.code(), "batch_failed");
    match &err {
        CoreError::BatchFailed { index, source } => {
            assert_eq!(*index, 3);
            assert_eq!(source.code(), "element_out_of_range");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(
        doc.state() == before,
        "batch must be rolled back completely"
    );
    assert!(doc.history_list().is_empty());
    assert!(!doc.can_undo());

    // nested batch failure also rolls back
    let err = doc
        .execute_spec(
            spec(r#"{"op":"batch","commands":[
                {"op":"create_empty"},
                {"op":"batch","commands":[{"op":"create_empty"},{"op":"rename_object","name":""}]}]}"#),
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(err.code(), "batch_failed");
    assert!(doc.state() == before);

    // a successful batch is one history entry
    let s = doc
        .execute_spec(
            spec(r#"{"op":"batch","description":"two things","commands":[{"op":"create_empty"},{"op":"create_empty"}]}"#),
            Origin::Ai,
        )
        .unwrap();
    assert_eq!(s.created.len(), 2);
    assert_eq!(doc.history_list().len(), 1);
    assert_eq!(doc.history_list()[0].label, "AI: two things");
    doc.undo().unwrap();
    assert!(doc.state() == before);
}

#[test]
fn history_labels_and_flags() {
    let (mut doc, a, _, _) = setup();
    doc.execute_spec(
        spec(&format!(r#"{{"op":"extrude","object":"{a}","faces":[1]}}"#)),
        Origin::Ai,
    )
    .unwrap();
    doc.execute_spec(
        spec(&format!(
            r#"{{"op":"rename_object","object":"{a}","name":"Hero"}}"#
        )),
        Origin::Gui,
    )
    .unwrap();
    doc.execute_spec(spec(r#"{"op":"create_empty"}"#), Origin::Cli)
        .unwrap();
    let h = doc.history_list();
    assert_eq!(h.len(), 3);
    assert_eq!(h[0].label, "AI: Extrude 1 face on 'A'");
    assert_eq!(h[0].origin, Origin::Ai);
    assert_eq!(h[1].label, "Rename 'A' to 'Hero'");
    assert_eq!(h[2].label, "CLI: Create empty 'Empty'");
    assert!(h.iter().all(|e| !e.undone));
    assert!(h[0].seq < h[1].seq && h[1].seq < h[2].seq);

    doc.undo().unwrap();
    let u = doc.undo().unwrap();
    assert_eq!(u.label, "Rename 'A' to 'Hero'");
    let h = doc.history_list();
    assert_eq!(
        h.iter().map(|e| e.undone).collect::<Vec<_>>(),
        vec![false, true, true]
    );
    // a new command discards the redo stack
    doc.execute_spec(spec(r#"{"op":"create_empty","name":"X"}"#), Origin::Gui)
        .unwrap();
    assert_eq!(doc.history_list().len(), 2);
    assert_eq!(doc.redo().unwrap_err().code(), "nothing_to_redo");
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.undo().unwrap_err().code(), "nothing_to_undo");
}

#[test]
fn failed_command_changes_nothing() {
    let (mut doc, a, _, g) = setup();
    let before = doc.state();
    for json in [
        format!(r#"{{"op":"extrude","object":"{g}"}}"#),
        format!(r#"{{"op":"extrude","object":"{a}"}}"#),
        format!(r#"{{"op":"bevel","object":"{a}","edges":[0],"width":5}}"#),
        format!(r#"{{"op":"loop_cut","object":"{a}","edge":100}}"#),
        format!(
            r#"{{"op":"translate","objects":["{}"],"offset":[1,0,0]}}"#,
            Uuid::new_v4()
        ),
        r#"{"op":"subdivide","levels":9}"#.to_string(),
    ] {
        let e = doc.execute_spec(spec(&json), Origin::Ai).unwrap_err();
        assert!(!e.code().is_empty() && !e.hint().is_empty());
        assert!(doc.state() == before, "{json}");
    }
    assert!(doc.history_list().is_empty());
    let e = doc
        .execute_spec(
            spec(&format!(r#"{{"op":"extrude","object":"{g}"}}"#)),
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(e.code(), "node_has_no_mesh");
    let e = doc
        .execute_spec(
            spec(&format!(r#"{{"op":"extrude","object":"{a}"}}"#)),
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(e.code(), "empty_selection");
}

#[test]
fn json_errors_are_invalid_command() {
    let mut doc = Document::new();
    for bad in [
        r#"{"op":"extrude","distanse":1}"#,
        r#"{"op":"no_such_op"}"#,
        r#"not json"#,
        r#"{"op":"create_primitive","primitive":{"type":"cube","size":"big"}}"#,
    ] {
        let e = doc.execute_json(bad, Origin::Ai).unwrap_err();
        assert_eq!(e.code(), "invalid_command", "{bad}");
        let r = e.report();
        assert_eq!(r.code, "invalid_command");
        assert!(!r.hint.is_empty());
    }
}

#[test]
fn spec_serde_roundtrip_and_schema() {
    let s = CommandSpec::Extrude {
        object: None,
        faces: Some(vec![1, 2]),
        distance: 0.5,
        offset: None,
    };
    let json = s.to_json();
    assert!(json.contains(r#""op":"extrude""#));
    assert_eq!(CommandSpec::from_json(&json).unwrap(), s);
    assert_eq!(s.op_name(), "extrude");
    let schema = CommandSpec::json_schema();
    let text = schema.to_string();
    for op in [
        "create_primitive",
        "boolean",
        "select_by_direction",
        "batch",
        "loop_cut",
    ] {
        assert!(text.contains(op), "schema lacks {op}");
    }
}

#[test]
fn create_select_extrude_by_direction_workflow() {
    let mut doc = Document::new();
    let id = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
            Origin::Ai,
        )
        .unwrap()
        .created[0]
        .id;
    assert_eq!(doc.selection.active(), Some(id));
    doc.execute_json(r#"{"op":"set_select_mode","mode":"face"}"#, Origin::Ai)
        .unwrap();
    doc.execute_json(
        r#"{"op":"select_by_direction","direction":[0,0,1],"max_angle_deg":10}"#,
        Origin::Ai,
    )
    .unwrap();
    assert_eq!(
        doc.selection
            .elements(id)
            .unwrap()
            .faces
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![1]
    );
    let s = doc
        .execute_json(r#"{"op":"extrude","distance":1}"#, Origin::Ai)
        .unwrap();
    assert_eq!(s.modified[0].vertex_count, 12);
    assert_eq!(s.modified[0].face_count, 10);
    assert_closed_manifold(doc.scene.mesh(id).unwrap());
    // extruded face stays selected, so a second extrude continues the column
    doc.execute_json(r#"{"op":"extrude","distance":1}"#, Origin::Ai)
        .unwrap();
    let m = doc.scene.mesh(id).unwrap();
    assert_counts(m, 16, 28, 14);
    assert_approx(m.bounding_box().unwrap().max.z, 3.0, 1e-5);
    doc.execute_json(r#"{"op":"delete_objects"}"#, Origin::Ai)
        .unwrap();
    assert!(doc.scene.is_empty());
    assert!(doc.selection.objects().is_empty());
}

#[test]
fn change_summary_merge() {
    let mut doc = Document::new();
    let s = doc
        .execute_json(
            r#"{"op":"batch","commands":[
                {"op":"create_primitive","primitive":{"type":"cube"}},
                {"op":"extrude","faces":[1]},
                {"op":"create_empty"},
                {"op":"delete_objects"}]}"#,
            Origin::Gui,
        )
        .unwrap();
    // the empty was created and deleted -> net effect: only the cube, with final counts
    assert_eq!(s.created.len(), 1);
    assert_eq!(s.created[0].vertex_count, 12);
    assert!(s.deleted.is_empty() && s.modified.is_empty());
    assert!(s.selection_changed);
}

#[test]
fn history_limit() {
    let mut doc = Document::new();
    doc.set_history_limit(Some(3));
    for _ in 0..5 {
        doc.execute_json(r#"{"op":"create_empty"}"#, Origin::Gui)
            .unwrap();
    }
    assert_eq!(doc.history_list().len(), 3);
    for _ in 0..3 {
        doc.undo().unwrap();
    }
    assert_eq!(doc.scene.len(), 2);
    assert!(doc.undo().is_err());
}

#[test]
fn document_state_serializes() {
    let (mut doc, a, b, _) = setup();
    doc.execute_spec(
        spec(&format!(
            r#"{{"op":"reparent","object":"{b}","parent":"{a}"}}"#
        )),
        Origin::Gui,
    )
    .unwrap();
    let json = serde_json::to_string(&doc.state()).unwrap();
    let back: DocumentState = serde_json::from_str(&json).unwrap();
    let doc2 = Document::from_state(back);
    assert_eq!(doc2.scene.traverse(), doc.scene.traverse());
    assert_eq!(doc2.scene.node(b).unwrap().parent(), Some(a));
    assert_eq!(doc2.scene.mesh(a).unwrap(), doc.scene.mesh(a).unwrap());
    assert!(doc2
        .scene
        .world_transform(b)
        .unwrap()
        .abs_diff_eq(doc.scene.world_transform(b).unwrap(), 1e-6));
}

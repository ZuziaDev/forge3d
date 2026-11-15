mod common;

use common::*;
use forge3d_core::prelude::*;
use forge3d_core::primitives as p;
use forge3d_core::selection as sel;
use glam::{Affine3A, Quat, Vec3};

#[test]
fn parenting_cycles_are_rejected() {
    let mut s = Scene::new();
    let a = s.add_node(Node::new("a"), None).unwrap();
    let b = s.add_node(Node::new("b"), Some(a)).unwrap();
    let c = s.add_node(Node::new("c"), Some(b)).unwrap();
    let before = s.clone();
    for (child, parent) in [(a, c), (a, b), (a, a), (b, c)] {
        let e = s.set_parent(child, Some(parent), true, None).unwrap_err();
        assert_eq!(e.code(), "hierarchy_cycle");
        assert_eq!(s, before);
    }
    // valid moves
    s.set_parent(c, None, true, None).unwrap();
    s.set_parent(a, Some(c), true, None).unwrap();
    assert!(s.is_ancestor(c, b));
    s.check_hierarchy().unwrap();

    // through the command layer
    let mut doc = Document::new();
    let x = doc
        .execute_json(r#"{"op":"create_empty","name":"x"}"#, Origin::Ai)
        .unwrap()
        .created[0]
        .id;
    let y = doc
        .execute_json(
            &format!(r#"{{"op":"create_empty","name":"y","parent":"{x}"}}"#),
            Origin::Ai,
        )
        .unwrap()
        .created[0]
        .id;
    let before = doc.state();
    let e = doc
        .execute_json(
            &format!(r#"{{"op":"reparent","object":"{x}","parent":"{y}"}}"#),
            Origin::Ai,
        )
        .unwrap_err();
    assert_eq!(e.code(), "hierarchy_cycle");
    assert!(doc.state() == before);
    assert_eq!(doc.history_list().len(), 2);
}

#[test]
fn world_transforms_and_reparenting() {
    let mut s = Scene::new();
    let parent = s
        .add_node(
            Node::new("p").with_transform(Affine3A::from_scale_rotation_translation(
                Vec3::splat(2.0),
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                Vec3::new(1.0, 0.0, 0.0),
            )),
            None,
        )
        .unwrap();
    let child = s
        .add_node(
            Node::new("c").with_transform(Affine3A::from_translation(Vec3::X)),
            Some(parent),
        )
        .unwrap();
    let w = s.world_transform(child).unwrap();
    // child at local +X, parent rotates 90° about Z and scales by 2 -> (1, 2, 0)
    assert!(Vec3::from(w.translation).abs_diff_eq(Vec3::new(1.0, 2.0, 0.0), 1e-5));
    s.set_parent(child, None, true, None).unwrap();
    assert!(s.world_transform(child).unwrap().abs_diff_eq(w, 1e-5));
    s.set_parent(child, Some(parent), false, Some(0)).unwrap();
    assert_eq!(s.node(parent).unwrap().children(), &[child]);
    let bb = s.world_bounding_box(child).unwrap();
    assert!(bb.is_none());
}

#[test]
fn delete_keep_children_and_duplicate_subtree() {
    let mut doc = Document::new();
    let g = doc
        .execute_json(
            r#"{"op":"create_empty","name":"G","translation":[5,0,0]}"#,
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let c = doc
        .execute_json(
            &format!(r#"{{"op":"create_primitive","primitive":{{"type":"cube"}},"parent":"{g}","translation":[1,0,0]}}"#),
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    let s = doc
        .execute_json(
            &format!(r#"{{"op":"duplicate_objects","objects":["{g}"]}}"#),
            Origin::Gui,
        )
        .unwrap();
    assert_eq!(s.created.len(), 2);
    let names: Vec<&str> = s.created.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["G.001", "Cube.001"]);
    assert_eq!(doc.scene.len(), 4);

    let world_c = doc.scene.world_transform(c).unwrap();
    let s = doc
        .execute_json(
            &format!(r#"{{"op":"delete_objects","objects":["{g}"],"keep_children":true}}"#),
            Origin::Gui,
        )
        .unwrap();
    assert_eq!(s.deleted.len(), 1);
    assert!(doc.scene.node(c).unwrap().parent().is_none());
    assert!(doc
        .scene
        .world_transform(c)
        .unwrap()
        .abs_diff_eq(world_c, 1e-5));
    doc.undo().unwrap();
    assert_eq!(doc.scene.node(c).unwrap().parent(), Some(g));

    let s = doc
        .execute_json(
            &format!(r#"{{"op":"delete_objects","objects":["{g}"]}}"#),
            Origin::Gui,
        )
        .unwrap();
    assert_eq!(s.deleted.len(), 2);
    assert!(!doc.scene.contains(c));
}

#[test]
fn selection_by_direction_local_and_world() {
    let m = cube(2.0);
    let up = sel::elements_facing(&m, ElementKind::Face, Vec3::Z, 10.0, None).unwrap();
    assert_eq!(up, vec![1]);
    // 46° picks the 3 faces around a diagonal direction? No: only faces within angle.
    let diag =
        sel::elements_facing(&m, ElementKind::Face, Vec3::new(1.0, 0.0, 1.0), 46.0, None).unwrap();
    assert_eq!(diag, vec![1, 5]);
    let verts = sel::elements_facing(&m, ElementKind::Vertex, Vec3::Z, 60.0, None).unwrap();
    assert_eq!(verts, vec![4, 5, 6, 7]);
    let edges = sel::elements_facing(&m, ElementKind::Edge, Vec3::Z, 50.0, None).unwrap();
    assert_eq!(edges.len(), 4);

    // world space with the object rotated +90° about X: local +Y faces world +Z
    let mut doc = Document::new();
    let id = doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"},"rotation_euler_deg":[90,0,0]}"#,
        Origin::Ai,
    ).unwrap().created[0].id;
    doc.execute_json(
        r#"{"op":"select_by_direction","kind":"face","direction":[0,0,1],"max_angle_deg":5}"#,
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
        vec![3]
    );
    doc.execute_json(
        r#"{"op":"select_by_direction","kind":"face","direction":[0,0,1],"max_angle_deg":5,"space":"local","action":"add"}"#,
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
        vec![1, 3]
    );
    assert!(sel::elements_facing(&m, ElementKind::Face, Vec3::ZERO, 10.0, None).is_err());
}

#[test]
fn selection_by_normal_box_all_none_invert() {
    let s = p::uv_sphere(1.0, 16, 8).unwrap();
    let top_face = forge3d_core::mesh::FaceId(0);
    let similar = sel::elements_similar_normal(&s, ElementKind::Face, top_face.0, 30.0).unwrap();
    assert!(similar.contains(&0) && similar.len() >= 16 && similar.len() < s.face_count());
    for f in &similar {
        assert!(s.face_normal(FaceId(*f)).dot(s.face_normal(top_face)) > 0.86);
    }

    let m = cube(2.0);
    let b = Aabb::new(Vec3::new(-2.0, -2.0, 0.0), Vec3::splat(2.0));
    let id = Affine3A::IDENTITY;
    assert_eq!(
        sel::elements_in_box(&m, ElementKind::Vertex, &b, &id),
        vec![4, 5, 6, 7]
    );
    assert_eq!(
        sel::elements_in_box(&m, ElementKind::Face, &b, &id),
        vec![1]
    );
    assert_eq!(
        sel::elements_in_box(&m, ElementKind::Edge, &b, &id).len(),
        4
    );
    // world-space box with a translated object
    let t = Affine3A::from_translation(Vec3::new(0.0, 0.0, -1.5));
    assert!(sel::elements_in_box(&m, ElementKind::Vertex, &b, &t).is_empty());

    let mut doc = Document::new();
    let obj = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    doc.execute_json(r#"{"op":"set_select_mode","mode":"vertex"}"#, Origin::Gui)
        .unwrap();
    doc.execute_json(r#"{"op":"select_all"}"#, Origin::Gui)
        .unwrap();
    assert_eq!(doc.selection.elements(obj).unwrap().vertices.len(), 8);
    doc.execute_json(
        r#"{"op":"select_elements","indices":[0,1],"action":"remove"}"#,
        Origin::Gui,
    )
    .unwrap();
    doc.execute_json(r#"{"op":"select_invert"}"#, Origin::Gui)
        .unwrap();
    assert_eq!(
        doc.selection
            .elements(obj)
            .unwrap()
            .vertices
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    doc.execute_json(r#"{"op":"select_none"}"#, Origin::Gui)
        .unwrap();
    assert!(doc.selection.elements(obj).is_none());
    let e = doc
        .execute_json(r#"{"op":"select_elements","indices":[99]}"#, Origin::Gui)
        .unwrap_err();
    assert_eq!(e.code(), "element_out_of_range");

    // object mode: all / invert / none act on objects
    doc.execute_json(r#"{"op":"set_select_mode","mode":"object"}"#, Origin::Gui)
        .unwrap();
    let other = doc
        .execute_json(r#"{"op":"create_empty"}"#, Origin::Gui)
        .unwrap()
        .created[0]
        .id;
    doc.execute_json(r#"{"op":"select_all"}"#, Origin::Gui)
        .unwrap();
    assert_eq!(doc.selection.objects().len(), 2);
    doc.execute_json(
        &format!(r#"{{"op":"select_objects","objects":["{other}"],"action":"remove"}}"#),
        Origin::Gui,
    )
    .unwrap();
    doc.execute_json(r#"{"op":"select_invert"}"#, Origin::Gui)
        .unwrap();
    assert_eq!(doc.selection.objects(), &[other]);
    doc.execute_json(r#"{"op":"select_none"}"#, Origin::Gui)
        .unwrap();
    assert!(doc.selection.objects().is_empty());
    let e = doc
        .execute_json(r#"{"op":"select_elements","indices":[0]}"#, Origin::Gui)
        .unwrap_err();
    assert!(matches!(e.code(), "invalid_selection" | "empty_selection"));
}

#[test]
fn edge_loop_selection_and_resolution() {
    let mut doc = Document::new();
    let t = doc
        .execute_json(
            r#"{"op":"create_primitive","primitive":{"type":"torus","major_segments":12,"minor_segments":8}}"#,
            Origin::Gui,
        )
        .unwrap()
        .created[0]
        .id;
    doc.execute_json(r#"{"op":"select_edge_loop","edge":0}"#, Origin::Gui)
        .unwrap();
    let n = doc.selection.elements(t).unwrap().edges.len();
    assert!(n == 12 || n == 8, "{n}");

    // element-kind resolution: vertices -> faces
    let m = cube(2.0);
    let mut es = ElementSelection::default();
    es.apply(ElementKind::Vertex, [4, 5, 6, 7], SelectAction::Set);
    assert_eq!(es.resolved_faces(&m), vec![FaceId(1)]);
    assert_eq!(es.resolved_edges(&m).len(), 4);
    let mut es = ElementSelection::default();
    es.apply(ElementKind::Face, [1], SelectAction::Set);
    assert_eq!(es.resolved_vertices(&m).len(), 4);
    assert_eq!(es.resolved_edges(&m).len(), 4);
}

#[test]
fn material_validation_and_error_codes() {
    let mut doc = Document::new();
    doc.execute_json(
        r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#,
        Origin::Gui,
    )
    .unwrap();
    let e = doc
        .execute_json(
            r#"{"op":"set_material","material":{"base_color":[2,0,0,1]}}"#,
            Origin::Gui,
        )
        .unwrap_err();
    assert_eq!(e.code(), "invalid_parameter");
    doc.execute_json(
        r#"{"op":"set_material","material":{"name":"Gold","metallic":1.0}}"#,
        Origin::Gui,
    )
    .unwrap();
    let id = doc.selection.active().unwrap();
    let mat = doc.scene.node(id).unwrap().material.clone().unwrap();
    assert_eq!(mat.roughness, 0.5);
    assert_eq!(mat.base_color, [0.8, 0.8, 0.8, 1.0]);

    // codes are unique, non-empty snake_case
    let errs = vec![
        CoreError::NodeNotFound(uuid::Uuid::nil()),
        CoreError::NoMesh(uuid::Uuid::nil()),
        CoreError::InvalidSelection(String::new()),
        CoreError::EmptySelection(String::new()),
        CoreError::ElementOutOfRange {
            kind: ElementKind::Face,
            index: 1,
            count: 0,
        },
        CoreError::NonManifold(String::new()),
        CoreError::param("x", "y"),
        CoreError::InvalidMesh(String::new()),
        CoreError::HierarchyCycle {
            child: uuid::Uuid::nil(),
            parent: uuid::Uuid::nil(),
        },
        CoreError::DuplicateNode(uuid::Uuid::nil()),
        CoreError::NothingToUndo,
        CoreError::NothingToRedo,
        CoreError::BooleanFailed(String::new()),
        CoreError::InvalidCommand(String::new()),
        CoreError::CommandState(String::new()),
        CoreError::BatchFailed {
            index: 0,
            source: Box::new(CoreError::NothingToUndo),
        },
    ];
    let codes: std::collections::BTreeSet<&str> = errs.iter().map(|e| e.code()).collect();
    assert_eq!(codes.len(), errs.len());
    for e in &errs {
        assert!(e.code().chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        assert!(!e.hint().is_empty());
        let json = serde_json::to_string(e).unwrap();
        let back: CoreError = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, e);
    }
}

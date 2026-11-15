//! Document hosting (headless + embedded) and resources.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use forge3d_api::resources::{list_resources, read_resource};
use forge3d_api::{DocumentHandle, DocumentHost, Forge3dTools, Workspace};
use forge3d_core::{command::Origin, document::Document};
use serde_json::json;

#[tokio::test]
async fn headless_host_serializes_jobs_from_many_tasks() {
    let (handle, thread) = DocumentHost::spawn_with_thread(Document::new());
    let mut tasks = Vec::new();
    for i in 0..16 {
        let h = handle.clone();
        tasks.push(tokio::spawn(async move {
            h.run(move |doc| {
                doc.execute_json(
                    &format!(
                        r#"{{"op":"create_primitive","primitive":{{"type":"cube"}},"name":"C{i}"}}"#
                    ),
                    Origin::Ai,
                )
                .unwrap();
                doc.scene.len()
            })
            .await
            .unwrap()
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(handle.run(|d| d.scene.len()).await.unwrap(), 16);
    // A panicking job does not kill the owner.
    let err = handle
        .run(|_| -> usize { panic!("boom") })
        .await
        .unwrap_err();
    assert_eq!(err.code, "internal_error");
    assert_eq!(handle.run(|d| d.history_list().len()).await.unwrap(), 16);
    // Blocking variant from a non-async thread.
    let h2 = handle.clone();
    let n = std::thread::spawn(move || h2.run_blocking(|d| d.revision()).unwrap())
        .join()
        .unwrap();
    assert_eq!(n, 16);
    drop(handle);
    let doc = thread.join().unwrap();
    assert_eq!(doc.scene.len(), 16);
}

#[tokio::test]
async fn embedded_channel_with_waker_and_drain() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let (handle, jobs) = DocumentHandle::channel_with_waker(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    let dir = tempfile::tempdir().unwrap();
    let tools = Forge3dTools::new(
        handle.clone(),
        Workspace::new(dir.path()).unwrap(),
        Origin::Ai,
    );

    // The "GUI thread" owns the document and drains jobs each frame.
    let gui = std::thread::spawn(move || {
        let mut doc = Document::new();
        let mut frames = 0;
        let mut ran = 0;
        while ran < 2 && frames < 10_000 {
            ran += jobs.drain(&mut doc);
            frames += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(jobs.pending(), 0);
        doc
    });

    let v = tools
        .call(
            "object_create",
            json!({"primitive": {"type": "cube"}, "name": "Live"}),
        )
        .await
        .unwrap();
    assert_eq!(v["history_entry"]["label"], "AI: Create cube 'Live'");
    let n = handle.run(|d| d.scene.len()).await.unwrap();
    assert_eq!(n, 1);
    let doc = gui.join().unwrap();
    assert_eq!(doc.scene.len(), 1);
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        2,
        "waker called once per queued job"
    );

    // Owner gone → structured error.
    let err = handle.run(|d| d.scene.len()).await.unwrap_err();
    assert_eq!(err.code, "document_unavailable");
}

#[tokio::test]
async fn resources_list_and_read() {
    let e = common::env();
    let id = e.cube("Box").await;
    e.ok("object_create", json!({"name": "Group"})).await;
    let list = list_resources(e.tools.handle()).await.unwrap();
    let uris: Vec<&str> = list.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"forge3d://scene"));
    assert!(uris.contains(&"forge3d://history"));
    assert!(uris.contains(&"forge3d://schema/command"));
    assert!(uris.contains(&format!("forge3d://object/{id}").as_str()));
    assert_eq!(list.len(), 5);

    let scene = read_resource(e.tools.handle(), "forge3d://scene")
        .await
        .unwrap();
    assert_eq!(scene["object_count"], 2);
    let obj = read_resource(e.tools.handle(), &format!("forge3d://object/{id}"))
        .await
        .unwrap();
    assert_eq!(obj["name"], "Box");
    let obj = read_resource(e.tools.handle(), "forge3d://object/Gro%75p")
        .await
        .unwrap();
    assert_eq!(obj["has_mesh"], false);
    let hist = read_resource(e.tools.handle(), "forge3d://history")
        .await
        .unwrap();
    assert_eq!(hist["entries"].as_array().unwrap().len(), 2);
    let schema = read_resource(e.tools.handle(), "forge3d://schema/command")
        .await
        .unwrap();
    assert!(schema.to_string().contains("create_primitive"));

    let err = read_resource(e.tools.handle(), "forge3d://object/Nope")
        .await
        .unwrap_err();
    assert_eq!(err.code, "object_not_found");
    let err = read_resource(e.tools.handle(), "file:///etc/passwd")
        .await
        .unwrap_err();
    assert_eq!(err.code, "resource_not_found");
}

#[tokio::test]
async fn script_runner_supports_tools_and_raw_commands() {
    use forge3d_api::script::{run_script, Script};
    let dir = tempfile::tempdir().unwrap();
    let tools = Forge3dTools::new(
        DocumentHost::spawn(Document::new()),
        Workspace::new(dir.path()).unwrap(),
        Origin::Cli,
    );
    let script = Script::parse(
        r#"[
            {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "A"}},
            {"op": "extrude", "faces": [0], "distance": 0.5},
            {"command": {"op": "subdivide"}, "name": "smooth"},
            {"tool": "file_export", "arguments": {"path": "a.glb"}}
        ]"#,
    )
    .unwrap();
    let mut seen = 0;
    let reports = run_script(&tools, &script, |_| seen += 1).await;
    assert_eq!(seen, 4);
    assert!(reports.iter().all(|r| r.result.is_ok()), "{reports:?}");
    assert_eq!(reports[2].label, "smooth");
    assert_eq!(reports[1].label, "op:extrude");
    assert!(dir.path().join("a.glb").exists());
    let h = tools.call("history_list", json!({})).await.unwrap();
    assert!(h["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["label"].as_str().unwrap().starts_with("CLI: ")));

    // Failure stops the run.
    let script = Script::parse(
        r#"{"steps": [
            {"tool": "object_delete", "arguments": {"objects": ["Missing"]}},
            {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}}}
        ]}"#,
    )
    .unwrap();
    let reports = run_script(&tools, &script, |_| {}).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].to_json()["error"]["code"], "object_not_found");

    // Atomic scripts are one batch.
    let script = Script::parse(
        r#"{"atomic": true, "description": "two cubes", "steps": [
            {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "X"}},
            {"op": "create_primitive", "primitive": {"type": "cube"}, "name": "Y"}
        ]}"#,
    )
    .unwrap();
    let reports = run_script(&tools, &script, |_| {}).await;
    assert_eq!(
        reports[0].result.as_ref().unwrap()["history_entry"]["label"],
        "CLI: two cubes"
    );

    assert!(Script::parse(r#"[{"tool": "x", "args": {}}]"#).is_err());
    assert!(Script::parse(r#"[{"op": "fly"}]"#).is_err());
    assert!(Script::parse("[]").is_err());
    assert!(Script::parse("{").is_err());
}

//! Tests of the `forge3d` binary.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_forge3d"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn run_example_script_exports_saves_and_renders() {
    let dir = tempfile::tempdir().unwrap();
    let glb = dir.path().join("out.glb");
    let obj = dir.path().join("sub/out.obj");
    let png_path = dir.path().join("out.png");
    let f3d = dir.path().join("table.f3d");
    let out = bin()
        .current_dir(dir.path())
        .arg("run")
        .arg(example("table.json"))
        .args([
            "--export",
            glb.to_str().unwrap(),
            "--export",
            obj.to_str().unwrap(),
        ])
        .args(["--save", f3d.to_str().unwrap()])
        .args([
            "--render",
            png_path.to_str().unwrap(),
            "--render-size",
            "320x240",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let summary = &lines.last().unwrap()["summary"];
    assert_eq!(summary["ok"], true);
    assert_eq!(summary["failed"], 0);
    assert!(lines[..lines.len() - 1].iter().all(|l| l["ok"] == true));
    // History labels of the script are CLI-originated.
    assert_eq!(
        lines[1]["result"]["history_entry"]["label"],
        "CLI: Create cube 'Tabletop'"
    );
    let scene = &lines[lines.len() - 2]["result"];
    assert_eq!(scene["object_count"], 8); // Table, Tabletop, 4 legs, Vase, Apple

    assert!(std::fs::metadata(&glb).unwrap().len() > 1000);
    assert!(obj.exists());
    assert!(f3d.exists());
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&png_path).unwrap(),
    ));
    let reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (320, 240));

    // The saved project converts with `forge3d export`.
    let stl = dir.path().join("table.stl");
    let out = bin().arg("export").arg(&f3d).arg(&stl).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stl.exists());
}

#[test]
fn failing_script_exits_non_zero_and_reports_step() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("bad.json");
    std::fs::write(
        &script,
        json!([
            {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "A"}},
            {"tool": "mesh_edit", "arguments": {"object": "A", "operation": {"type": "extrude", "faces": [42]}}},
            {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}}}
        ])
        .to_string(),
    )
    .unwrap();
    let glb = dir.path().join("never.glb");
    let out = bin()
        .current_dir(dir.path())
        .args([
            "run",
            script.to_str().unwrap(),
            "--export",
            glb.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[1]["error"]["code"], "element_out_of_range");
    assert!(lines[1]["error"]["hint"].as_str().unwrap().len() > 10);
    assert_eq!(lines[2]["summary"]["failed"], 1);
    assert!(!glb.exists());

    // Script file problems are reported clearly.
    std::fs::write(&script, "{ not json").unwrap();
    let out = bin()
        .args(["run", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not valid JSON"));
}

#[test]
fn tools_command_prints_schemas() {
    let out = bin().arg("tools").output().unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tools = v.as_array().unwrap();
    assert_eq!(tools.len(), 21);
    assert!(tools.iter().all(|t| t["input_schema"]["type"] == "object"));
}

#[test]
fn mcp_stdio_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = bin()
        .args(["mcp", "--workspace", dir.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(
                stdout.read_line(&mut line).unwrap() > 0,
                "server closed stdout"
            );
            // Every stdout line must be JSON-RPC.
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non JSON-RPC on stdout {line:?}: {e}"));
            assert_eq!(v["jsonrpc"], "2.0");
            if v["id"] == json!(id) {
                return v;
            }
        }
    };

    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "cli-test", "version": "1"}}}),
    );
    let init = recv(1);
    assert_eq!(init["result"]["serverInfo"]["name"], "forge3d");
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let list = recv(2);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 21);

    send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
        "name": "object_create", "arguments": {"primitive": {"type": "uv_sphere"}, "name": "Ball"}}}),
    );
    let call = recv(3);
    assert_eq!(
        call["result"]["structuredContent"]["object"]["name"],
        "Ball"
    );
    assert_eq!(
        call["result"]["structuredContent"]["history_entry"]["label"],
        "AI: Create sphere 'Ball'"
    );

    send(
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
        "name": "file_export", "arguments": {"path": "../outside.glb"}}}),
    );
    let denied = recv(4);
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["code"],
        "path_not_allowed"
    );

    send(
        json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": {"uri": "forge3d://scene"}}),
    );
    let res = recv(5);
    let scene: Value =
        serde_json::from_str(res["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(scene["objects"][0]["name"], "Ball");

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
}

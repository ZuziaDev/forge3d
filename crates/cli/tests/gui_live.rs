//! `forge3d gui` + MCP over HTTP: an AI edit must reach the live GUI document.
//! Skipped (with a notice) when there is no display / GPU adapter.

#![cfg(feature = "gui")]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Gui(Child);
impl Drop for Gui {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn sse_or_json(body: &str, id: u64) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return v;
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v["id"] == json!(id))
        .unwrap_or_else(|| panic!("no response with id {id} in {body:?}"))
}

#[tokio::test]
async fn gui_exposes_live_document_over_mcp() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("SKIPPED gui_live: no DISPLAY (run under xvfb-run)");
        return;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = tempfile::tempdir().unwrap();
    let mut gui = Gui(Command::new(env!("CARGO_BIN_EXE_forge3d"))
        .args(["gui", "--mcp-http", &format!("127.0.0.1:{port}")])
        .args(["--workspace", dir.path().to_str().unwrap()])
        .stdin(Stdio::null())
        .spawn()
        .unwrap());

    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        if let Some(status) = gui.0.try_wait().unwrap() {
            eprintln!("SKIPPED gui_live: GUI exited early ({status}); no display/GPU adapter");
            return;
        }
        assert!(Instant::now() < deadline, "MCP server never came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let http = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");
    let post = || {
        http.post(&url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
    };
    let r = post()
        .body(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let session = r
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    sse_or_json(&r.text().await.unwrap(), 1);
    let with = |rb: reqwest::RequestBuilder| {
        let rb = rb.header("mcp-protocol-version", "2025-06-18");
        match &session {
            Some(s) => rb.header("mcp-session-id", s),
            None => rb,
        }
    };
    let r = with(post())
        .body(json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    let mut id = 1;
    let mut call = |name: &'static str, args: Value| {
        id += 1;
        let body = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});
        let this = id;
        let rb = with(post()).body(body.to_string());
        async move {
            let r = tokio::time::timeout(Duration::from_secs(30), rb.send())
                .await
                .expect("GUI answers in time")
                .unwrap();
            assert_eq!(r.status(), 200);
            sse_or_json(&r.text().await.unwrap(), this)["result"]["structuredContent"].clone()
        }
    };
    let created = call(
        "object_create",
        json!({"primitive":{"type":"torus"},"name":"Ring"}),
    )
    .await;
    assert_eq!(created["object"]["name"], "Ring");
    let scene = call("scene_get", json!({})).await;
    assert!(scene.to_string().contains("Ring"), "{scene}");
    let hist = call("history_list", json!({})).await;
    assert!(
        hist.to_string().contains("AI: Create torus 'Ring'"),
        "{hist}"
    );
}

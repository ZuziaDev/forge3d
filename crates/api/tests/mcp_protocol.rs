//! End-to-end MCP protocol tests: raw JSON-RPC over an in-process stdio-like
//! transport, and Streamable HTTP with bearer-token auth.

mod common;

use forge3d_api::{DocumentHost, Forge3dTools, HttpConfig, Workspace};
use forge3d_core::{command::Origin, document::Document};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn tools(dir: &std::path::Path) -> Forge3dTools {
    Forge3dTools::new(
        DocumentHost::spawn(Document::new()),
        Workspace::new(dir).unwrap(),
        Origin::Ai,
    )
}

struct Client {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    reader: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    next_id: u64,
}

impl Client {
    async fn send(&mut self, msg: Value) {
        let mut line = serde_json::to_string(&msg).unwrap();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.reader.read_line(&mut line),
            )
            .await
            .expect("response in time")
            .unwrap();
            assert!(n > 0, "server closed the stream");
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON line {line:?}: {e}"));
            assert_eq!(v["jsonrpc"], "2.0");
            if v["id"] == json!(id) {
                return v;
            }
            // notifications are ignored
        }
    }
}

#[tokio::test]
async fn mcp_session_over_in_process_transport() {
    let dir = tempfile::tempdir().unwrap();
    let (client_end, server_end) = tokio::io::duplex(1 << 22);
    let (sr, sw) = tokio::io::split(server_end);
    let server = tokio::spawn(forge3d_api::serve_io(tools(dir.path()), sr, sw));
    let (cr, cw) = tokio::io::split(client_end);
    let mut c = Client {
        writer: cw,
        reader: BufReader::new(cr),
        next_id: 0,
    };

    let init = c
        .request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        )
        .await;
    let r = &init["result"];
    assert_eq!(r["protocolVersion"], "2025-06-18");
    assert_eq!(r["serverInfo"]["name"], "forge3d");
    assert!(r["capabilities"]["tools"].is_object());
    assert!(r["capabilities"]["resources"].is_object());
    assert!(r["instructions"].as_str().unwrap().contains("Z is up"));
    c.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;

    let list = c.request("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 21);
    let create = tools.iter().find(|t| t["name"] == "object_create").unwrap();
    assert_eq!(create["inputSchema"]["type"], "object");
    assert_eq!(create["annotations"]["readOnlyHint"], false);
    let render = tools
        .iter()
        .find(|t| t["name"] == "viewport_render")
        .unwrap();
    assert_eq!(render["annotations"]["readOnlyHint"], true);

    let call = c
        .request(
            "tools/call",
            json!({"name": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "Box"}}),
        )
        .await;
    let res = &call["result"];
    assert_ne!(res["isError"], true);
    assert_eq!(res["structuredContent"]["object"]["name"], "Box");
    assert_eq!(
        res["structuredContent"]["history_entry"]["label"],
        "AI: Create cube 'Box'"
    );
    let text: Value = serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["object"]["name"], "Box");

    // Tool errors are results with isError + structured JSON.
    let bad = c
        .request(
            "tools/call",
            json!({"name": "object_delete", "arguments": {"objects": ["Ghost"]}}),
        )
        .await;
    assert_eq!(bad["result"]["isError"], true);
    let err: Value =
        serde_json::from_str(bad["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(err["code"], "object_not_found");
    assert!(err["hint"].as_str().unwrap().contains("scene_get"));
    let bad = c
        .request(
            "tools/call",
            json!({"name": "object_create", "arguments": {"primitive": 5}}),
        )
        .await;
    assert_eq!(bad["result"]["isError"], true);
    assert_eq!(
        bad["result"]["structuredContent"]["code"],
        "invalid_arguments"
    );

    // Unknown tool is a protocol error.
    let unknown = c
        .request("tools/call", json!({"name": "teleport", "arguments": {}}))
        .await;
    assert!(unknown["error"].is_object(), "{unknown}");
    assert_eq!(unknown["error"]["data"]["code"], "unknown_tool");

    // Image content.
    let img = c
        .request(
            "tools/call",
            json!({"name": "viewport_render", "arguments": {"width": 96, "height": 64}}),
        )
        .await;
    let content = img["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/png");
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode(content[0]["data"].as_str().unwrap())
        .unwrap();
    let info = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    assert_eq!((info.info().width, info.info().height), (96, 64));
    assert_eq!(content[1]["type"], "text");

    // Resources.
    let rl = c.request("resources/list", json!({})).await;
    let uris: Vec<&str> = rl["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert!(uris.contains(&"forge3d://scene"));
    assert!(uris.contains(&"forge3d://history"));
    assert!(uris.iter().any(|u| u.starts_with("forge3d://object/")));
    let tl = c.request("resources/templates/list", json!({})).await;
    assert_eq!(
        tl["result"]["resourceTemplates"][0]["uriTemplate"],
        "forge3d://object/{id}"
    );

    let rr = c
        .request("resources/read", json!({"uri": "forge3d://scene"}))
        .await;
    let contents = &rr["result"]["contents"][0];
    assert_eq!(contents["mimeType"], "application/json");
    let scene: Value = serde_json::from_str(contents["text"].as_str().unwrap()).unwrap();
    assert_eq!(scene["objects"][0]["name"], "Box");
    let rr = c
        .request("resources/read", json!({"uri": "forge3d://object/Box"}))
        .await;
    let obj: Value =
        serde_json::from_str(rr["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(obj["face_count"], 6);
    let rr = c
        .request("resources/read", json!({"uri": "forge3d://history"}))
        .await;
    let hist: Value =
        serde_json::from_str(rr["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(hist["entries"][0]["label"], "AI: Create cube 'Box'");
    let rr = c
        .request("resources/read", json!({"uri": "forge3d://nothing"}))
        .await;
    assert!(rr["error"].is_object());

    drop(c);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
}

fn sse_or_json(body: &str, id: u64) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return v;
    }
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                if v["id"] == json!(id) {
                    return v;
                }
            }
        }
    }
    panic!("no JSON-RPC response with id {id} in body {body:?}");
}

#[tokio::test]
async fn streamable_http_with_bearer_token() {
    let dir = tempfile::tempdir().unwrap();
    let server = forge3d_api::start_http(
        tools(dir.path()),
        HttpConfig {
            addr: "127.0.0.1:0".parse().unwrap(),
            token: Some("s3cret-token".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let url = server.url.clone();
    let http = reqwest::Client::new();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}});
    let post = |token: Option<&str>| {
        let mut rb = http
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        if let Some(t) = token {
            rb = rb.header("Authorization", format!("Bearer {t}"));
        }
        rb
    };

    // No token / wrong token → 401 with structured body.
    let r = post(None).body(init.to_string()).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert!(r.headers().get("www-authenticate").is_some());
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(body["code"], "unauthorized");
    let r = post(Some("wrong"))
        .body(init.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Foreign browser origin → rejected.
    let r = post(Some("s3cret-token"))
        .header("Origin", "http://evil.example")
        .body(init.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // Correct token → initialize + tools/call.
    let r = post(Some("s3cret-token"))
        .body(init.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let session = r
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    let v = sse_or_json(&r.text().await.unwrap(), 1);
    assert_eq!(v["result"]["serverInfo"]["name"], "forge3d");

    let with_session = |rb: reqwest::RequestBuilder| match &session {
        Some(s) => rb
            .header("mcp-session-id", s)
            .header("mcp-protocol-version", "2025-06-18"),
        None => rb.header("mcp-protocol-version", "2025-06-18"),
    };
    let r = with_session(post(Some("s3cret-token")))
        .body(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "object_create", "arguments": {"primitive": {"type": "torus"}, "name": "Ring"}}});
    let r = with_session(post(Some("s3cret-token")))
        .body(call.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v = sse_or_json(&r.text().await.unwrap(), 2);
    assert_eq!(v["result"]["structuredContent"]["object"]["name"], "Ring");
    assert_eq!(
        v["result"]["structuredContent"]["history_entry"]["label"],
        "AI: Create torus 'Ring'"
    );

    server.shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(10), server.wait())
        .await
        .expect("server stops")
        .unwrap();
}

#[tokio::test]
async fn non_loopback_without_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let r = forge3d_api::start_http(
        tools(dir.path()),
        HttpConfig {
            addr: "0.0.0.0:0".parse().unwrap(),
            token: None,
            ..Default::default()
        },
    )
    .await;
    assert!(r.is_err());
}

#[test]
fn bearer_compare() {
    use forge3d_api::mcp::bearer_matches;
    assert!(bearer_matches(Some("Bearer abc"), "abc"));
    assert!(!bearer_matches(Some("Bearer abcd"), "abc"));
    assert!(!bearer_matches(Some("Basic abc"), "abc"));
    assert!(!bearer_matches(None, "abc"));
}

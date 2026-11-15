#![allow(dead_code)]

use forge3d_api::{ApiError, DocumentHost, Forge3dTools, Workspace};
use forge3d_core::command::Origin;
use forge3d_core::document::Document;
use serde_json::{json, Value};

pub struct Env {
    pub dir: tempfile::TempDir,
    pub tools: Forge3dTools,
}

pub fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path()).unwrap();
    let tools = Forge3dTools::new(DocumentHost::spawn(Document::new()), ws, Origin::Ai);
    Env { dir, tools }
}

impl Env {
    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        match self.tools.call(tool, args.clone()).await {
            Ok(v) => v,
            Err(e) => panic!("{tool} {args} failed: {}", serde_json::to_string_pretty(&e).unwrap()),
        }
    }

    pub async fn err(&self, tool: &str, args: Value) -> ApiError {
        match self.tools.call(tool, args.clone()).await {
            Ok(v) => panic!("{tool} {args} unexpectedly succeeded: {v}"),
            Err(e) => {
                assert!(!e.code.is_empty() && !e.message.is_empty() && !e.hint.is_empty(), "{e:?}");
                e
            }
        }
    }

    pub async fn cube(&self, name: &str) -> String {
        let v = self
            .ok("object_create", json!({"primitive": {"type": "cube", "size": 2.0}, "name": name}))
            .await;
        v["object"]["id"].as_str().unwrap().to_string()
    }

    pub async fn scene(&self) -> Value {
        self.ok("scene_get", json!({})).await
    }

    pub async fn object(&self, name: &str) -> Value {
        let s = self.scene().await;
        s["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no object {name} in {s}"))
    }

    pub async fn history_labels(&self) -> Vec<String> {
        let h = self.ok("history_list", json!({})).await;
        h["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["label"].as_str().unwrap().to_string())
            .collect()
    }
}

pub fn approx(v: &Value, expected: [f64; 3]) {
    let a: Vec<f64> = v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    for i in 0..3 {
        assert!((a[i] - expected[i]).abs() < 1e-3, "{a:?} != {expected:?}");
    }
}

//! JSON scripts: a list of steps executed through the tool layer.
//!
//! Format — either a bare array of steps, or an object:
//!
//! ```json
//! {
//!   "description": "Build a table",
//!   "atomic": false,
//!   "steps": [
//!     {"tool": "object_create", "arguments": {"primitive": {"type": "cube"}, "name": "Top"}},
//!     {"op": "subdivide", "levels": 1},
//!     {"command": {"op": "set_visibility", "visible": true}}
//!   ]
//! }
//! ```
//!
//! A step is a tool call `{"tool", "arguments"}` (any tool, including
//! queries like `scene_get` and file tools like `file_export`), a raw
//! [`CommandSpec`] (an object with `"op"`), or `{"command": CommandSpec}`.
//! Optional `"name"` on a step labels it in the output. Each step is one
//! undo-history entry recorded with the tools' origin; with `"atomic": true`
//! all steps run as one `batch` (one entry, rolled back on failure; then
//! only batchable tools and commands are allowed).

use forge3d_core::spec::CommandSpec;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::tools::Forge3dTools;

/// One script step.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStep {
    /// A tool call.
    Tool {
        tool: String,
        arguments: Value,
        name: Option<String>,
    },
    /// A raw command.
    Command {
        spec: CommandSpec,
        name: Option<String>,
    },
}

impl ScriptStep {
    /// Short label (`tool` name or `op:<op>`), or the step's `name`.
    pub fn label(&self) -> String {
        match self {
            ScriptStep::Tool { tool, name, .. } => name.clone().unwrap_or_else(|| tool.clone()),
            ScriptStep::Command { spec, name } => {
                name.clone().unwrap_or_else(|| format!("op:{}", spec.op_name()))
            }
        }
    }

    fn to_batch_step(&self) -> Value {
        match self {
            ScriptStep::Tool {
                tool, arguments, ..
            } => json!({"tool": tool, "arguments": arguments}),
            ScriptStep::Command { spec, .. } => json!({"command": spec}),
        }
    }
}

/// A parsed script.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub description: Option<String>,
    pub atomic: bool,
    pub steps: Vec<ScriptStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptObject {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    atomic: bool,
    steps: Vec<Value>,
}

fn parse_step(i: usize, v: Value) -> ApiResult<ScriptStep> {
    let err = |m: String| ApiError::invalid_arguments(format!("script step {i}: {m}"));
    let Value::Object(mut m) = v else {
        return Err(err("a step must be a JSON object".into()));
    };
    let name = match m.remove("name") {
        None => None,
        Some(Value::String(s)) => Some(s),
        Some(_) => return Err(err("`name` must be a string".into())),
    };
    if let Some(tool) = m.remove("tool") {
        let Value::String(tool) = tool else {
            return Err(err("`tool` must be a string".into()));
        };
        let arguments = m.remove("arguments").unwrap_or(Value::Object(Default::default()));
        if let Some(k) = m.keys().next() {
            return Err(err(format!("unknown field `{k}` next to `tool` (put tool parameters inside `arguments`)")));
        }
        return Ok(ScriptStep::Tool {
            tool,
            arguments,
            name,
        });
    }
    let spec_value = match m.remove("command") {
        Some(c) => {
            if let Some(k) = m.keys().next() {
                return Err(err(format!("unknown field `{k}` next to `command`")));
            }
            c
        }
        None if m.contains_key("op") => Value::Object(m),
        None => {
            return Err(err(
                "expected {\"tool\": ..., \"arguments\": {...}}, a CommandSpec with \"op\", or {\"command\": {...}}".into(),
            ))
        }
    };
    let spec: CommandSpec = serde_json::from_value(spec_value)
        .map_err(|e| err(format!("invalid command: {e}")).with_hint("Raw commands follow the CommandSpec JSON Schema (`forge3d tools` / resource forge3d://schema/command); `op` is snake_case."))?;
    Ok(ScriptStep::Command { spec, name })
}

impl Script {
    /// Parse a script from JSON text.
    pub fn parse(text: &str) -> ApiResult<Script> {
        let v: Value = serde_json::from_str(text)
            .map_err(|e| ApiError::invalid_arguments(format!("script is not valid JSON: {e}")))?;
        Script::from_value(v)
    }

    /// Parse a script from a JSON value.
    pub fn from_value(v: Value) -> ApiResult<Script> {
        let (description, atomic, raw) = match v {
            Value::Array(a) => (None, false, a),
            v @ Value::Object(_) => {
                let o: ScriptObject = serde_json::from_value(v).map_err(|e| {
                    ApiError::invalid_arguments(format!("invalid script object: {e}"))
                        .with_hint("A script is an array of steps or {\"description\"?, \"atomic\"?, \"steps\": [...]}.")
                })?;
                (o.description, o.atomic, o.steps)
            }
            _ => {
                return Err(ApiError::invalid_arguments(
                    "a script must be a JSON array of steps or an object with `steps`",
                ))
            }
        };
        let steps = raw
            .into_iter()
            .enumerate()
            .map(|(i, s)| parse_step(i, s))
            .collect::<ApiResult<Vec<_>>>()?;
        if steps.is_empty() {
            return Err(ApiError::invalid_arguments("the script has no steps"));
        }
        Ok(Script {
            description,
            atomic,
            steps,
        })
    }
}

/// Outcome of one executed step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepReport {
    pub index: usize,
    pub label: String,
    pub result: Result<Value, ApiError>,
}

impl StepReport {
    /// JSON line for output: `{"step", "label", "ok", "result" | "error"}`.
    pub fn to_json(&self) -> Value {
        match &self.result {
            Ok(v) => json!({"step": self.index, "label": self.label, "ok": true, "result": v}),
            Err(e) => json!({"step": self.index, "label": self.label, "ok": false, "error": e}),
        }
    }
}

/// Run a script, calling `on_step` after every step. Execution stops at the
/// first failing step; returns all reports.
pub async fn run_script(
    tools: &Forge3dTools,
    script: &Script,
    mut on_step: impl FnMut(&StepReport),
) -> Vec<StepReport> {
    let mut reports = Vec::new();
    if script.atomic {
        let steps: Vec<Value> = script.steps.iter().map(ScriptStep::to_batch_step).collect();
        let mut args = json!({ "steps": steps });
        if let Some(d) = &script.description {
            args["description"] = json!(d);
        }
        let r = StepReport {
            index: 0,
            label: "batch".into(),
            result: tools.call("batch", args).await,
        };
        on_step(&r);
        reports.push(r);
        return reports;
    }
    for (i, step) in script.steps.iter().enumerate() {
        let result = match step {
            ScriptStep::Tool {
                tool, arguments, ..
            } => tools.call_rich(tool, arguments.clone()).await.map(|out| {
                let mut v = out.value;
                if !out.images.is_empty() {
                    v["images_omitted"] = json!(out.images.len());
                }
                v
            }),
            ScriptStep::Command { spec, .. } => tools.execute_spec(spec.clone()).await,
        };
        let r = StepReport {
            index: i,
            label: step.label(),
            result,
        };
        on_step(&r);
        let failed = r.result.is_err();
        reports.push(r);
        if failed {
            break;
        }
    }
    reports
}

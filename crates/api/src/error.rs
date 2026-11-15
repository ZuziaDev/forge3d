//! Structured API errors: every failure surfaces as `{code, message, hint}`
//! (plus optional machine-readable `details`), so an AI agent can read the
//! error and correct its next call.

use forge3d_core::error::CoreError;
use forge3d_io::IoError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Result alias of the API layer.
pub type ApiResult<T> = Result<T, ApiError>;

/// A structured error with a stable `code`, a human-readable `message` and an
/// actionable `hint`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    /// Stable machine-readable code, e.g. `object_not_found`, `invalid_arguments`.
    pub code: String,
    /// What went wrong.
    pub message: String,
    /// How to fix it.
    pub hint: String,
    /// Optional extra data (e.g. `{"index": 2}` for a failing batch step).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    /// Build an error from its parts.
    pub fn new(code: impl Into<String>, message: impl Into<String>, hint: impl Into<String>) -> Self {
        ApiError {
            code: code.into(),
            message: message.into(),
            hint: hint.into(),
            details: None,
        }
    }

    /// Attach machine-readable details.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Replace the hint.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Arguments did not match the tool's input schema.
    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        ApiError::new(
            "invalid_arguments",
            message,
            "Check the arguments against the tool's JSON input schema (tools/list). Unknown fields are rejected; enum values are snake_case.",
        )
    }

    /// No tool with that name.
    pub fn unknown_tool(name: &str, available: &[&str]) -> Self {
        ApiError::new(
            "unknown_tool",
            format!("there is no tool named '{name}'"),
            format!("Use one of: {}.", available.join(", ")),
        )
    }

    /// Path outside the workspace.
    pub fn path_not_allowed(path: &str, workspace: &std::path::Path, why: &str) -> Self {
        ApiError::new(
            "path_not_allowed",
            format!("path '{path}' is not allowed: {why}"),
            format!(
                "File operations are confined to the workspace directory '{}'. Use a path relative to it (e.g. 'models/table.glb') without '..' and without symlinks that lead outside.",
                workspace.display()
            ),
        )
    }

    /// Missing or wrong bearer token.
    pub fn unauthorized() -> Self {
        ApiError::new(
            "unauthorized",
            "missing or invalid bearer token",
            "Send the header `Authorization: Bearer <token>` with the token the server was started with (--token or FORGE3D_TOKEN).",
        )
    }

    /// The document host is gone (thread ended or GUI closed).
    pub fn document_unavailable() -> Self {
        ApiError::new(
            "document_unavailable",
            "the document host is not running",
            "The application owning the document has shut down; restart the forge3d server.",
        )
    }

    /// Internal failure that is not the caller's fault.
    pub fn internal(message: impl Into<String>) -> Self {
        ApiError::new(
            "internal_error",
            message,
            "This is a bug in forge3d; retry once, and if it persists inspect the scene with scene_get and continue with a different approach.",
        )
    }

    /// Serialize to a JSON value.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("ApiError serializes")
    }
}

fn core_hint(e: &CoreError) -> String {
    match e {
        CoreError::NodeNotFound(_) => "Call scene_get to list the objects and their ids; objects can be referenced by id or by unique name. The object may have been deleted or the id mistyped.".into(),
        CoreError::NoMesh(_) => "Target an object that has mesh data (created with object_create or imported), not an empty/group node; scene_get shows has_mesh per object.".into(),
        CoreError::EmptySelection(_) => "Pass the objects/elements explicitly (e.g. `objects`, `faces`, `indices`) or select them first with mesh_select; scene_get shows the current selection and active object.".into(),
        CoreError::ElementOutOfRange { count, .. } => format!(
            "Element indices must be in 0..{count}. Indices change after topology edits (extrude, inset, delete, subdivide...), so call mesh_get or mesh_select again to obtain current indices."
        ),
        CoreError::InvalidSelection(_) => "Select elements of the kind this operation expects (faces for extrude/inset, edges for bevel/loop_cut, vertices for merge) with mesh_select, or pass them explicitly.".into(),
        CoreError::NothingToUndo => "The undo history is empty (see history_list); nothing was changed.".into(),
        CoreError::NothingToRedo => "Nothing to redo: there is no undone entry (see history_list). Executing a new operation clears the redo stack.".into(),
        CoreError::InvalidCommand(_) => "Check the command against the CommandSpec JSON Schema (`op` is required and snake_case; unknown fields are rejected). Prefer the typed tools (object_create, mesh_edit, ...) which have precise schemas.".into(),
        CoreError::BatchFailed { source, .. } => core_hint(source),
        other => other.hint(),
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        let mut err = ApiError::new(e.code(), e.to_string(), core_hint(&e));
        match &e {
            CoreError::BatchFailed { index, source } => {
                err.details = Some(serde_json::json!({
                    "index": index,
                    "step_error": {"code": source.code(), "message": source.to_string()}
                }));
            }
            CoreError::NodeNotFound(id) => {
                err.details = Some(serde_json::json!({ "id": id }));
            }
            _ => {}
        }
        err
    }
}

impl From<IoError> for ApiError {
    fn from(e: IoError) -> Self {
        match e {
            IoError::Core(c) => c.into(),
            IoError::FileNotFound(_) => ApiError::new(
                e.code(),
                e.to_string(),
                "Check the path; relative paths are resolved inside the server's workspace directory.",
            ),
            other => ApiError::new(other.code(), other.to_string(), other.hint()),
        }
    }
}

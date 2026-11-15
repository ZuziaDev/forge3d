//! The native `.f3d` project format.
//!
//! A project is a single JSON document:
//!
//! ```json
//! { "format": "forge3d-project", "version": 1, "generator": "forge3d-io 0.1.0",
//!   "state": { "scene": { ... }, "selection": { ... } } }
//! ```
//!
//! * `format` is a magic string that distinguishes projects from arbitrary JSON.
//! * `version` is an integer that is bumped whenever the layout of `state`
//!   changes incompatibly. Files with a *newer* version than
//!   [`PROJECT_VERSION`] are rejected with [`IoError::VersionMismatch`]; older
//!   versions are upgraded step by step by the functions in `MIGRATIONS`
//!   (`MIGRATIONS[i]` converts version `i + 1` into `i + 2` and operates on the
//!   whole JSON envelope, so it may restructure anything).

use forge3d_core::document::{Document, DocumentState};
use serde_json::{json, Value};

use crate::error::{IoError, IoResult};

/// Magic value of the `format` field.
pub const PROJECT_FORMAT: &str = "forge3d-project";
/// Version written by this build (the newest version it can read).
pub const PROJECT_VERSION: u32 = 1;

/// A function that upgrades a project envelope by exactly one version.
pub(crate) type Migration = fn(Value) -> IoResult<Value>;

/// Upgrade steps; `MIGRATIONS[i]` converts version `i + 1` to `i + 2`.
/// Version 1 is the first release, so there is nothing to migrate yet.
const MIGRATIONS: &[Migration] = &[];

const _: () = assert!(MIGRATIONS.len() + 1 == PROJECT_VERSION as usize);

/// Serialize a document state to `.f3d` bytes.
pub fn state_to_bytes(state: &DocumentState, pretty: bool) -> IoResult<Vec<u8>> {
    let state_value = serde_json::to_value(state)
        .map_err(|e| IoError::InvalidData(format!("cannot serialize project: {e}")))?;
    let envelope = json!({
        "format": PROJECT_FORMAT,
        "version": PROJECT_VERSION,
        "generator": concat!("forge3d-io ", env!("CARGO_PKG_VERSION")),
        "state": state_value,
    });
    if pretty {
        serde_json::to_vec_pretty(&envelope)
    } else {
        serde_json::to_vec(&envelope)
    }
    .map_err(|e| IoError::InvalidData(format!("cannot serialize project: {e}")))
}

/// Upgrade `envelope` (at `version`) to `target` using `migrations`.
pub(crate) fn migrate(
    mut envelope: Value,
    version: u32,
    target: u32,
    migrations: &[Migration],
) -> IoResult<Value> {
    for v in version..target {
        let step = migrations.get((v - 1) as usize).ok_or_else(|| {
            IoError::InvalidProject(format!("no migration from version {v} to {}", v + 1))
        })?;
        envelope = step(envelope)?;
        envelope["version"] = json!(v + 1);
    }
    Ok(envelope)
}

/// Check magic and version of a parsed envelope; returns the version found.
fn check_envelope(envelope: &Value, current: u32) -> IoResult<u32> {
    let obj = envelope.as_object().ok_or_else(|| {
        IoError::InvalidProject("the top-level JSON value is not an object".into())
    })?;
    match obj.get("format").and_then(Value::as_str) {
        Some(PROJECT_FORMAT) => {}
        Some(other) => {
            return Err(IoError::InvalidProject(format!(
                "format is '{other}', expected '{PROJECT_FORMAT}'"
            )))
        }
        None => {
            return Err(IoError::InvalidProject(
                "the 'format' field is missing".into(),
            ))
        }
    }
    let version = obj
        .get("version")
        .ok_or_else(|| IoError::InvalidProject("the 'version' field is missing".into()))?;
    match version.as_u64() {
        Some(v) if (1..=u64::from(current)).contains(&v) => Ok(v as u32),
        _ => Err(IoError::VersionMismatch {
            found: version.to_string(),
            supported: current,
        }),
    }
}

/// Parse `.f3d` bytes into a document state (migrating older versions).
pub fn state_from_bytes(bytes: &[u8]) -> IoResult<DocumentState> {
    state_from_bytes_with(bytes, MIGRATIONS, PROJECT_VERSION)
}

pub(crate) fn state_from_bytes_with(
    bytes: &[u8],
    migrations: &[Migration],
    current: u32,
) -> IoResult<DocumentState> {
    let envelope: Value = serde_json::from_slice(bytes)
        .map_err(|e| IoError::parse("f3d", format!("not valid JSON: {e}")))?;
    let version = check_envelope(&envelope, current)?;
    let envelope = migrate(envelope, version, current, migrations)?;
    let state = envelope
        .get("state")
        .cloned()
        .ok_or_else(|| IoError::InvalidProject("the 'state' field is missing".into()))?;
    serde_json::from_value(state)
        .map_err(|e| IoError::parse("f3d", format!("invalid project state: {e}")))
}

/// Serialize a document (scene, materials, hierarchy, selection; not the undo
/// history) to `.f3d` bytes.
pub fn project_to_bytes(doc: &Document) -> IoResult<Vec<u8>> {
    state_to_bytes(&doc.state(), false)
}

/// Parse `.f3d` bytes into a fresh document with empty history.
pub fn project_from_bytes(bytes: &[u8]) -> IoResult<Document> {
    Ok(Document::from_state(state_from_bytes(bytes)?))
}

/// Save a document atomically (temp file + rename) to `path`.
pub fn save_project(doc: &Document, path: impl AsRef<std::path::Path>) -> IoResult<()> {
    crate::util::write_atomic(path.as_ref(), &project_to_bytes(doc)?)
}

/// Load a project file into a fresh document with empty history.
pub fn load_project(path: impl AsRef<std::path::Path>) -> IoResult<Document> {
    project_from_bytes(&crate::util::read_file(path.as_ref())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bump(mut v: Value) -> IoResult<Value> {
        v["state"]["migrated_by"] = json!("m");
        Ok(v)
    }

    #[test]
    fn migration_chain_runs_in_order() {
        let env = json!({"format": PROJECT_FORMAT, "version": 1, "state": {}});
        let out = migrate(env, 1, 3, &[bump, bump]).unwrap();
        assert_eq!(out["version"], 3);
        assert_eq!(out["state"]["migrated_by"], "m");
    }

    #[test]
    fn missing_migration_is_an_error() {
        let env = json!({"format": PROJECT_FORMAT, "version": 1, "state": {}});
        let e = migrate(env, 1, 2, &[]).unwrap_err();
        assert_eq!(e.code(), "invalid_project");
    }

    #[test]
    fn older_versions_are_migrated_on_load() {
        let state = serde_json::to_value(DocumentState::default()).unwrap();
        let env = json!({"format": PROJECT_FORMAT, "version": 1, "state": state});
        let bytes = serde_json::to_vec(&env).unwrap();
        let loaded = state_from_bytes_with(&bytes, &[|v| Ok(v)], 2).unwrap();
        assert_eq!(loaded, DocumentState::default());
    }
}

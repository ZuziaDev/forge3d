//! Read-only JSON resources (transport independent):
//!
//! * `forge3d://scene` – the scene tree (same as the `scene_get` tool),
//! * `forge3d://object/{id}` – one object (id or unique name), template,
//! * `forge3d://history` – the undo history,
//! * `forge3d://schema/command` – JSON Schema of raw CommandSpec (batch `command` steps).

use forge3d_core::spec::CommandSpec;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::host::DocumentHandle;
use crate::info::{object_info, resolve_ref, scene_info};

/// URI of the scene resource.
pub const SCENE_URI: &str = "forge3d://scene";
/// URI of the history resource.
pub const HISTORY_URI: &str = "forge3d://history";
/// URI of the CommandSpec schema resource.
pub const COMMAND_SCHEMA_URI: &str = "forge3d://schema/command";
/// URI prefix / template of object resources.
pub const OBJECT_PREFIX: &str = "forge3d://object/";
/// RFC 6570 template of object resources.
pub const OBJECT_TEMPLATE: &str = "forge3d://object/{id}";

/// A listed resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// Static resources plus one entry per object.
pub async fn list_resources(handle: &DocumentHandle) -> ApiResult<Vec<ResourceInfo>> {
    let objects = handle
        .run(|doc| {
            doc.scene
                .traverse()
                .into_iter()
                .filter_map(|id| doc.scene.get(id).map(|n| (id, n.name.clone(), n.mesh.as_ref().map(|m| m.face_count()))))
                .collect::<Vec<_>>()
        })
        .await?;
    let mut out = vec![
        ResourceInfo {
            uri: SCENE_URI.into(),
            name: "scene".into(),
            description: "Scene tree: objects, transforms, materials, counts, bounds, selection.".into(),
            mime_type: "application/json".into(),
        },
        ResourceInfo {
            uri: HISTORY_URI.into(),
            name: "history".into(),
            description: "Undo history (AI operations are labelled \"AI: ...\").".into(),
            mime_type: "application/json".into(),
        },
        ResourceInfo {
            uri: COMMAND_SCHEMA_URI.into(),
            name: "command-schema".into(),
            description: "JSON Schema of raw CommandSpec objects usable as batch `command` steps.".into(),
            mime_type: "application/schema+json".into(),
        },
    ];
    for (id, name, faces) in objects {
        out.push(ResourceInfo {
            uri: format!("{OBJECT_PREFIX}{id}"),
            name: format!("object: {name}"),
            description: match faces {
                Some(f) => format!("Object '{name}' ({f} faces)"),
                None => format!("Empty/group object '{name}'"),
            },
            mime_type: "application/json".into(),
        });
    }
    Ok(out)
}

/// Read a resource as JSON.
pub async fn read_resource(handle: &DocumentHandle, uri: &str) -> ApiResult<Value> {
    match uri {
        SCENE_URI => handle
            .run(|doc| scene_info(doc).map(|s| serde_json::to_value(s).expect("serializes")))
            .await?,
        HISTORY_URI => {
            handle
                .run(|doc| {
                    json!({
                        "entries": doc.history_list(),
                        "can_undo": doc.can_undo(),
                        "can_redo": doc.can_redo(),
                        "revision": doc.revision(),
                    })
                })
                .await
        }
        COMMAND_SCHEMA_URI => Ok(CommandSpec::json_schema()),
        _ => {
            let Some(r) = uri.strip_prefix(OBJECT_PREFIX) else {
                return Err(unknown(uri));
            };
            let r = percent_decode(r);
            if r.is_empty() {
                return Err(unknown(uri));
            }
            handle
                .run(move |doc| {
                    let id = resolve_ref(&doc.scene, &r)?;
                    object_info(doc, id).map(|o| serde_json::to_value(o).expect("serializes"))
                })
                .await?
        }
    }
}

fn unknown(uri: &str) -> ApiError {
    ApiError::new(
        "resource_not_found",
        format!("unknown resource '{uri}'"),
        format!("Available: {SCENE_URI}, {HISTORY_URI}, {COMMAND_SCHEMA_URI} and {OBJECT_TEMPLATE} (id or unique name); list them with resources/list."),
    )
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

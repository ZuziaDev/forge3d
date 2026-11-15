//! Read-only views of the document (scene tree, objects, selection, history)
//! and resolution of object references (id or unique name).

use forge3d_core::command::ChangeSummary;
use forge3d_core::document::{Document, HistoryEntryInfo};
use forge3d_core::math::{euler_deg_from_quat, Aabb};
use forge3d_core::scene::{Material, Scene};
use forge3d_core::selection::SelectMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// Reference to an object: its id (UUID) or its unique name.
pub type ObjectRef = String;

/// Resolve an object reference against the scene.
pub fn resolve_ref(scene: &Scene, r: &str) -> ApiResult<Uuid> {
    let r = r.trim();
    if let Ok(id) = Uuid::parse_str(r) {
        if scene.contains(id) {
            return Ok(id);
        }
        // A node could in principle be *named* like a UUID; fall through.
        if let Some(id) = unique_by_name(scene, r)? {
            return Ok(id);
        }
        return Err(not_found(scene, r));
    }
    match unique_by_name(scene, r)? {
        Some(id) => Ok(id),
        None => Err(not_found(scene, r)),
    }
}

fn unique_by_name(scene: &Scene, name: &str) -> ApiResult<Option<Uuid>> {
    let matches: Vec<Uuid> = scene
        .traverse()
        .into_iter()
        .filter(|id| scene.get(*id).is_some_and(|n| n.name == name))
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        _ => Err(ApiError::new(
            "ambiguous_object",
            format!("{} objects are named '{name}'", matches.len()),
            "Reference the object by its id instead (scene_get lists ids), or rename one of them with object_rename.",
        )
        .with_details(serde_json::json!({ "ids": matches }))),
    }
}

fn not_found(scene: &Scene, r: &str) -> ApiError {
    let names: Vec<String> = scene
        .traverse()
        .into_iter()
        .take(20)
        .filter_map(|id| scene.get(id).map(|n| n.name.clone()))
        .collect();
    let known = if names.is_empty() {
        "The scene is empty; create objects with object_create first.".to_string()
    } else {
        format!(
            "Existing objects include: {}.",
            names
                .iter()
                .map(|n| format!("'{n}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    ApiError::new(
        "object_not_found",
        format!("no object with id or name '{r}'"),
        format!("Objects can be referenced by id or by unique (case-sensitive) name; call scene_get for the current list. {known}"),
    )
}

/// Resolve an optional list of references.
pub fn resolve_refs(scene: &Scene, refs: &Option<Vec<ObjectRef>>) -> ApiResult<Option<Vec<Uuid>>> {
    match refs {
        None => Ok(None),
        Some(list) => list
            .iter()
            .map(|r| resolve_ref(scene, r))
            .collect::<ApiResult<Vec<_>>>()
            .map(Some),
    }
}

/// Resolve an optional reference.
pub fn resolve_opt(scene: &Scene, r: &Option<ObjectRef>) -> ApiResult<Option<Uuid>> {
    r.as_deref().map(|r| resolve_ref(scene, r)).transpose()
}

/// Bounding box in JSON form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoundsInfo {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub size: [f32; 3],
}

impl From<Aabb> for BoundsInfo {
    fn from(b: Aabb) -> Self {
        BoundsInfo {
            min: round3(b.min.to_array()),
            max: round3(b.max.to_array()),
            center: round3(b.center().to_array()),
            size: round3(b.size().to_array()),
        }
    }
}

/// Material summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaterialInfo {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
}

impl From<&Material> for MaterialInfo {
    fn from(m: &Material) -> Self {
        MaterialInfo {
            name: m.name.clone(),
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.roughness,
            emissive: m.emissive,
        }
    }
}

/// One object of the scene tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectInfo {
    pub id: Uuid,
    pub name: String,
    pub parent: Option<Uuid>,
    pub children: Vec<Uuid>,
    /// Local transform relative to the parent.
    pub translation: [f32; 3],
    /// XYZ Euler angles in degrees.
    pub rotation_euler_deg: [f32; 3],
    pub scale: [f32; 3],
    pub visible: bool,
    pub material: Option<MaterialInfo>,
    pub has_mesh: bool,
    pub vertex_count: usize,
    pub edge_count: usize,
    pub face_count: usize,
    /// Axis-aligned bounding box in world space (null without mesh).
    pub world_bounds: Option<BoundsInfo>,
    pub selected: bool,
    pub active: bool,
}

/// Round to 5 decimals to keep JSON short and stable.
pub fn round(v: f32) -> f32 {
    let r = (v as f64 * 1e5).round() / 1e5;
    if r == 0.0 {
        0.0
    } else {
        r as f32
    }
}

/// Round each component.
pub fn round3(v: [f32; 3]) -> [f32; 3] {
    [round(v[0]), round(v[1]), round(v[2])]
}

/// Summary of one object.
pub fn object_info(doc: &Document, id: Uuid) -> ApiResult<ObjectInfo> {
    let scene = &doc.scene;
    let n = scene.node(id)?;
    let (s, r, t) = n.transform.to_scale_rotation_translation();
    let m = n.mesh.as_ref();
    Ok(ObjectInfo {
        id,
        name: n.name.clone(),
        parent: n.parent(),
        children: n.children().to_vec(),
        translation: round3(t.to_array()),
        rotation_euler_deg: round3(euler_deg_from_quat(r)),
        scale: round3(s.to_array()),
        visible: n.visible,
        material: n.material.as_ref().map(MaterialInfo::from),
        has_mesh: m.is_some(),
        vertex_count: m.map_or(0, |m| m.vertex_count()),
        edge_count: m.map_or(0, |m| m.edge_count()),
        face_count: m.map_or(0, |m| m.face_count()),
        world_bounds: scene.world_bounding_box(id)?.map(BoundsInfo::from),
        selected: doc.selection.is_selected(id),
        active: doc.selection.active() == Some(id),
    })
}

/// Element selection of one object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ElementSelectionInfo {
    pub object: Uuid,
    pub object_name: String,
    pub vertices: usize,
    pub edges: usize,
    pub faces: usize,
}

/// The current selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SelectionInfo {
    pub mode: SelectMode,
    pub objects: Vec<Uuid>,
    pub active: Option<Uuid>,
    pub active_name: Option<String>,
    /// Objects with selected mesh elements (counts per kind).
    pub elements: Vec<ElementSelectionInfo>,
}

/// Summarize the selection.
pub fn selection_info(doc: &Document) -> SelectionInfo {
    SelectionInfo {
        mode: doc.selection.mode,
        objects: doc.selection.objects().to_vec(),
        active: doc.selection.active(),
        active_name: doc
            .selection
            .active()
            .and_then(|a| doc.scene.get(a))
            .map(|n| n.name.clone()),
        elements: doc
            .selection
            .objects_with_elements()
            .map(|(id, s)| ElementSelectionInfo {
                object: *id,
                object_name: doc.scene.get(*id).map(|n| n.name.clone()).unwrap_or_default(),
                vertices: s.vertices.len(),
                edges: s.edges.len(),
                faces: s.faces.len(),
            })
            .collect(),
    }
}

/// The whole scene tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SceneInfo {
    /// Objects in depth-first order (parents before children).
    pub objects: Vec<ObjectInfo>,
    pub roots: Vec<Uuid>,
    pub object_count: usize,
    /// World bounds of all visible meshes.
    pub world_bounds: Option<BoundsInfo>,
    pub selection: SelectionInfo,
    /// Increments with every change (execute/undo/redo).
    pub revision: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Coordinate convention reminder for the model.
    pub conventions: String,
}

/// Coordinate conventions.
pub const CONVENTIONS: &str = "Right-handed, Z is up, units are meters; the ground grid is the XY plane at z=0. Primitives are centered at their origin (a cube of size 2 spans -1..1), so to stand an object on the ground set translation.z to half its height. Rotations are XYZ Euler angles in degrees.";

/// Build the scene summary.
pub fn scene_info(doc: &Document) -> ApiResult<SceneInfo> {
    let mut objects = Vec::with_capacity(doc.scene.len());
    let mut bounds: Option<Aabb> = None;
    for id in doc.scene.traverse() {
        let info = object_info(doc, id)?;
        if info.visible {
            if let Some(b) = doc.scene.world_bounding_box(id)? {
                bounds = Some(bounds.map_or(b, |o| o.union(&b)));
            }
        }
        objects.push(info);
    }
    Ok(SceneInfo {
        object_count: objects.len(),
        objects,
        roots: doc.scene.roots().to_vec(),
        world_bounds: bounds.map(BoundsInfo::from),
        selection: selection_info(doc),
        revision: doc.revision(),
        can_undo: doc.can_undo(),
        can_redo: doc.can_redo(),
        conventions: CONVENTIONS.to_string(),
    })
}

/// Result of every mutating tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MutationResult {
    /// Objects created / modified / deleted by the operation.
    pub changes: ChangeSummary,
    /// The new undo-history entry (label is prefixed with "AI: " for AI calls).
    pub history_entry: Option<HistoryEntryInfo>,
    /// Document revision after the operation.
    pub revision: u64,
}

/// The most recently applied history entry.
pub fn last_entry(doc: &Document) -> Option<HistoryEntryInfo> {
    doc.history_list().into_iter().rev().find(|e| !e.undone)
}

/// Build a [`MutationResult`] after a successful execute.
pub fn mutation(doc: &Document, changes: ChangeSummary) -> MutationResult {
    MutationResult {
        changes,
        history_entry: last_entry(doc),
        revision: doc.revision(),
    }
}

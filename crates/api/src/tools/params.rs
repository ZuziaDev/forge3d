//! Typed tool parameters. Doc comments become JSON-Schema descriptions, so
//! they are written for the model that calls the tools.

use forge3d_core::math::{Axis, Space};
use forge3d_core::mesh::{ElementKind, MeshData};
use forge3d_core::modifier::ModifierKind;
use forge3d_core::ops::BooleanOp;
use forge3d_core::primitives::PrimitiveSpec;
use forge3d_core::scene::Material;
use forge3d_core::selection::{SelectAction, SelectMode};
use forge3d_core::spec::MergeMode;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::info::ObjectRef;

fn d_true() -> bool {
    true
}
fn d_one_f() -> f32 {
    1.0
}
fn d_thickness() -> f32 {
    0.1
}
fn d_width() -> f32 {
    0.1
}
fn d_one() -> u32 {
    1
}
fn d_half() -> f32 {
    0.5
}
fn d_threshold() -> f32 {
    1e-4
}
fn d_angle() -> f32 {
    30.0
}
fn d_quad_angle() -> f32 {
    40.0
}
fn d_max_indices() -> usize {
    200
}
fn d_limit() -> usize {
    100
}
fn d_history_limit() -> usize {
    50
}

/// Parameters of `scene_get`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneGetParams {
    /// Include hidden objects (default true).
    #[serde(default = "d_true")]
    pub include_hidden: bool,
}

/// Parameters of `object_create`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectCreateParams {
    /// The shape and its parameters, e.g. `{"type":"cube","size":2}`,
    /// `{"type":"cylinder","radius":0.5,"depth":2,"segments":32}`. Omit it to
    /// create an empty group node (useful as a parent for several parts).
    #[serde(default)]
    pub primitive: Option<PrimitiveSpec>,
    /// Object name (made unique by appending .001, .002 ... if taken).
    /// Defaults to the primitive's name ("Cube", "Cylinder", ...).
    #[serde(default)]
    pub name: Option<String>,
    /// Parent object (id or unique name). The transform is then relative to the parent.
    #[serde(default)]
    pub parent: Option<ObjectRef>,
    /// Local position [x, y, z] in meters (Z is up). Default [0,0,0].
    #[serde(default)]
    pub translation: Option<[f32; 3]>,
    /// Local rotation as XYZ Euler angles in degrees. Default [0,0,0].
    #[serde(default)]
    pub rotation_euler_deg: Option<[f32; 3]>,
    /// Local scale per axis (non-zero). Default [1,1,1].
    #[serde(default)]
    pub scale: Option<[f32; 3]>,
    /// PBR material for the object, e.g. `{"name":"Wood","base_color":[0.55,0.35,0.2,1],"roughness":0.8}`.
    #[serde(default)]
    pub material: Option<Material>,
}

/// Absolute or relative transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransformMode {
    /// Add to the current transform: move by `translation`, rotate by
    /// `rotation_euler_deg` (applied about X, then Y, then Z) or by
    /// `rotation_axis`/`rotation_angle_deg`, multiply the scale by `scale`.
    #[default]
    Relative,
    /// Set the local transform components; omitted components keep their value.
    Absolute,
}

/// Parameters of `object_transform`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectTransformParams {
    /// Objects to transform (ids or unique names). Default: the selected objects.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// `relative` (default) or `absolute`.
    #[serde(default)]
    pub mode: TransformMode,
    /// Absolute: new local position. Relative: offset to move by.
    #[serde(default)]
    pub translation: Option<[f32; 3]>,
    /// Absolute: new local rotation (XYZ Euler, degrees). Relative: rotation
    /// to add, applied about the X, then Y, then Z axis.
    #[serde(default)]
    pub rotation_euler_deg: Option<[f32; 3]>,
    /// Relative mode only: rotate about this axis ...
    #[serde(default)]
    pub rotation_axis: Option<[f32; 3]>,
    /// ... by this angle in degrees (requires `rotation_axis`).
    #[serde(default)]
    pub rotation_angle_deg: Option<f32>,
    /// Absolute: new local scale. Relative: factors to multiply the scale by.
    #[serde(default)]
    pub scale: Option<[f32; 3]>,
    /// Relative mode: whether offsets/axes are in `world` (default) or the object's `local` space.
    #[serde(default)]
    pub space: Space,
    /// Relative mode: world-space pivot for rotation and scaling (default: each object's origin).
    #[serde(default)]
    pub pivot: Option<[f32; 3]>,
}

/// Parameters of `object_delete`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectDeleteParams {
    /// Objects to delete (ids or unique names). Required; must not be empty.
    pub objects: Vec<ObjectRef>,
    /// Keep the children (they are re-attached to the deleted object's parent)
    /// instead of deleting them too. Default false.
    #[serde(default)]
    pub keep_children: bool,
}

/// Parameters of `object_duplicate`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectDuplicateParams {
    /// Objects to copy (ids or unique names), each with its children. Default: the selected objects.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// World-space offset applied to every copy, e.g. [2,0,0]. Default [0,0,0].
    #[serde(default)]
    pub offset: [f32; 3],
}

/// Parameters of `object_rename`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectRenameParams {
    /// Object to rename (id or unique name).
    pub object: ObjectRef,
    /// New name (made unique if already used).
    pub name: String,
}

/// Parameters of `object_parent`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectParentParams {
    /// Objects to re-parent (ids or unique names).
    pub objects: Vec<ObjectRef>,
    /// New parent (id or unique name); null or omitted makes them top-level objects.
    #[serde(default)]
    pub parent: Option<ObjectRef>,
    /// Keep the objects where they are in the world (default true). When
    /// false the local transform is kept and the object moves with its new parent.
    #[serde(default = "d_true")]
    pub keep_world_transform: bool,
}

/// A mesh editing operation. Element index lists are optional: when omitted
/// the object's current element selection (see mesh_select) is used.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeshOperation {
    /// Extrude faces (as one connected region) along their normals by
    /// `distance`, or along `offset` (object-local vector). The new cap faces become selected.
    Extrude {
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_one_f")]
        distance: f32,
        #[serde(default)]
        offset: Option<[f32; 3]>,
    },
    /// Inset faces by `thickness` (border width), optionally pushing the inner faces by `depth`.
    Inset {
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_thickness")]
        thickness: f32,
        #[serde(default)]
        depth: f32,
        /// Inset each face separately instead of the selection as one region.
        #[serde(default)]
        individual: bool,
    },
    /// Bevel interior edges with the given width. `segments` 1 = flat chamfer;
    /// 2..=16 = rounded profile (use 3-4 for nicely rounded hard-surface edges).
    /// The new bevel faces become selected.
    Bevel {
        #[serde(default)]
        edges: Option<Vec<u32>>,
        #[serde(default = "d_width")]
        width: f32,
        #[serde(default = "d_one")]
        segments: u32,
    },
    /// Give the whole surface a wall `thickness` (inner shell grown against
    /// the normals, rim quads on open edges). Turns an open surface into a solid.
    Solidify {
        #[serde(default = "d_thickness")]
        thickness: f32,
    },
    /// Connect two holes (boundary loops with equal vertex counts) with a ring
    /// of quads. `edges`: any edges on the two loops (default: selection).
    BridgeEdgeLoops {
        #[serde(default)]
        edges: Option<Vec<u32>>,
    },
    /// Close holes with one n-gon each (or a triangle fan). Default: holes
    /// touching the selected edges, or every hole when nothing is selected.
    FillHoles {
        #[serde(default)]
        edges: Option<Vec<u32>>,
        #[serde(default)]
        triangle_fan: bool,
    },
    /// Remove vertices/edges/faces while merging the surrounding faces into
    /// one n-gon (keeps the surface closed, unlike delete).
    Dissolve {
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
    },
    /// Split faces into triangles (default: selected faces, or all).
    Triangulate {
        #[serde(default)]
        faces: Option<Vec<u32>>,
    },
    /// Join adjacent triangle pairs into convex quads when their normals
    /// differ by at most `max_angle_deg` (default: selected faces, or all).
    TrisToQuads {
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_quad_angle")]
        max_angle_deg: f32,
    },
    /// Replace faces with triangle fans around their centers, pushed by
    /// `offset` along the face normal (spikes / pyramids).
    Poke {
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default)]
        offset: f32,
    },
    /// Laplacian smoothing: move vertices `factor` (0..1) towards the average
    /// of their neighbours, `iterations` times (default: selection, or all).
    Smooth {
        #[serde(default)]
        vertices: Option<Vec<u32>>,
        #[serde(default = "d_half")]
        factor: f32,
        #[serde(default = "d_one")]
        iterations: u32,
    },
    /// Move faces (default: selected) into a new object with the same
    /// transform, material, shading and modifiers.
    Separate {
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Insert `cuts` edge loops across the ring of quads through `edge`.
    LoopCut {
        edge: u32,
        #[serde(default = "d_one")]
        cuts: u32,
        /// Position of a single cut along the edge (0..1, exclusive).
        #[serde(default = "d_half")]
        factor: f32,
    },
    /// Catmull–Clark subdivision of the whole mesh (`levels` 1..=4; each level ~4x the faces).
    Subdivide {
        #[serde(default = "d_one")]
        levels: u32,
    },
    /// Merge vertices: `center` collapses them into one; `distance` welds
    /// vertices closer than `threshold` (whole mesh when no vertices given/selected).
    Merge {
        #[serde(default)]
        vertices: Option<Vec<u32>>,
        #[serde(default)]
        mode: MergeMode,
        #[serde(default = "d_threshold")]
        threshold: f32,
    },
    /// Delete vertices, edges or faces (`kind` defaults to the select mode).
    Delete {
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
    },
    /// Mirror the geometry across the object-local plane perpendicular to `axis`
    /// (keeps the original and adds the mirrored half; `merge` welds the seam).
    Mirror {
        axis: Axis,
        #[serde(default = "d_true")]
        merge: bool,
        #[serde(default = "d_threshold")]
        merge_threshold: f32,
    },
    /// Make the winding consistent with normals pointing outward (or inward).
    RecalculateNormals {
        #[serde(default)]
        inside: bool,
    },
    /// Reverse face winding (given/selected faces, or all faces when none).
    FlipNormals {
        #[serde(default)]
        faces: Option<Vec<u32>>,
    },
    /// Move/rotate/scale elements in object-local space about `pivot`
    /// (default: the median of the affected vertices).
    Transform {
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        #[serde(default)]
        rotation_euler_deg: Option<[f32; 3]>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
        #[serde(default)]
        pivot: Option<[f32; 3]>,
    },
    /// Replace the whole mesh with raw data (positions + polygons, CCW seen from outside).
    SetMeshData { mesh: MeshData },
}

/// Parameters of `mesh_edit`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeshEditParams {
    /// Object to edit (id or unique name). Default: the active object.
    #[serde(default)]
    pub object: Option<ObjectRef>,
    /// The operation, tagged by `type`, e.g. `{"type":"extrude","faces":[5],"distance":0.5}`.
    pub operation: MeshOperation,
}

/// How `mesh_select` picks elements.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectBy {
    /// Explicit element indices (see mesh_get for indices).
    Indices { indices: Vec<u32> },
    /// All elements of `kind`.
    All,
    /// Clear the element selection of the object.
    None,
    /// Invert the element selection of `kind`.
    Invert,
    /// Elements whose normal is within `max_angle_deg` of `direction`
    /// (e.g. [0,0,1] = facing up). Great for "the top face".
    Direction {
        direction: [f32; 3],
        #[serde(default = "d_angle")]
        max_angle_deg: f32,
        #[serde(default)]
        space: Space,
    },
    /// Elements whose normal is similar to that of element `reference`.
    Normal {
        reference: u32,
        #[serde(default = "d_angle")]
        max_angle_deg: f32,
    },
    /// Elements entirely inside the axis-aligned box `min`..`max` (world space by default).
    Box {
        min: [f32; 3],
        max: [f32; 3],
        #[serde(default)]
        space: Space,
    },
    /// The edge loop (or ring with `ring: true`) through `edge`; selects edges.
    EdgeLoop {
        edge: u32,
        #[serde(default)]
        ring: bool,
    },
    /// Object-level selection: select these objects (ids or unique names);
    /// the last listed becomes active. Switches to object mode.
    Objects { objects: Vec<ObjectRef> },
}

/// Parameters of `mesh_select`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeshSelectParams {
    /// Object whose elements are selected (id or unique name). Default: the active object.
    #[serde(default)]
    pub object: Option<ObjectRef>,
    /// Element kind: `vertex`, `edge` or `face`. Default: the current select
    /// mode, or `face` in object mode. The select mode is switched to this kind.
    #[serde(default)]
    pub kind: Option<ElementKind>,
    /// What to select. Omit to only change the select `mode`.
    #[serde(default)]
    pub by: Option<SelectBy>,
    /// How to combine with the existing selection: `set` (default), `add`, `remove`, `intersect`.
    #[serde(default)]
    pub action: SelectAction,
    /// Explicitly set the select mode (`object`, `vertex`, `edge`, `face`).
    #[serde(default)]
    pub mode: Option<SelectMode>,
    /// Maximum number of selected indices to return (default 200, max 5000).
    #[serde(default = "d_max_indices")]
    pub max_indices: usize,
}

/// Sections returned by `mesh_get`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeshSection {
    Vertices,
    Edges,
    Faces,
}

fn d_sections() -> Vec<MeshSection> {
    vec![MeshSection::Vertices, MeshSection::Faces]
}

/// Parameters of `mesh_get`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeshGetParams {
    /// Object (id or unique name). Default: the active object.
    #[serde(default)]
    pub object: Option<ObjectRef>,
    /// Which element lists to return (default ["vertices","faces"]).
    #[serde(default = "d_sections")]
    pub include: Vec<MeshSection>,
    /// Index of the first element of each list (default 0).
    #[serde(default)]
    pub offset: usize,
    /// Maximum elements per list (default 100, hard cap 1000). Use `next_offset` to page.
    #[serde(default = "d_limit")]
    pub limit: usize,
    /// Include vertex/face normals.
    #[serde(default)]
    pub normals: bool,
    /// Coordinates in `local` (default, as used by mesh_edit) or `world` space.
    #[serde(default = "local_space")]
    pub space: Space,
    /// Include the mesh validation report (manifold, closed, issues). Default true.
    #[serde(default = "d_true")]
    pub validate: bool,
}

fn local_space() -> Space {
    Space::Local
}

/// Parameters of `boolean_op`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BooleanOpParams {
    /// Object A (id or unique name); its mesh is replaced by the result.
    pub object: ObjectRef,
    /// Object B (id or unique name), the cutter / second operand.
    pub other: ObjectRef,
    /// `union` (A ∪ B), `difference` (A − B) or `intersect` (A ∩ B).
    pub operation: BooleanOp,
    /// Keep object B instead of deleting it (default false).
    #[serde(default)]
    pub keep_other: bool,
}

/// Parameters of `material_set`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterialSetParams {
    /// Objects (ids or unique names). Default: the selected objects.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// Material to assign. Colors are sRGB-ish RGBA in 0..1; metallic/roughness 0..1;
    /// emissive RGB >= 0. Omitted fields take defaults (base_color [0.8,0.8,0.8,1], roughness 0.5).
    #[serde(default)]
    pub material: Option<Material>,
    /// Remove the material instead (objects render with the default grey).
    #[serde(default)]
    pub clear: bool,
}

/// Parameters of `undo` / `redo`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepsParams {
    /// Number of steps (default 1, max 100).
    #[serde(default = "d_one")]
    pub steps: u32,
}

/// Parameters of `history_list`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryListParams {
    /// Return at most this many of the most recent entries (default 50, max 500).
    #[serde(default = "d_history_limit")]
    pub limit: usize,
}

/// Up axis convention of OBJ files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpAxisParam {
    Y,
    Z,
}

/// Parameters of `file_import`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileImportParams {
    /// File inside the workspace (.gltf, .glb, .obj, .stl, .f3d), relative paths recommended.
    pub path: String,
    /// Parent object for the imported objects (id or unique name). Default: top level.
    #[serde(default)]
    pub parent: Option<ObjectRef>,
    /// Weld duplicate vertices (default true).
    #[serde(default = "d_true")]
    pub weld: bool,
    /// Up axis of OBJ files (default `y`, converted to forge3d's Z-up).
    #[serde(default)]
    pub obj_up_axis: Option<UpAxisParam>,
}

/// Parameters of `file_export`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileExportParams {
    /// Target file inside the workspace; the format follows the extension
    /// (.glb, .gltf, .obj, .stl, .f3d). Missing directories are created.
    pub path: String,
    /// Export only these objects (and their children). Default: the whole scene.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// Include hidden objects (default true).
    #[serde(default = "d_true")]
    pub include_hidden: bool,
    /// Write ASCII instead of binary STL.
    #[serde(default)]
    pub stl_ascii: bool,
}

/// Parameters of `project_save` / `project_load`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPathParams {
    /// .f3d project file inside the workspace.
    pub path: String,
}

/// Parameters of `project_load`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectLoadParams {
    /// .f3d project file inside the workspace.
    pub path: String,
    /// Also clear the undo history. Default false: loading is itself one
    /// undoable step, so `undo` brings the previous scene back.
    #[serde(default)]
    pub clear_history: bool,
}

/// One step of a `batch`. Give EITHER `tool` + `arguments` (any mutating tool:
/// object_create, object_transform, object_delete, object_duplicate,
/// object_rename, object_parent, mesh_edit, mesh_select, boolean_op,
/// material_set, file_import) OR `command` (a raw CommandSpec object with an
/// `op` field, see the forge3d://schema/command resource).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchStepSchema {
    /// Tool name, e.g. "object_create".
    #[serde(default)]
    pub tool: Option<String>,
    /// Arguments of the tool, exactly as for a direct call.
    #[serde(default)]
    pub arguments: Option<serde_json::Map<String, serde_json::Value>>,
    /// Raw command, e.g. {"op":"subdivide","levels":1}.
    #[serde(default)]
    pub command: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Parameters of `batch`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchParams {
    /// Label of the single undo-history entry (default: the joined step descriptions).
    #[serde(default)]
    pub description: Option<String>,
    /// Steps executed in order. Later steps can reference objects created by
    /// earlier steps by name. If any step fails, all are rolled back.
    pub steps: Vec<BatchStepSchema>,
}

/// One modifier-stack action.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModifierAction {
    /// Append a modifier (or insert it at `index`).
    Add {
        modifier: ModifierKind,
        #[serde(default = "d_true")]
        enabled: bool,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Replace the parameters (`modifier`, full object incl. `type`) and/or `enabled`.
    Set {
        index: usize,
        #[serde(default)]
        modifier: Option<ModifierKind>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    /// Remove the modifier at `index`.
    Remove { index: usize },
    /// Move the modifier at `from` to position `to` (order matters: e.g.
    /// mirror before subdivision for a seamless symmetric surface).
    Move { from: usize, to: usize },
    /// Bake the modifier at `index` into the base mesh, or the whole stack
    /// when `index` is omitted. Needed before element edits should see the result.
    Apply {
        #[serde(default)]
        index: Option<usize>,
    },
}

/// Parameters of `modifier_edit`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ModifierEditParams {
    /// Object (id or unique name). Default: the active object.
    #[serde(default)]
    pub object: Option<ObjectRef>,
    /// The stack change, tagged by `action`, e.g.
    /// `{"action":"add","modifier":{"type":"mirror","axis":"x"}}`.
    #[serde(flatten)]
    pub action: ModifierAction,
}

/// Parameters of `object_shading`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectShadingParams {
    /// Objects (ids or unique names). Default: the selection.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// true = smooth (interpolated normals), false = flat faces.
    pub smooth: bool,
    /// With smooth: edges whose faces meet at more than this angle (degrees,
    /// 0..=180) stay sharp. 30-45 suits hard-surface models; omit for fully smooth.
    #[serde(default)]
    pub auto_smooth_angle_deg: Option<f32>,
}

/// Parameters of `object_join`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectJoinParams {
    /// Objects to merge (ids or unique names, at least two incl. `into`). Default: the selection.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// Object that receives the geometry (default: the active object).
    #[serde(default)]
    pub into: Option<ObjectRef>,
}

/// Parameters of `mesh_measure`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeshMeasureParams {
    /// Objects to measure (ids or unique names). Default: the selection, or every mesh object.
    #[serde(default)]
    pub objects: Option<Vec<ObjectRef>>,
    /// Measure the base mesh instead of the evaluated (modifiers applied) mesh.
    #[serde(default)]
    pub base_mesh: bool,
}

//! Transport-independent tool layer.
//!
//! [`Forge3dTools`] lists the tools (name, description, JSON Schema of the
//! input) and dispatches calls with JSON arguments. The MCP server and the
//! CLI script runner are thin adapters around it.

pub mod params;
mod plan;
mod render;

use std::sync::Arc;

use forge3d_core::command::{ChangeSummary, Command, ObjectSummary, Origin};
use forge3d_core::document::{Document, DocumentState};
use forge3d_core::error::CoreResult;
use forge3d_core::math::Space;
use forge3d_core::mesh::{EdgeId, FaceId, VertexId};
use forge3d_core::selection::normal_matrix;
use forge3d_core::spec::CommandSpec;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::host::DocumentHandle;
use crate::info::{self, mutation, resolve_refs, round3};
use crate::workspace::{PathAccess, Workspace};
use params::*;
use plan::{Plan, Sequence};

pub use render::{CameraParams, RenderedImage, ViewportRenderParams};

/// Context shared by all tool calls of one [`Forge3dTools`] instance.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Directory file tools are confined to.
    pub workspace: Workspace,
    /// Origin recorded in the undo history (`Ai` → "AI: ..." labels).
    pub origin: Origin,
}

/// Description of one tool.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// JSON Schema (draft 2020-12) of the arguments object.
    pub input_schema: Value,
    /// The tool never changes the document or files.
    pub read_only: bool,
    /// The tool may delete or overwrite data (still undoable for document edits).
    pub destructive: bool,
    /// The tool can be used as a `batch` step.
    pub batchable: bool,
}

/// Result of a tool call: JSON plus optional images (viewport_render).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub images: Vec<RenderedImage>,
}

impl ToolOutput {
    fn json(value: Value) -> ToolOutput {
        ToolOutput {
            value,
            images: Vec::new(),
        }
    }
}

fn schema_of<T: JsonSchema>() -> Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    serde_json::to_value(schema).expect("schema serializes")
}

struct Spec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    read_only: bool,
    destructive: bool,
    batchable: bool,
}

const TOOLS: &[Spec] = &[
    Spec {
        name: "scene_get",
        title: "Get scene",
        description: "Return the scene tree: every object with id, name, parent/children, local transform (translation, rotation_euler_deg, scale), visibility, material summary, vertex/edge/face counts and world bounding box, plus the current selection, active object, undo state and the coordinate conventions (Z up, meters). Call this first and whenever you need ids or want to verify the result of an edit.",
        schema: schema_of::<SceneGetParams>,
        read_only: true,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "object_create",
        title: "Create object",
        description: "Add a primitive object (cube, plane, uv_sphere, ico_sphere, cylinder, cone, torus) with its parameters, optional name, parent, local transform and PBR material; omit `primitive` to create an empty group node. Primitives are centered at their origin and Z is up (a cube of size 1 at translation [0,0,0.5] rests on the ground). The new object becomes selected and active. Returns the created object and the history entry.",
        schema: schema_of::<ObjectCreateParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "object_transform",
        title: "Transform objects",
        description: "Move, rotate and/or scale objects. mode `relative` (default) adds to the current transform (translate by an offset, rotate by Euler angles or axis+angle, multiply scale), optionally in local space or about a world pivot; mode `absolute` sets local translation/rotation/scale directly (omitted components are kept). Objects default to the selection; reference them by id or unique name.",
        schema: schema_of::<ObjectTransformParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "object_delete",
        title: "Delete objects",
        description: "Delete objects (by id or unique name) together with their children, or keep the children with `keep_children`. Undoable with `undo`.",
        schema: schema_of::<ObjectDeleteParams>,
        read_only: false,
        destructive: true,
        batchable: true,
    },
    Spec {
        name: "object_duplicate",
        title: "Duplicate objects",
        description: "Copy objects (with children, meshes and materials), optionally offset in world space; the copies become selected. Returns the new objects (their names get a .001 style suffix).",
        schema: schema_of::<ObjectDuplicateParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "object_rename",
        title: "Rename object",
        description: "Rename an object. Names are made unique (a .001 suffix is added if the name is taken); the result shows the final name.",
        schema: schema_of::<ObjectRenameParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "object_parent",
        title: "Set parent",
        description: "Attach objects to a parent (building a hierarchy) or detach them to the top level with `parent: null`. By default the world placement is kept.",
        schema: schema_of::<ObjectParentParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "mesh_edit",
        title: "Edit mesh",
        description: "Apply a polygon-modeling operation to an object's mesh: extrude, inset, bevel, loop_cut, subdivide (Catmull-Clark), merge, delete, mirror, recalculate_normals, flip_normals, transform (move/rotate/scale elements) or set_mesh_data. Element indices (faces/edges/vertices) are optional: when omitted the object's current element selection from mesh_select is used. Indices change after topology edits; the result returns the new counts and the resulting element selection (e.g. the extruded cap faces), so you can chain edits.",
        schema: schema_of::<MeshEditParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "mesh_select",
        title: "Select elements",
        description: "Select mesh elements of an object (vertices, edges or faces) by index, by normal direction (e.g. faces facing [0,0,1] = top), by similar normal, inside a box, all/none/invert, or an edge loop/ring; combine with action set/add/remove/intersect. Switches the select mode to the element kind and makes the object active. `by: {type: objects}` selects whole objects instead. Returns the selected indices (capped) and counts. Selection changes are undoable steps.",
        schema: schema_of::<MeshSelectParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "mesh_get",
        title: "Get mesh data",
        description: "Read an object's mesh: vertex positions, face vertex-index lists and edges, paged with offset/limit (max 1000 per page; each list reports total and next_offset), optional normals, local or world coordinates, and a validation report (manifold, closed, consistent winding, issues). Use it to find element indices for mesh_select/mesh_edit.",
        schema: schema_of::<MeshGetParams>,
        read_only: true,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "boolean_op",
        title: "Boolean",
        description: "Boolean of two closed meshes: union, difference (object minus other) or intersect. The result replaces `object`'s mesh; `other` is deleted unless keep_other. Both meshes must be closed and overlap.",
        schema: schema_of::<BooleanOpParams>,
        read_only: false,
        destructive: true,
        batchable: true,
    },
    Spec {
        name: "material_set",
        title: "Set material",
        description: "Assign a PBR material (name, base_color RGBA 0..1, metallic, roughness, emissive) to objects (default: selection), or remove it with clear: true.",
        schema: schema_of::<MaterialSetParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "undo",
        title: "Undo",
        description: "Undo the last `steps` operations (default 1), in reverse order. Every tool call that changed the document is one step, a batch is one step.",
        schema: schema_of::<StepsParams>,
        read_only: false,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "redo",
        title: "Redo",
        description: "Redo the last `steps` undone operations (default 1). A new edit clears the redo stack.",
        schema: schema_of::<StepsParams>,
        read_only: false,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "history_list",
        title: "History",
        description: "List the undo history (most recent last): label (AI-made entries start with \"AI: \"), origin, whether undone, and what each step changed.",
        schema: schema_of::<HistoryListParams>,
        read_only: true,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "viewport_render",
        title: "Render viewport",
        description: "Render the scene to a PNG image and return it so you can SEE your work. Camera: `auto` frames the whole scene or `focus` objects from a view preset (front, back, left, right, top, bottom, iso) or yaw/pitch; `orbit` gives target/distance/yaw/pitch; `look_at` gives eye/target/up/fov. Toggles for wireframe, grid and selection highlight. Use it after building or editing to check proportions and placement.",
        schema: schema_of::<ViewportRenderParams>,
        read_only: true,
        destructive: false,
        batchable: false,
    },
    Spec {
        name: "file_import",
        title: "Import file",
        description: "Import a .glb/.gltf/.obj/.stl/.f3d file from the workspace into the scene as one undoable step (imported objects become selected). Paths are relative to the workspace directory.",
        schema: schema_of::<FileImportParams>,
        read_only: false,
        destructive: false,
        batchable: true,
    },
    Spec {
        name: "file_export",
        title: "Export file",
        description: "Export the scene (or some objects) to a file in the workspace; the format follows the extension: .glb (recommended), .gltf, .obj, .stl or .f3d. Existing files are overwritten. Returns the written paths.",
        schema: schema_of::<FileExportParams>,
        read_only: false,
        destructive: true,
        batchable: false,
    },
    Spec {
        name: "project_save",
        title: "Save project",
        description: "Save the document (scene, materials, hierarchy, selection) as a forge3d project (.f3d) in the workspace. The undo history is not saved.",
        schema: schema_of::<ProjectPathParams>,
        read_only: false,
        destructive: true,
        batchable: false,
    },
    Spec {
        name: "project_load",
        title: "Load project",
        description: "Replace the current scene with a .f3d project from the workspace. Loading is one undoable step (undo restores the previous scene) unless clear_history is true, which also empties the undo history.",
        schema: schema_of::<ProjectLoadParams>,
        read_only: false,
        destructive: true,
        batchable: false,
    },
    Spec {
        name: "batch",
        title: "Batch",
        description: "Run several steps atomically as ONE undo entry. Each step is {\"tool\": <mutating tool name>, \"arguments\": {...}} (object_create, object_transform, object_delete, object_duplicate, object_rename, object_parent, mesh_edit, mesh_select, boolean_op, material_set, file_import) or {\"command\": <raw CommandSpec with \"op\">}. Steps run in order and may reference objects created by earlier steps by name. If any step fails everything is rolled back and the error's details.index names the failing step. Returns per-step results.",
        schema: schema_of::<BatchParams>,
        read_only: false,
        destructive: false,
        batchable: false,
    },
];

/// Names of all tools.
pub fn tool_names() -> Vec<&'static str> {
    TOOLS.iter().map(|t| t.name).collect()
}

/// All tool definitions.
pub fn tool_defs() -> Vec<ToolDef> {
    TOOLS
        .iter()
        .map(|t| ToolDef {
            name: t.name,
            title: t.title,
            description: t.description,
            input_schema: (t.schema)(),
            read_only: t.read_only,
            destructive: t.destructive,
            batchable: t.batchable,
        })
        .collect()
}

fn batchable_names() -> Vec<&'static str> {
    TOOLS.iter().filter(|t| t.batchable).map(|t| t.name).collect()
}

/// Parse arguments into a typed parameter struct with a path-qualified error.
pub(crate) fn parse_args<T: DeserializeOwned>(args: Value) -> ApiResult<T> {
    let args = match args {
        Value::Null => Value::Object(Default::default()),
        v @ Value::Object(_) => v,
        other => {
            return Err(ApiError::invalid_arguments(format!(
                "arguments must be a JSON object, got {}",
                match other {
                    Value::Array(_) => "an array",
                    Value::String(_) => "a string",
                    Value::Number(_) => "a number",
                    Value::Bool(_) => "a boolean",
                    _ => "something else",
                }
            )))
        }
    };
    serde_path_to_error::deserialize(args).map_err(|e| {
        let path = e.path().to_string();
        let inner = e.into_inner().to_string();
        if path == "." || path.is_empty() {
            ApiError::invalid_arguments(inner)
        } else {
            ApiError::invalid_arguments(format!("at `{path}`: {inner}"))
        }
    })
}

/// Plan a batchable (document-mutating) tool.
fn plan_tool(doc: &Document, name: &str, args: Value, ctx: &Ctx) -> ApiResult<Plan> {
    match name {
        "object_create" => plan::object_create(doc, parse_args(args)?),
        "object_transform" => plan::object_transform(doc, parse_args(args)?),
        "object_delete" => plan::object_delete(doc, parse_args(args)?),
        "object_duplicate" => plan::object_duplicate(doc, parse_args(args)?),
        "object_rename" => plan::object_rename(doc, parse_args(args)?),
        "object_parent" => plan::object_parent(doc, parse_args(args)?),
        "mesh_edit" => plan::mesh_edit(doc, parse_args(args)?),
        "mesh_select" => plan::mesh_select(doc, parse_args(args)?),
        "boolean_op" => plan::boolean_op(doc, parse_args(args)?),
        "material_set" => plan::material_set(doc, parse_args(args)?),
        "file_import" => plan::file_import(doc, parse_args(args)?, ctx),
        other => Err(ApiError::invalid_arguments(format!(
            "tool '{other}' cannot be used here"
        ))
        .with_hint(format!(
            "Only document-editing tools can be batch steps: {}.",
            batchable_names().join(", ")
        ))),
    }
}

fn merge_into(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(b), Value::Object(e)) = (&mut base, extra) {
        for (k, v) in e {
            b.insert(k, v);
        }
    }
    base
}

/// Execute a planned mutation as one history entry and build the result.
fn execute_plan(doc: &mut Document, plan: Plan, origin: Origin) -> ApiResult<Value> {
    let changes = doc.execute(plan.command, origin)?;
    let extra = match plan.post {
        Some(f) => f(doc, &changes)?,
        None => json!({}),
    };
    let base = serde_json::to_value(mutation(doc, changes)).expect("serializes");
    Ok(merge_into(base, extra))
}

/// The forge3d tool set bound to a document.
#[derive(Clone)]
pub struct Forge3dTools {
    handle: DocumentHandle,
    ctx: Arc<Ctx>,
}

impl std::fmt::Debug for Forge3dTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forge3dTools").field("ctx", &self.ctx).finish()
    }
}

impl Forge3dTools {
    /// Tools operating on `handle`'s document, confined to `workspace`,
    /// recording history entries with `origin`.
    pub fn new(handle: DocumentHandle, workspace: Workspace, origin: Origin) -> Forge3dTools {
        Forge3dTools {
            handle,
            ctx: Arc::new(Ctx { workspace, origin }),
        }
    }

    /// The document handle.
    pub fn handle(&self) -> &DocumentHandle {
        &self.handle
    }

    /// The workspace.
    pub fn workspace(&self) -> &Workspace {
        &self.ctx.workspace
    }

    /// The origin recorded for history entries.
    pub fn origin(&self) -> Origin {
        self.ctx.origin
    }

    /// Definitions of all tools.
    pub fn list(&self) -> Vec<ToolDef> {
        tool_defs()
    }

    /// Call a tool; images (viewport_render) are embedded in the JSON as
    /// `images: [{mime_type, width, height, data_base64}]`.
    pub async fn call(&self, name: &str, args: Value) -> ApiResult<Value> {
        let out = self.call_rich(name, args).await?;
        if out.images.is_empty() {
            return Ok(out.value);
        }
        let images: Vec<Value> = out.images.iter().map(RenderedImage::to_json).collect();
        Ok(merge_into(out.value, json!({ "images": images })))
    }

    /// Call a tool, returning images separately.
    pub async fn call_rich(&self, name: &str, args: Value) -> ApiResult<ToolOutput> {
        let ctx = self.ctx.clone();
        match name {
            "scene_get" => {
                let p: SceneGetParams = parse_args(args)?;
                let v = self
                    .handle
                    .run(move |doc| {
                        let mut s = info::scene_info(doc)?;
                        if !p.include_hidden {
                            s.objects.retain(|o| o.visible);
                            s.object_count = s.objects.len();
                        }
                        Ok::<_, ApiError>(serde_json::to_value(s).expect("serializes"))
                    })
                    .await??;
                Ok(ToolOutput::json(v))
            }
            "mesh_get" => {
                let p: MeshGetParams = parse_args(args)?;
                let v = self.handle.run(move |doc| mesh_get(doc, p)).await??;
                Ok(ToolOutput::json(v))
            }
            "history_list" => {
                let p: HistoryListParams = parse_args(args)?;
                let v = self.handle.run(move |doc| history_list(doc, p)).await?;
                Ok(ToolOutput::json(v))
            }
            "undo" | "redo" => {
                let p: StepsParams = parse_args(args)?;
                let redo = name == "redo";
                let v = self.handle.run(move |doc| undo_redo(doc, p, redo)).await??;
                Ok(ToolOutput::json(v))
            }
            "viewport_render" => render::viewport_render(self, parse_args(args)?).await,
            "file_export" => self.file_export(parse_args(args)?).await.map(ToolOutput::json),
            "project_save" => self.project_save(parse_args(args)?).await.map(ToolOutput::json),
            "project_load" => self.project_load(parse_args(args)?).await.map(ToolOutput::json),
            "batch" => {
                let p: BatchParams = parse_args(args)?;
                let v = self
                    .handle
                    .run(move |doc| run_batch(doc, p, &ctx))
                    .await??;
                Ok(ToolOutput::json(v))
            }
            n if TOOLS.iter().any(|t| t.name == n && t.batchable) => {
                let name = n.to_string();
                let v = self
                    .handle
                    .run(move |doc| {
                        let plan = plan_tool(doc, &name, args, &ctx)?;
                        execute_plan(doc, plan, ctx.origin)
                    })
                    .await??;
                Ok(ToolOutput::json(v))
            }
            other => Err(ApiError::unknown_tool(other, &tool_names())),
        }
    }

    /// Execute a raw [`CommandSpec`] (used by scripts) with this origin.
    pub async fn execute_spec(&self, spec: CommandSpec) -> ApiResult<Value> {
        let origin = self.ctx.origin;
        self.handle
            .run(move |doc| {
                let changes = doc.execute_spec(spec, origin)?;
                Ok::<_, ApiError>(serde_json::to_value(mutation(doc, changes)).expect("serializes"))
            })
            .await?
    }

    async fn file_export(&self, p: FileExportParams) -> ApiResult<Value> {
        let path = self.ctx.workspace.resolve(&p.path, PathAccess::Write)?;
        forge3d_io::ExportFormat::from_path(&path)?;
        let objects = p.objects.clone();
        let scene = self
            .handle
            .run(move |doc| {
                let nodes = resolve_refs(&doc.scene, &objects)?;
                Ok::<_, ApiError>((doc.scene.clone(), nodes))
            })
            .await??;
        let (scene, nodes) = scene;
        let opts = forge3d_io::ExportOptions {
            nodes,
            include_hidden: p.include_hidden,
            stl_ascii: p.stl_ascii,
            ..Default::default()
        };
        let written = blocking(move || {
            forge3d_io::export_scene(&scene, &path, &opts).map_err(ApiError::from)
        })
        .await?;
        let ws = &self.ctx.workspace;
        let files: Vec<Value> = written
            .iter()
            .map(|f| {
                json!({
                    "path": ws.display_path(f),
                    "bytes": std::fs::metadata(f).map(|m| m.len()).unwrap_or(0),
                })
            })
            .collect();
        Ok(json!({ "files": files }))
    }

    async fn project_save(&self, p: ProjectPathParams) -> ApiResult<Value> {
        let path = self.ctx.workspace.resolve(&p.path, PathAccess::Write)?;
        check_f3d(&path, &p.path)?;
        let state = self.handle.run(|doc| doc.state()).await?;
        let objects = state.scene.len();
        let p2 = path.clone();
        blocking(move || {
            forge3d_io::save_project(&Document::from_state(state), &p2).map_err(ApiError::from)
        })
        .await?;
        Ok(json!({
            "path": self.ctx.workspace.display_path(&path),
            "objects": objects,
            "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        }))
    }

    async fn project_load(&self, p: ProjectLoadParams) -> ApiResult<Value> {
        let path = self.ctx.workspace.resolve(&p.path, PathAccess::Read)?;
        check_f3d(&path, &p.path)?;
        let p2 = path.clone();
        let loaded = blocking(move || forge3d_io::load_project(&p2).map_err(ApiError::from)).await?;
        let state = loaded.state();
        let label = format!(
            "Load project '{}'",
            path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
        );
        let origin = self.ctx.origin;
        let clear = p.clear_history;
        self.handle
            .run(move |doc| {
                let changes = doc.execute(Box::new(ReplaceState::new(state, label)), origin)?;
                let mut v = serde_json::to_value(mutation(doc, changes)).expect("serializes");
                if clear {
                    doc.clear_history();
                    v["history_entry"] = Value::Null;
                }
                v["history_cleared"] = json!(clear);
                v["scene"] = serde_json::to_value(info::scene_info(doc)?).expect("serializes");
                Ok::<_, ApiError>(v)
            })
            .await?
    }
}

fn check_f3d(path: &std::path::Path, input: &str) -> ApiResult<()> {
    let ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("f3d"));
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid_arguments(format!(
            "'{input}' is not a .f3d project path"
        ))
        .with_hint("Projects use the .f3d extension; use file_export/file_import for .glb, .gltf, .obj and .stl."))
    }
}

/// Run blocking work (file I/O, rendering) off the async executor.
pub(crate) async fn blocking<R: Send + 'static>(
    f: impl FnOnce() -> ApiResult<R> + Send + 'static,
) -> ApiResult<R> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("background task failed: {e}")))?
}

/// Replaces scene + selection (project_load); undo swaps the old content back.
struct ReplaceState {
    other: DocumentState,
    label: String,
}

impl ReplaceState {
    fn new(state: DocumentState, label: String) -> ReplaceState {
        ReplaceState {
            other: state,
            label,
        }
    }

    fn swap(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let mut summary = ChangeSummary::new();
        for id in doc.scene.traverse() {
            summary = summary.deleted(ObjectSummary::of(&doc.scene, id)?);
        }
        std::mem::swap(&mut doc.scene, &mut self.other.scene);
        std::mem::swap(&mut doc.selection, &mut self.other.selection);
        for id in doc.scene.traverse() {
            summary = summary.created(&doc.scene, id)?;
        }
        Ok(summary.with_selection_changed())
    }
}

impl Command for ReplaceState {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        self.swap(doc).map(|_| ())
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

// ---------------------------------------------------------------------------
// Query tools
// ---------------------------------------------------------------------------

const MESH_GET_CAP: usize = 1000;

fn page(total: usize, offset: usize, limit: usize) -> (usize, usize, Value) {
    let start = offset.min(total);
    let end = (start + limit).min(total);
    let next = if end < total { json!(end) } else { Value::Null };
    (start, end, next)
}

fn mesh_get(doc: &Document, p: MeshGetParams) -> ApiResult<Value> {
    let id = match info::resolve_opt(&doc.scene, &p.object)? {
        Some(id) => id,
        None => doc.selection.active().ok_or_else(|| {
            ApiError::new(
                "empty_selection",
                "no `object` given and there is no active object",
                "Pass `object` (id or unique name; see scene_get).",
            )
        })?,
    };
    if p.limit == 0 {
        return Err(ApiError::invalid_arguments("`limit` must be at least 1"));
    }
    let limit = p.limit.min(MESH_GET_CAP);
    let mesh = doc.scene.mesh(id)?;
    let node = doc.scene.node(id)?;
    let world = doc.scene.world_transform(id)?;
    let to_space = |v: glam::Vec3| match p.space {
        Space::Local => v,
        Space::World => world.transform_point3(v),
    };
    let nmat = normal_matrix(&world);
    let to_normal = |n: glam::Vec3| match p.space {
        Space::Local => n,
        Space::World => (nmat * n).normalize_or_zero(),
    };
    let mut out = json!({
        "object": id,
        "object_name": node.name,
        "space": p.space,
        "vertex_count": mesh.vertex_count(),
        "edge_count": mesh.edge_count(),
        "face_count": mesh.face_count(),
        "limit": limit,
        "limit_capped": p.limit > MESH_GET_CAP,
    });
    if let Some(b) = mesh.bounding_box() {
        out["local_bounds"] = serde_json::to_value(info::BoundsInfo::from(b)).expect("serializes");
    }
    for section in &p.include {
        match section {
            MeshSection::Vertices => {
                let (s, e, next) = page(mesh.vertex_count(), p.offset, limit);
                let items: Vec<Value> = (s..e)
                    .map(|i| {
                        let v = VertexId(i as u32);
                        let mut item = json!({
                            "index": i,
                            "position": round3(to_space(mesh.position(v)).to_array()),
                        });
                        if p.normals {
                            item["normal"] = json!(round3(to_normal(mesh.vertex_normal(v)).to_array()));
                        }
                        item
                    })
                    .collect();
                out["vertices"] = json!({"total": mesh.vertex_count(), "offset": s, "next_offset": next, "items": items});
            }
            MeshSection::Faces => {
                let (s, e, next) = page(mesh.face_count(), p.offset, limit);
                let items: Vec<Value> = (s..e)
                    .map(|i| {
                        let f = FaceId(i as u32);
                        let mut item = json!({
                            "index": i,
                            "vertices": mesh.face_vertices(f).map(|v| v.0).collect::<Vec<_>>(),
                            "center": round3(to_space(mesh.face_centroid(f)).to_array()),
                        });
                        if p.normals {
                            item["normal"] = json!(round3(to_normal(mesh.face_normal(f)).to_array()));
                        }
                        item
                    })
                    .collect();
                out["faces"] = json!({"total": mesh.face_count(), "offset": s, "next_offset": next, "items": items});
            }
            MeshSection::Edges => {
                let (s, e, next) = page(mesh.edge_count(), p.offset, limit);
                let items: Vec<Value> = (s..e)
                    .map(|i| {
                        let ed = EdgeId(i as u32);
                        let [a, b] = mesh.edge_vertices(ed);
                        json!({
                            "index": i,
                            "vertices": [a.0, b.0],
                            "faces": mesh.edge_face_count(ed),
                        })
                    })
                    .collect();
                out["edges"] = json!({"total": mesh.edge_count(), "offset": s, "next_offset": next, "items": items});
            }
        }
    }
    if p.validate {
        let r = mesh.validate();
        out["validation"] = json!({
            "is_valid": r.is_valid,
            "is_manifold": r.is_manifold,
            "is_closed": r.is_closed,
            "is_consistently_oriented": r.is_consistently_oriented,
            "euler_characteristic": r.euler_characteristic,
            "boundary_edges": r.boundary_edges,
            "non_manifold_edges": r.non_manifold_edges,
            "non_manifold_vertices": r.non_manifold_vertices,
            "degenerate_faces": r.degenerate_faces,
            "isolated_vertices": r.isolated_vertices,
            "issues": r.issues.iter().take(10).collect::<Vec<_>>(),
        });
    }
    Ok(out)
}

fn history_list(doc: &Document, p: HistoryListParams) -> Value {
    let all = doc.history_list();
    let limit = p.limit.clamp(1, 500);
    let skip = all.len().saturating_sub(limit);
    json!({
        "total": all.len(),
        "entries": &all[skip..],
        "truncated": skip > 0,
        "can_undo": doc.can_undo(),
        "can_redo": doc.can_redo(),
        "revision": doc.revision(),
    })
}

fn undo_redo(doc: &mut Document, p: StepsParams, redo: bool) -> ApiResult<Value> {
    if p.steps == 0 || p.steps > 100 {
        return Err(ApiError::invalid_arguments("`steps` must be in 1..=100"));
    }
    let mut done = Vec::new();
    let mut stopped = None;
    for _ in 0..p.steps {
        let r = if redo { doc.redo() } else { doc.undo() };
        match r {
            Ok(e) => done.push(e),
            Err(e) if done.is_empty() => return Err(e.into()),
            Err(e) => {
                stopped = Some(ApiError::from(e).to_json());
                break;
            }
        }
    }
    let changes: Vec<ChangeSummary> = done.iter().map(|e| e.summary.clone()).collect();
    Ok(json!({
        if redo { "redone" } else { "undone" }: done,
        "changes": changes,
        "stopped_early": stopped,
        "can_undo": doc.can_undo(),
        "can_redo": doc.can_redo(),
        "revision": doc.revision(),
    }))
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

const MAX_BATCH_STEPS: usize = 1000;

enum Step {
    Tool(String, Value),
    Command(CommandSpec),
}

impl Step {
    fn label(&self) -> String {
        match self {
            Step::Tool(n, _) => n.clone(),
            Step::Command(c) => format!("command:{}", c.op_name()),
        }
    }
}

fn parse_step(s: BatchStepSchema) -> ApiResult<Step> {
    match (s.tool, s.command) {
        (Some(tool), None) => {
            if !TOOLS.iter().any(|t| t.name == tool) {
                return Err(ApiError::unknown_tool(&tool, &batchable_names()));
            }
            Ok(Step::Tool(tool, Value::Object(s.arguments.unwrap_or_default())))
        }
        (None, Some(cmd)) => {
            if s.arguments.is_some() {
                return Err(ApiError::invalid_arguments("`arguments` belongs to `tool` steps; a `command` step carries its fields inside `command`"));
            }
            let spec: CommandSpec = serde_json::from_value(Value::Object(cmd))
                .map_err(|e| forge3d_core::error::CoreError::InvalidCommand(e.to_string()))?;
            Ok(Step::Command(spec))
        }
        (Some(_), Some(_)) => Err(ApiError::invalid_arguments("a step has either `tool` or `command`, not both")),
        (None, None) => Err(ApiError::invalid_arguments(
            "a step needs `tool` (+ `arguments`) or `command`",
        )),
    }
}

fn batch_error(index: usize, label: &str, e: ApiError) -> ApiError {
    let mut step_error = e.to_json();
    if let Value::Object(m) = &mut step_error {
        m.remove("details");
    }
    ApiError {
        code: "batch_failed".into(),
        message: format!(
            "batch step {index} ({label}) failed and the whole batch was rolled back: {}",
            e.message
        ),
        hint: format!(
            "Fix step {index} ({}: {}) and resend the whole batch; nothing from this batch was applied.",
            e.code, e.hint
        ),
        details: Some(json!({ "index": index, "step": label, "step_error": step_error, "step_details": e.details })),
    }
}

fn rollback(doc: &mut Document, applied: &mut [Box<dyn Command>], before: &forge3d_core::selection::Selection) -> ApiResult<()> {
    for c in applied.iter_mut().rev() {
        c.undo(doc).map_err(|e| ApiError::internal(format!("batch rollback failed: {e}")))?;
    }
    doc.selection = before.clone();
    Ok(())
}

fn run_batch(doc: &mut Document, p: BatchParams, ctx: &Ctx) -> ApiResult<Value> {
    if p.steps.is_empty() {
        return Err(ApiError::invalid_arguments("`steps` must contain at least one step"));
    }
    if p.steps.len() > MAX_BATCH_STEPS {
        return Err(ApiError::invalid_arguments(format!(
            "a batch may contain at most {MAX_BATCH_STEPS} steps"
        )));
    }
    let mut steps = Vec::with_capacity(p.steps.len());
    for (i, s) in p.steps.into_iter().enumerate() {
        let label = s.tool.clone().unwrap_or_else(|| "command".into());
        steps.push(parse_step(s).map_err(|e| batch_error(i, &label, e))?);
    }

    // Pass 1: plan + apply step by step so later steps see earlier results
    // (e.g. reference a just-created object by name).
    let before = doc.selection.clone();
    let mut applied: Vec<Box<dyn Command>> = Vec::with_capacity(steps.len());
    let mut results = Vec::with_capacity(steps.len());
    for (i, step) in steps.into_iter().enumerate() {
        let label = step.label();
        let planned = match step {
            Step::Tool(name, args) => {
                if !TOOLS.iter().any(|t| t.name == name && t.batchable) {
                    Err(ApiError::invalid_arguments(format!("tool '{name}' cannot be a batch step"))
                        .with_hint(format!("Batch steps can use: {}.", batchable_names().join(", "))))
                } else {
                    plan_tool(doc, &name, args, ctx)
                }
            }
            Step::Command(spec) => spec.into_command().map(|command| Plan { command, post: None }).map_err(ApiError::from),
        };
        let Plan { mut command, post } = match planned {
            Ok(p) => p,
            Err(e) => {
                rollback(doc, &mut applied, &before)?;
                return Err(batch_error(i, &label, e));
            }
        };
        match command.apply(doc) {
            Ok(changes) => {
                doc.selection.prune(&doc.scene);
                let extra = match post {
                    Some(f) => f(doc, &changes),
                    None => Ok(json!({})),
                };
                let extra = match extra {
                    Ok(v) => v,
                    Err(e) => {
                        applied.push(command);
                        rollback(doc, &mut applied, &before)?;
                        return Err(batch_error(i, &label, e));
                    }
                };
                results.push(merge_into(
                    json!({ "index": i, "step": label, "description": command.describe(), "changes": changes }),
                    extra,
                ));
                applied.push(command);
            }
            Err(e) => {
                rollback(doc, &mut applied, &before)?;
                return Err(batch_error(i, &label, e.into()));
            }
        }
    }

    // Pass 2: roll back and record everything as one history entry; the
    // commands cache their resolved targets so re-applying is deterministic.
    rollback(doc, &mut applied, &before)?;
    let seq = Sequence::new(applied, p.description);
    let changes = doc.execute(Box::new(seq), ctx.origin)?;
    let mut v = serde_json::to_value(mutation(doc, changes)).expect("serializes");
    v["steps"] = Value::Array(results);
    Ok(v)
}

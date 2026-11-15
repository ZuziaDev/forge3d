//! Application state machine, independent of winit / wgpu / egui.
//!
//! [`AppState`] owns the [`Document`] and everything the UI needs besides
//! it (camera, tool, drag sessions, status). All scene changes are executed
//! as [`CommandSpec`]s with [`Origin::Gui`]; live previews (gizmo drags,
//! drag-value edits) apply a command and replace it on every update, so one
//! interaction always ends up as exactly one undo entry.

use std::path::{Path, PathBuf};

use forge3d_core::command::Origin;
use forge3d_core::math::Aabb;
use forge3d_core::prelude::*;
use forge3d_core::scene::Material;
use forge3d_io::{ExportOptions, ImportOptions};
use forge3d_render::camera::scene_bounds;
use forge3d_render::{OrbitCamera, PickHit, ViewOptions, ViewPreset};
use glam::{Vec2, Vec3};
use uuid::Uuid;

use crate::actions::{
    boolean_spec, create_primitive, mesh_tool_spec, MeshTool, PrimitiveDraft, ToolParams,
};
use crate::camera_input;
use crate::error::AppError;
use crate::gizmo::{DragDelta, GizmoDrag, GizmoMode, Handle};
use crate::target::GizmoTarget;

/// Severity of a status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Error,
}

/// The last command result, shown in the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub kind: StatusKind,
    pub text: String,
}

impl Status {
    pub fn info(text: impl Into<String>) -> Status {
        Status {
            kind: StatusKind::Info,
            text: text.into(),
        }
    }

    /// `code: message — hint`
    pub fn error(code: &str, message: &str, hint: &str) -> Status {
        Status {
            kind: StatusKind::Error,
            text: format!("{code}: {message} \u{2014} {hint}"),
        }
    }
}

/// A click in the viewport waiting to be resolved by the GPU picker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRequest {
    /// Pixel position inside the viewport texture (top-left origin).
    pub pos: Vec2,
    /// Shift held: add to / toggle the selection.
    pub shift: bool,
}

/// An applied-but-not-yet-final command that is replaced on every update.
#[derive(Debug, Clone)]
struct Preview {
    key: String,
    seq: u64,
}

/// A running gizmo drag. The live preview is applied to a scratch copy of
/// the document (through the same command API); the real document gets a
/// single command when the drag ends, and nothing at all when it is cancelled.
struct ActiveDrag {
    drag: GizmoDrag,
    target: GizmoTarget,
    scratch: Document,
    /// The scratch document currently holds the preview command.
    applied: bool,
    /// Revision of the real document at drag start.
    base_revision: u64,
    serial: u64,
}

/// Kinds of the file dialog window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDialogKind {
    Open,
    SaveAs,
    Import,
    Export,
}

impl FileDialogKind {
    pub fn title(self) -> &'static str {
        match self {
            FileDialogKind::Open => "Open project",
            FileDialogKind::SaveAs => "Save project as",
            FileDialogKind::Import => "Import file",
            FileDialogKind::Export => "Export file",
        }
    }
}

/// An open file dialog (a typed path; no native dialog dependency).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDialog {
    pub kind: FileDialogKind,
    pub path: String,
}

/// UI-only state kept next to the document.
#[derive(Default)]
pub struct UiState {
    /// Object being renamed in the outliner, with the edit buffer.
    pub renaming: Option<(Uuid, String)>,
    pub file_dialog: Option<FileDialog>,
    pub primitive_dialog: Option<PrimitiveDraft>,
    /// Cached Euler angles so that edited values don't jump (object, rotation, angles).
    pub euler_cache: Option<(Uuid, glam::Quat, [f32; 3])>,
    /// Cached mesh validation: (node, document revision, report).
    pub validation: Option<(Uuid, u64, ValidationReport)>,
    pub show_history: bool,
    pub show_outliner: bool,
    pub show_properties: bool,
}

/// High-level user actions shared by menus, toolbar buttons and shortcuts.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Undo,
    Redo,
    SelectAll,
    SelectNone,
    SelectInvert,
    SetMode(SelectMode),
    /// Tab: object mode <-> the last element mode.
    ToggleEditMode,
    SetTool(GizmoMode),
    FrameSelected,
    FrameAll,
    View(ViewPreset),
    ToggleOrtho,
    ToggleWireframe,
    ToggleGrid,
    ToggleSmooth,
    DeleteSelected,
    DuplicateSelected,
    AddPrimitive(PrimitiveSpec),
    MeshTool(MeshTool),
    Boolean(BooleanOp),
    /// Escape: cancel a running drag.
    Cancel,
}

/// The whole GUI state.
pub struct AppState {
    pub doc: Document,
    pub tool: GizmoMode,
    pub camera: OrbitCamera,
    pub view: ViewOptions,
    pub status: Status,
    pub params: ToolParams,
    pub ui: UiState,
    /// Size of the viewport in points (for projection math).
    pub viewport: (u32, u32),
    /// Project file the document was opened from / saved to.
    pub path: Option<PathBuf>,
    pub should_exit: bool,
    last_edit_mode: SelectMode,
    saved_revision: u64,
    preview: Option<Preview>,
    drag: Option<ActiveDrag>,
    hovered: Option<Handle>,
    picks: Vec<PickRequest>,
    drag_serial: u64,
}

impl AppState {
    pub fn new(doc: Document) -> AppState {
        let saved_revision = doc.revision();
        let mut s = AppState {
            doc,
            tool: GizmoMode::Translate,
            camera: OrbitCamera::default(),
            view: ViewOptions::default(),
            status: Status::info("Ready"),
            params: ToolParams::default(),
            ui: UiState {
                show_history: true,
                show_outliner: true,
                show_properties: true,
                ..UiState::default()
            },
            viewport: (1280, 720),
            path: None,
            should_exit: false,
            last_edit_mode: SelectMode::Face,
            saved_revision,
            preview: None,
            drag: None,
            hovered: None,
            picks: Vec::new(),
            drag_serial: 0,
        };
        s.frame(false);
        s
    }

    // ----- status -----------------------------------------------------

    /// Show a structured error in the status bar.
    pub fn report(&mut self, err: &AppError) {
        self.status = Status::error(err.code(), &err.to_string(), &err.hint());
    }

    fn report_core(&mut self, err: &CoreError) {
        self.status = Status::error(err.code(), &err.to_string(), &err.hint());
    }

    fn done_status(&mut self) {
        let label = self
            .doc
            .history_list()
            .into_iter()
            .rev()
            .find(|e| !e.undone)
            .map(|e| e.label)
            .unwrap_or_else(|| "Done".into());
        self.status = Status::info(label);
    }

    /// True when the document changed since the last save / load.
    pub fn is_dirty(&self) -> bool {
        self.doc.revision() != self.saved_revision
    }

    // ----- command execution -------------------------------------------

    /// Execute a command as the GUI. Returns false (and shows the error in
    /// the status bar) when it failed; the document is unchanged then.
    pub fn run(&mut self, spec: CommandSpec) -> bool {
        self.preview = None;
        match self.doc.execute_spec(spec, Origin::Gui) {
            Ok(_) => {
                self.done_status();
                true
            }
            Err(e) => {
                self.report_core(&e);
                false
            }
        }
    }

    pub fn undo(&mut self) {
        self.end_interactions();
        match self.doc.undo() {
            Ok(e) => self.status = Status::info(format!("Undo: {}", e.label)),
            Err(e) => self.report_core(&e),
        }
    }

    pub fn redo(&mut self) {
        self.end_interactions();
        match self.doc.redo() {
            Ok(e) => self.status = Status::info(format!("Redo: {}", e.label)),
            Err(e) => self.report_core(&e),
        }
    }

    /// Undo / redo until the history entry `seq` is the last applied one
    /// (`None` = before the first entry).
    pub fn history_goto(&mut self, seq: Option<u64>) {
        self.end_interactions();
        let list = self.doc.history_list();
        let target = match seq {
            Some(s) => match list.iter().position(|e| e.seq == s) {
                Some(i) => i + 1,
                None => return,
            },
            None => 0,
        };
        let mut applied = list.iter().filter(|e| !e.undone).count();
        while applied > target {
            if let Err(e) = self.doc.undo() {
                self.report_core(&e);
                return;
            }
            applied -= 1;
        }
        while applied < target {
            if let Err(e) = self.doc.redo() {
                self.report_core(&e);
                return;
            }
            applied += 1;
        }
        self.status = Status::info("History position changed");
    }

    // ----- previews (coalesced edits) -----------------------------------

    fn last_applied_seq(&self) -> Option<u64> {
        self.doc
            .history_list()
            .iter()
            .rev()
            .find(|e| !e.undone)
            .map(|e| e.seq)
    }

    /// True when the open preview is still the newest applied history entry
    /// (nobody else executed a command in between).
    fn preview_is_top(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|p| self.last_applied_seq() == Some(p.seq))
    }

    /// Replace the preview with key `key` by `spec`. The first call of a
    /// series executes the command; later calls undo the previous result and
    /// execute the new one, so the series is a single history entry. A
    /// preview whose entry was buried by foreign commands is left as is
    /// (committed) and a new one is started.
    pub fn preview(&mut self, key: &str, spec: CommandSpec) -> bool {
        if let Some(p) = &self.preview {
            if p.key == key && self.preview_is_top() {
                if let Err(e) = self.doc.undo() {
                    self.preview = None;
                    self.report_core(&e);
                    return false;
                }
            }
        }
        self.preview = None;
        match self.doc.execute_spec(spec, Origin::Gui) {
            Ok(_) => {
                if let Some(seq) = self.last_applied_seq() {
                    self.preview = Some(Preview {
                        key: key.to_string(),
                        seq,
                    });
                }
                self.done_status();
                true
            }
            Err(e) => {
                self.report_core(&e);
                false
            }
        }
    }

    /// Finish the current preview series; its entry stays in the history.
    pub fn commit_preview(&mut self) {
        self.preview = None;
    }

    /// Call once per frame. `busy` is true while a pointer button is held or
    /// a text field has keyboard focus; when it ends, property edits commit.
    pub fn end_frame(&mut self, busy: bool) {
        if !busy && self.drag.is_none() {
            self.preview = None;
        }
    }

    /// Finish a drag and any preview before an unrelated command.
    fn end_interactions(&mut self) {
        self.drag = None;
        self.preview = None;
    }

    // ----- document transforms from the properties panel -----------------

    /// Live-edit the transform of `object` (one undo entry per interaction).
    pub fn preview_transform(
        &mut self,
        object: Uuid,
        translation: Option<[f32; 3]>,
        rotation_euler_deg: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    ) -> bool {
        self.preview(
            &format!("transform:{object}"),
            CommandSpec::SetTransform {
                objects: Some(vec![object]),
                translation,
                rotation_euler_deg,
                scale,
            },
        )
    }

    /// Live-edit the material of `object`.
    pub fn preview_material(&mut self, object: Uuid, material: Material) -> bool {
        self.preview(
            &format!("material:{object}"),
            CommandSpec::SetMaterial {
                objects: Some(vec![object]),
                material: Some(material),
            },
        )
    }

    // ----- external changes ---------------------------------------------

    /// Re-validate UI state after the document may have been changed by
    /// somebody else (e.g. the MCP frame hook): drags whose target vanished
    /// or whose preview was buried end, dangling UI references are dropped.
    pub fn sync_external(&mut self) {
        if let Some(d) = &self.drag {
            if d.base_revision != self.doc.revision() || !d.target.is_valid(&self.doc) {
                self.drag = None;
                self.status = Status::info("Drag cancelled: the scene was changed externally");
            }
        }
        if self.preview.is_some() && !self.preview_is_top() {
            self.preview = None;
        }
        if let Some((id, _)) = &self.ui.renaming {
            if !self.doc.scene.contains(*id) {
                self.ui.renaming = None;
            }
        }
        if let Some((id, _, _)) = &self.ui.euler_cache {
            if !self.doc.scene.contains(*id) {
                self.ui.euler_cache = None;
            }
        }
    }

    // ----- selection ------------------------------------------------------

    pub fn set_mode(&mut self, mode: SelectMode) {
        if mode == self.doc.selection.mode {
            return;
        }
        self.end_interactions();
        if mode != SelectMode::Object {
            self.last_edit_mode = mode;
        }
        self.run(CommandSpec::SetSelectMode { mode });
    }

    pub fn toggle_edit_mode(&mut self) {
        let next = if self.doc.selection.mode == SelectMode::Object {
            self.last_edit_mode
        } else {
            SelectMode::Object
        };
        self.set_mode(next);
    }

    pub fn queue_pick(&mut self, req: PickRequest) {
        self.picks.push(req);
    }

    pub fn take_picks(&mut self) -> Vec<PickRequest> {
        std::mem::take(&mut self.picks)
    }

    /// Turn a pick result (or a click on empty space) into selection commands.
    pub fn apply_pick(&mut self, hit: Option<PickHit>, shift: bool) {
        self.end_interactions();
        let mode = self.doc.selection.mode;
        let active = self.doc.selection.active();
        let Some(hit) = hit else {
            if !shift {
                let object = (mode != SelectMode::Object).then_some(active).flatten();
                self.run(CommandSpec::SelectNone { object });
            }
            return;
        };
        let node = hit.node_id;
        match mode.element_kind() {
            Some(kind) if Some(node) == active => {
                let index = match kind {
                    ElementKind::Vertex => hit.vertex,
                    ElementKind::Edge => hit.edge,
                    ElementKind::Face => hit.face,
                };
                let selected = self
                    .doc
                    .selection
                    .elements(node)
                    .is_some_and(|e| e.set(kind).contains(&index));
                let action = match (shift, selected) {
                    (false, _) => SelectAction::Set,
                    (true, true) => SelectAction::Remove,
                    (true, false) => SelectAction::Add,
                };
                self.run(CommandSpec::SelectElements {
                    object: Some(node),
                    kind: Some(kind),
                    indices: vec![index],
                    action,
                });
            }
            // Edit mode, another object: make it the edited object.
            Some(_) => {
                self.run(CommandSpec::SelectObjects {
                    objects: vec![node],
                    action: SelectAction::Set,
                });
            }
            None => {
                let selected = self.doc.selection.is_selected(node);
                let spec = match (shift, selected) {
                    (false, _) => CommandSpec::SelectObjects {
                        objects: vec![node],
                        action: SelectAction::Set,
                    },
                    (true, false) => CommandSpec::SelectObjects {
                        objects: vec![node],
                        action: SelectAction::Add,
                    },
                    (true, true) if active != Some(node) => {
                        CommandSpec::SetActiveObject { object: node }
                    }
                    (true, true) => CommandSpec::SelectObjects {
                        objects: vec![node],
                        action: SelectAction::Remove,
                    },
                };
                self.run(spec);
            }
        }
    }

    /// Select exactly one object (outliner click) or add / toggle with `shift`.
    pub fn select_object(&mut self, id: Uuid, shift: bool) {
        self.end_interactions();
        let selected = self.doc.selection.is_selected(id);
        let spec = if !shift {
            CommandSpec::SelectObjects {
                objects: vec![id],
                action: SelectAction::Set,
            }
        } else if selected && self.doc.selection.active() != Some(id) {
            CommandSpec::SetActiveObject { object: id }
        } else if selected {
            CommandSpec::SelectObjects {
                objects: vec![id],
                action: SelectAction::Remove,
            }
        } else {
            CommandSpec::SelectObjects {
                objects: vec![id],
                action: SelectAction::Add,
            }
        };
        self.run(spec);
    }

    // ----- object management ---------------------------------------------

    pub fn rename(&mut self, id: Uuid, name: String) {
        self.run(CommandSpec::RenameObject {
            object: Some(id),
            name,
        });
    }

    pub fn set_visible(&mut self, id: Uuid, visible: bool) {
        self.run(CommandSpec::SetVisibility {
            objects: Some(vec![id]),
            visible,
        });
    }

    pub fn delete_objects(&mut self, ids: Vec<Uuid>) {
        self.run(CommandSpec::DeleteObjects {
            objects: Some(ids),
            keep_children: false,
        });
    }

    pub fn reparent(&mut self, object: Uuid, parent: Option<Uuid>) {
        self.run(CommandSpec::Reparent {
            object: Some(object),
            parent,
            keep_world_transform: true,
        });
    }

    // ----- actions ----------------------------------------------------------

    /// Perform a user action (menu, toolbar or shortcut).
    pub fn perform(&mut self, action: Action) {
        match action {
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::SelectAll => {
                self.end_interactions();
                let active = self.doc.selection.active();
                match self.doc.selection.mode.element_kind() {
                    None => {
                        self.run(CommandSpec::SelectAll {
                            object: None,
                            kind: None,
                        });
                    }
                    Some(kind) => {
                        self.run(CommandSpec::SelectAll {
                            object: active,
                            kind: Some(kind),
                        });
                    }
                }
            }
            Action::SelectNone => {
                self.end_interactions();
                let active = self.doc.selection.active();
                self.run(CommandSpec::SelectNone { object: active });
            }
            Action::SelectInvert => {
                self.end_interactions();
                let active = self.doc.selection.active();
                self.run(CommandSpec::SelectInvert {
                    object: active,
                    kind: self.doc.selection.mode.element_kind(),
                });
            }
            Action::SetMode(m) => self.set_mode(m),
            Action::ToggleEditMode => self.toggle_edit_mode(),
            Action::SetTool(t) => self.tool = t,
            Action::FrameSelected => self.frame(true),
            Action::FrameAll => self.frame(false),
            Action::View(p) => camera_input::apply_preset(&mut self.camera, p),
            Action::ToggleOrtho => self.camera.orthographic = !self.camera.orthographic,
            Action::ToggleWireframe => self.view.wireframe = !self.view.wireframe,
            Action::ToggleGrid => self.view.grid = !self.view.grid,
            Action::ToggleSmooth => self.view.smooth_shading = !self.view.smooth_shading,
            Action::DeleteSelected => self.delete_selected(),
            Action::DuplicateSelected => {
                self.run(CommandSpec::DuplicateObjects {
                    objects: None,
                    offset: self.params.duplicate_offset,
                });
            }
            Action::AddPrimitive(p) => {
                self.run(create_primitive(p));
            }
            Action::MeshTool(t) => match mesh_tool_spec(t, &self.doc, &self.params) {
                Ok(spec) => {
                    self.run(spec);
                }
                Err(e) => self.report(&e),
            },
            Action::Boolean(op) => match boolean_spec(&self.doc, op) {
                Ok(spec) => {
                    self.run(spec);
                }
                Err(e) => self.report(&e),
            },
            Action::Cancel => self.cancel_drag(),
        }
    }

    /// Delete: selected elements in edit modes, selected objects otherwise.
    fn delete_selected(&mut self) {
        self.end_interactions();
        match self.doc.selection.mode.element_kind() {
            Some(_) => self.perform(Action::MeshTool(MeshTool::DeleteElements)),
            None => {
                let ids: Vec<Uuid> = self.doc.selection.objects().to_vec();
                if ids.is_empty() {
                    self.status = Status::info("Nothing selected");
                } else {
                    self.delete_objects(ids);
                }
            }
        }
    }

    // ----- camera ---------------------------------------------------------

    /// Bounds of the selection (elements in edit modes, objects otherwise).
    pub fn selection_bounds(&self) -> Option<Aabb> {
        if self.doc.selection.mode != SelectMode::Object {
            if let Some(t) = GizmoTarget::compute(&self.doc) {
                if let crate::target::TargetKind::Elements { object, kind, indices, world } = &t.kind {
                    let mesh = self.doc.scene.get(*object)?.mesh.as_ref()?;
                    let mut only = ElementSelection::default();
                    only.set_mut(*kind).extend(indices.iter().copied());
                    let pts = only
                        .touched_vertices(mesh)
                        .into_iter()
                        .map(|v| world.transform_point3(mesh.position(v)));
                    return Aabb::from_points(pts);
                }
            }
        }
        let ids: Vec<Uuid> = self.doc.selection.objects().to_vec();
        if ids.is_empty() {
            return None;
        }
        scene_bounds(&self.doc.scene, &ids).ok().flatten()
    }

    /// Frame the selection (`selected`) or the whole scene. Falls back to
    /// the whole scene when nothing is selected.
    pub fn frame(&mut self, selected: bool) {
        let aspect = self.viewport.0.max(1) as f32 / self.viewport.1.max(1) as f32;
        let bounds = if selected { self.selection_bounds() } else { None }
            .or_else(|| scene_bounds(&self.doc.scene, &[]).ok().flatten());
        camera_input::frame(&mut self.camera, bounds, aspect);
    }

    // ----- gizmo ------------------------------------------------------------

    /// The document to display and pick against: the scratch copy holding
    /// the live preview while a gizmo drag runs, the real document otherwise.
    pub fn display_doc(&self) -> &Document {
        self.drag.as_ref().map_or(&self.doc, |d| &d.scratch)
    }

    /// Cache key for the renderer matching [`AppState::display_doc`].
    pub fn display_revision(&self) -> u64 {
        match &self.drag {
            Some(d) => (1 << 62) | (d.serial << 24) | (d.scratch.revision() & 0xff_ffff),
            None => self.doc.revision(),
        }
    }

    /// Current gizmo target (None when nothing transformable is selected).
    /// While dragging it follows the live preview.
    pub fn gizmo_target(&self) -> Option<GizmoTarget> {
        GizmoTarget::compute(self.display_doc())
    }

    pub fn hovered_handle(&self) -> Option<Handle> {
        self.hovered
    }

    pub fn set_hovered_handle(&mut self, h: Option<Handle>) {
        self.hovered = h;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Handle of the running drag.
    pub fn drag_handle(&self) -> Option<Handle> {
        self.drag.as_ref().map(|d| d.drag.handle)
    }

    /// Start dragging `handle` of the current gizmo. `cursor` is in
    /// viewport points.
    pub fn begin_gizmo_drag(&mut self, handle: Handle, cursor: Vec2) -> bool {
        self.end_interactions();
        let Some(target) = GizmoTarget::compute(&self.doc) else {
            return false;
        };
        let frame = target.frame(self.tool);
        let cam = self.camera.to_camera();
        let drag = GizmoDrag::begin(self.tool, handle, frame, &cam, self.viewport, cursor);
        self.drag_serial += 1;
        self.drag = Some(ActiveDrag {
            drag,
            target,
            scratch: Document::from_state(self.doc.state()),
            applied: false,
            base_revision: self.doc.revision(),
            serial: self.drag_serial,
        });
        true
    }

    /// Update the running drag and its live preview. Returns false when the
    /// drag ended (target vanished or the scene was changed externally).
    pub fn update_gizmo_drag(&mut self, cursor: Vec2, snap: bool) -> bool {
        let Some(mut active) = self.drag.take() else {
            return false;
        };
        if active.base_revision != self.doc.revision() || !active.target.is_valid(&self.doc) {
            self.status = Status::info("Drag cancelled: the scene was changed externally");
            return false;
        }
        let cam = self.camera.to_camera();
        let snap_step = cam.grid_cell_size() * 0.25;
        let delta: DragDelta = active
            .drag
            .update(&cam, self.viewport, cursor, snap, snap_step);
        if active.applied {
            if let Err(e) = active.scratch.undo() {
                self.report_core(&e);
                return false;
            }
            active.applied = false;
        }
        if !delta.is_identity() {
            if let Err(e) = active
                .scratch
                .execute_spec(active.target.spec(delta), Origin::Gui)
            {
                self.report_core(&e);
                return false;
            }
            active.applied = true;
        }
        self.drag = Some(active);
        true
    }

    /// Finish the drag: the real document receives one command with the
    /// accumulated transform. Returns whether anything was committed.
    pub fn end_gizmo_drag(&mut self) -> bool {
        let Some(active) = self.drag.take() else {
            return false;
        };
        if !active.applied {
            return false;
        }
        if active.base_revision != self.doc.revision() || !active.target.is_valid(&self.doc) {
            self.status = Status::info("Drag cancelled: the scene was changed externally");
            return false;
        }
        self.run(active.target.spec(active.drag.delta()))
    }

    /// Abort the drag; the real document was never touched (Escape).
    pub fn cancel_drag(&mut self) {
        if self.drag.take().is_some() {
            self.status = Status::info("Cancelled");
        }
    }

    // ----- files ----------------------------------------------------------------

    /// Start with an empty document.
    pub fn new_document(&mut self) {
        self.replace_document(Document::new(), None);
        self.status = Status::info("New scene");
    }

    fn replace_document(&mut self, doc: Document, path: Option<PathBuf>) {
        self.drag = None;
        self.preview = None;
        self.picks.clear();
        self.ui.renaming = None;
        self.ui.euler_cache = None;
        self.ui.validation = None;
        self.doc = doc;
        self.path = path;
        self.saved_revision = self.doc.revision();
        self.frame(false);
    }

    pub fn open_project(&mut self, path: &Path) {
        match forge3d_io::load_project(path) {
            Ok(doc) => {
                self.replace_document(doc, Some(path.to_path_buf()));
                self.status = Status::info(format!("Opened {}", path.display()));
            }
            Err(e) => self.report(&AppError::Io(e)),
        }
    }

    pub fn save_project(&mut self, path: &Path) {
        match forge3d_io::save_project(&self.doc, path) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.saved_revision = self.doc.revision();
                self.status = Status::info(format!("Saved {}", path.display()));
            }
            Err(e) => self.report(&AppError::Io(e)),
        }
    }

    /// Import a file as one undoable command.
    pub fn import_file(&mut self, path: &Path) {
        self.end_interactions();
        match forge3d_io::import_file_command(path, &ImportOptions::default(), None) {
            Ok((cmd, warnings)) => match self.doc.execute(cmd, Origin::Gui) {
                Ok(_) => {
                    self.frame(false);
                    self.status = if warnings.is_empty() {
                        Status::info(format!("Imported {}", path.display()))
                    } else {
                        Status::info(format!(
                            "Imported {} ({} warnings: {})",
                            path.display(),
                            warnings.len(),
                            warnings[0]
                        ))
                    };
                }
                Err(e) => self.report_core(&e),
            },
            Err(e) => self.report(&AppError::Io(e)),
        }
    }

    pub fn export_file(&mut self, path: &Path) {
        match forge3d_io::export_scene(&self.doc.scene, path, &ExportOptions::default()) {
            Ok(files) => {
                self.status = Status::info(format!("Exported {} file(s) to {}", files.len(), path.display()));
            }
            Err(e) => self.report(&AppError::Io(e)),
        }
    }

    /// Execute the open file dialog with its typed path.
    pub fn confirm_file_dialog(&mut self) {
        let Some(dlg) = self.ui.file_dialog.take() else {
            return;
        };
        let path = PathBuf::from(dlg.path.trim());
        if dlg.path.trim().is_empty() {
            self.report(&AppError::Usage("enter a file path".into()));
            self.ui.file_dialog = Some(dlg);
            return;
        }
        match dlg.kind {
            FileDialogKind::Open => self.open_project(&path),
            FileDialogKind::SaveAs => {
                let path = if path.extension().is_none() {
                    path.with_extension("f3d")
                } else {
                    path
                };
                self.save_project(&path);
            }
            FileDialogKind::Import => self.import_file(&path),
            FileDialogKind::Export => self.export_file(&path),
        }
    }

    /// Save to the current path, or open the Save As dialog.
    pub fn save(&mut self) {
        match self.path.clone() {
            Some(p) => self.save_project(&p),
            None => {
                self.ui.file_dialog = Some(FileDialog {
                    kind: FileDialogKind::SaveAs,
                    path: "scene.f3d".into(),
                })
            }
        }
    }

    /// World-space point under the gizmo origin helper for tests and UI.
    pub fn gizmo_origin(&self) -> Option<Vec3> {
        self.gizmo_target().map(|t| t.origin)
    }

    /// Cached mesh validation report for the properties panel.
    pub fn validation_for(&mut self, id: Uuid) -> Option<ValidationReport> {
        let rev = self.doc.revision();
        if let Some((vid, vrev, rep)) = &self.ui.validation {
            if *vid == id && *vrev == rev {
                return Some(rep.clone());
            }
        }
        let rep = self.doc.scene.get(id)?.mesh.as_ref()?.validate();
        self.ui.validation = Some((id, rev, rep.clone()));
        Some(rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::{self, hit_test};

    fn cube_state() -> (AppState, Uuid) {
        let mut st = AppState::new(Document::new());
        assert!(st.run(create_primitive(PrimitiveSpec::Cube { size: 2.0 })));
        let id = st.doc.selection.active().unwrap();
        (st, id)
    }

    fn screen(st: &AppState, p: Vec3) -> Vec2 {
        gizmo::project(&st.camera.to_camera(), p, st.viewport).unwrap()
    }

    fn handle_point(st: &AppState, axis: usize) -> Vec2 {
        let t = st.gizmo_target().unwrap();
        let frame = t.frame(st.tool);
        let len = gizmo::axis_length(&st.camera.to_camera(), &frame, st.viewport);
        screen(st, frame.origin + frame.axes[axis] * len * 0.5)
    }

    fn applied(st: &AppState) -> usize {
        st.doc.history_list().iter().filter(|e| !e.undone).count()
    }

    #[test]
    fn translate_drag_is_one_history_entry_and_undo_restores_exactly() {
        let (mut st, id) = cube_state();
        let before = st.doc.state();
        let entries = applied(&st);
        let start = handle_point(&st, 0);
        let h = hit_test(
            st.tool,
            &st.gizmo_target().unwrap().frame(st.tool),
            &st.camera.to_camera(),
            st.viewport,
            start,
        );
        assert_eq!(h, Some(Handle::Axis(0)));
        assert!(st.begin_gizmo_drag(h.unwrap(), start));
        for k in 1..=12 {
            let p = start + Vec2::new(k as f32 * 7.0, k as f32 * 1.5);
            assert!(st.update_gizmo_drag(p, false));
        }
        assert!(st.end_gizmo_drag());
        assert_eq!(applied(&st), entries + 1, "exactly one entry for the drag");
        let moved = st.doc.scene.world_transform(id).unwrap().translation;
        assert!(moved.x.abs() > 0.1 && moved.y.abs() < 1e-5 && moved.z.abs() < 1e-5);
        st.undo();
        assert_eq!(st.doc.state(), before, "undo restores the original state exactly");
        assert_eq!(st.doc.history_list().iter().filter(|e| !e.undone).count(), entries);
    }

    #[test]
    fn rotate_and_scale_drags_commit_once() {
        let (mut st, id) = cube_state();
        let before = st.doc.state();
        // rotate around Z using the ring point nearest the camera
        st.perform(Action::SetTool(GizmoMode::Rotate));
        let cam = st.camera.to_camera();
        let frame = st.gizmo_target().unwrap().frame(st.tool);
        let r = gizmo::axis_length(&cam, &frame, st.viewport) * 0.85;
        let toward = (cam.eye - frame.origin).truncate().normalize();
        let a = Vec3::new(toward.x, toward.y, 0.0);
        let b = Vec3::new(-toward.y, toward.x, 0.0);
        let p0 = screen(&st, a * r);
        let h = hit_test(st.tool, &frame, &cam, st.viewport, p0).unwrap();
        assert_eq!(h, Handle::Axis(2));
        let n = applied(&st);
        assert!(st.begin_gizmo_drag(h, p0));
        for k in 1..=5 {
            let ang = k as f32 * 0.15;
            let p = screen(&st, (a * ang.cos() + b * ang.sin()) * r);
            assert!(st.update_gizmo_drag(p, false));
        }
        st.end_gizmo_drag();
        assert_eq!(applied(&st), n + 1);
        let (_, rot, _) = st
            .doc
            .scene
            .get(id)
            .unwrap()
            .transform
            .to_scale_rotation_translation();
        assert!(rot.to_axis_angle().1.abs() > 0.3);

        // scale along X
        st.perform(Action::SetTool(GizmoMode::Scale));
        let frame = st.gizmo_target().unwrap().frame(st.tool);
        let cam = st.camera.to_camera();
        let len = gizmo::axis_length(&cam, &frame, st.viewport);
        let p0 = screen(&st, frame.origin + frame.axes[0] * len * 0.9);
        let h = hit_test(st.tool, &frame, &cam, st.viewport, p0).unwrap();
        assert_eq!(h, Handle::Axis(0));
        let n = applied(&st);
        assert!(st.begin_gizmo_drag(h, p0));
        let p1 = screen(&st, frame.origin + frame.axes[0] * len * 1.8);
        assert!(st.update_gizmo_drag(p1, false));
        st.end_gizmo_drag();
        assert_eq!(applied(&st), n + 1);
        st.undo();
        st.undo();
        assert_eq!(st.doc.state(), before);
    }

    #[test]
    fn escape_cancels_a_drag_without_leaving_history() {
        let (mut st, _id) = cube_state();
        let before = st.doc.state();
        let n = applied(&st);
        let start = handle_point(&st, 1);
        assert!(st.begin_gizmo_drag(Handle::Axis(1), start));
        assert!(st.update_gizmo_drag(start + Vec2::new(30.0, -20.0), false));
        assert_eq!(st.doc.state(), before, "the real document is untouched during the drag");
        assert_ne!(st.display_doc().state(), before, "the preview shows the move");
        st.perform(Action::Cancel);
        assert!(!st.is_dragging());
        assert_eq!(st.doc.state(), before);
        assert_eq!(applied(&st), n);
        assert_eq!(st.doc.history_list().len(), n, "nothing is left on the redo stack");
        assert!(!st.doc.can_redo());
    }

    #[test]
    fn dragging_back_to_the_start_commits_nothing() {
        let (mut st, _id) = cube_state();
        let n = st.doc.history_list().len();
        let start = handle_point(&st, 0);
        assert!(st.begin_gizmo_drag(Handle::Axis(0), start));
        assert!(st.update_gizmo_drag(start + Vec2::new(40.0, 0.0), false));
        assert!(st.update_gizmo_drag(start, false));
        assert!(!st.end_gizmo_drag());
        assert_eq!(st.doc.history_list().len(), n);
    }

    #[test]
    fn element_drag_uses_transform_elements_once() {
        let (mut st, id) = cube_state();
        st.perform(Action::SetMode(SelectMode::Face));
        st.run(CommandSpec::SelectElements {
            object: Some(id),
            kind: Some(ElementKind::Face),
            indices: vec![0],
            action: SelectAction::Set,
        });
        let before = st.doc.state();
        let n = applied(&st);
        let target = st.gizmo_target().unwrap();
        assert!(matches!(target.kind, crate::target::TargetKind::Elements { .. }));
        let start = handle_point(&st, 2);
        assert!(st.begin_gizmo_drag(Handle::Axis(2), start));
        for k in 1..=6 {
            assert!(st.update_gizmo_drag(start + Vec2::new(0.0, -(k as f32) * 6.0), false));
        }
        st.end_gizmo_drag();
        assert_eq!(applied(&st), n + 1);
        let mesh = st.doc.scene.mesh(id).unwrap();
        let bb_before = before.scene.get(id).unwrap().mesh.as_ref().unwrap();
        assert_ne!(mesh.positions(), bb_before.positions());
        assert_eq!(mesh.vertex_count(), 8, "topology unchanged");
        st.undo();
        assert_eq!(st.doc.state(), before);
    }

    #[test]
    fn external_delete_during_a_drag_ends_it_cleanly() {
        let (mut st, id) = cube_state();
        let start = handle_point(&st, 0);
        assert!(st.begin_gizmo_drag(Handle::Axis(0), start));
        assert!(st.update_gizmo_drag(start + Vec2::new(25.0, 0.0), false));
        // an AI deletes the object behind the GUI's back
        st.doc
            .execute_spec(
                CommandSpec::DeleteObjects {
                    objects: Some(vec![id]),
                    keep_children: false,
                },
                Origin::Ai,
            )
            .unwrap();
        st.sync_external();
        assert!(!st.is_dragging());
        assert!(!st.update_gizmo_drag(start + Vec2::new(50.0, 0.0), false));
        assert!(st.doc.scene.is_empty());
        assert!(st.gizmo_target().is_none());
        // the history is still usable
        st.undo();
        st.undo();
        assert!(st.doc.scene.contains(id) || st.doc.scene.is_empty());
    }

    #[test]
    fn foreign_command_between_updates_keeps_both_changes_consistent() {
        let (mut st, id) = cube_state();
        let start = handle_point(&st, 0);
        assert!(st.begin_gizmo_drag(Handle::Axis(0), start));
        assert!(st.update_gizmo_drag(start + Vec2::new(25.0, 0.0), false));
        st.doc
            .execute_spec(
                CommandSpec::RenameObject {
                    object: Some(id),
                    name: "Renamed".into(),
                },
                Origin::Ai,
            )
            .unwrap();
        // The next update notices the foreign change and cancels the drag
        // instead of committing a stale preview.
        assert!(!st.update_gizmo_drag(start + Vec2::new(60.0, 0.0), false));
        assert!(!st.is_dragging());
        assert!(!st.end_gizmo_drag());
        assert_eq!(st.doc.scene.get(id).unwrap().name, "Renamed");
        let moved = st.doc.scene.world_transform(id).unwrap().translation.x;
        assert!(moved.abs() < 1e-6, "the cancelled drag left no trace");
    }

    #[test]
    fn property_edits_coalesce_per_interaction() {
        let (mut st, id) = cube_state();
        let before = st.doc.state();
        let n = applied(&st);
        for k in 1..=10 {
            assert!(st.preview_transform(id, Some([k as f32 * 0.1, 0.0, 0.0]), None, None));
            st.end_frame(true); // mouse still down
        }
        assert_eq!(applied(&st), n + 1);
        st.end_frame(false); // released: interaction over
        assert!(st.preview_transform(id, Some([0.0, 2.0, 0.0]), None, None));
        st.end_frame(false);
        assert_eq!(applied(&st), n + 2, "a new interaction is a new entry");
        st.undo();
        st.undo();
        assert_eq!(st.doc.state(), before);
        // material edits too
        let mut m = Material::default();
        for k in 1..=5 {
            m.roughness = k as f32 * 0.1;
            assert!(st.preview_material(id, m.clone()));
        }
        assert_eq!(applied(&st), n + 1);
    }

    fn hit(node: Uuid, face: u32, vertex: u32, edge: u32) -> PickHit {
        PickHit {
            node_id: node,
            face,
            vertex,
            edge,
            position: [0.0; 3],
            distance: 1.0,
        }
    }

    #[test]
    fn picking_maps_to_selection_commands_per_mode() {
        let (mut st, a) = cube_state();
        st.run(create_primitive(PrimitiveSpec::Cube { size: 1.0 }));
        let b = st.doc.selection.active().unwrap();
        // object mode
        st.apply_pick(Some(hit(a, 1, 0, 0)), false);
        assert_eq!(st.doc.selection.objects(), &[a]);
        st.apply_pick(Some(hit(b, 1, 0, 0)), true);
        assert_eq!(st.doc.selection.objects(), &[a, b]);
        assert_eq!(st.doc.selection.active(), Some(b));
        st.apply_pick(Some(hit(a, 1, 0, 0)), true); // selected, not active -> make active
        assert_eq!(st.doc.selection.active(), Some(a));
        st.apply_pick(Some(hit(a, 1, 0, 0)), true); // active -> deselect
        assert_eq!(st.doc.selection.objects(), &[b]);
        st.apply_pick(None, true);
        assert_eq!(st.doc.selection.objects(), &[b], "shift on empty keeps selection");
        st.apply_pick(None, false);
        assert!(st.doc.selection.objects().is_empty());
        // element modes
        st.apply_pick(Some(hit(a, 2, 3, 4)), false);
        st.perform(Action::SetMode(SelectMode::Face));
        st.apply_pick(Some(hit(a, 2, 3, 4)), false);
        assert!(st.doc.selection.elements(a).unwrap().faces.contains(&2));
        st.apply_pick(Some(hit(a, 5, 3, 4)), true);
        assert_eq!(st.doc.selection.elements(a).unwrap().faces.len(), 2);
        st.apply_pick(Some(hit(a, 5, 3, 4)), true); // toggle off
        assert_eq!(st.doc.selection.elements(a).unwrap().faces.len(), 1);
        st.perform(Action::SetMode(SelectMode::Edge));
        st.apply_pick(Some(hit(a, 5, 3, 7)), false);
        assert!(st.doc.selection.elements(a).unwrap().edges.contains(&7));
        st.perform(Action::SetMode(SelectMode::Vertex));
        st.apply_pick(Some(hit(a, 5, 6, 7)), false);
        assert!(st.doc.selection.elements(a).unwrap().vertices.contains(&6));
        st.apply_pick(Some(hit(b, 0, 0, 0)), false); // other object becomes the edited one
        assert_eq!(st.doc.selection.active(), Some(b));
        st.apply_pick(None, false);
        assert!(st.doc.selection.elements(b).is_none());
    }

    #[test]
    fn toggle_edit_mode_remembers_the_element_mode() {
        let (mut st, _) = cube_state();
        st.perform(Action::SetMode(SelectMode::Edge));
        st.perform(Action::ToggleEditMode);
        assert_eq!(st.doc.selection.mode, SelectMode::Object);
        st.perform(Action::ToggleEditMode);
        assert_eq!(st.doc.selection.mode, SelectMode::Edge);
    }

    #[test]
    fn errors_become_structured_status_without_panicking() {
        let (mut st, _) = cube_state();
        st.perform(Action::MeshTool(MeshTool::LoopCut));
        assert_eq!(st.status.kind, StatusKind::Error);
        assert!(st.status.text.starts_with("invalid_action:"), "{}", st.status.text);
        assert!(st.status.text.contains('\u{2014}'));
        st.perform(Action::MeshTool(MeshTool::Extrude)); // object mode, no faces
        assert_eq!(st.status.kind, StatusKind::Error);
        st.perform(Action::Boolean(BooleanOp::Union));
        assert_eq!(st.status.kind, StatusKind::Error);
        st.open_project(Path::new("/definitely/not/here.f3d"));
        assert_eq!(st.status.kind, StatusKind::Error);
        st.import_file(Path::new("/definitely/not/here.obj"));
        assert_eq!(st.status.kind, StatusKind::Error);
        st.undo();
        st.undo();
        st.undo();
        st.undo();
        assert_eq!(st.status.kind, StatusKind::Error, "nothing to undo is reported");
    }

    #[test]
    fn history_goto_moves_the_cursor() {
        let (mut st, _) = cube_state();
        st.run(create_primitive(PrimitiveSpec::Cube { size: 1.0 }));
        st.run(create_primitive(PrimitiveSpec::Cube { size: 1.0 }));
        let list = st.doc.history_list();
        assert_eq!(st.doc.scene.len(), 3);
        st.history_goto(Some(list[0].seq));
        assert_eq!(st.doc.scene.len(), 1);
        st.history_goto(Some(list[2].seq));
        assert_eq!(st.doc.scene.len(), 3);
        st.history_goto(None);
        assert_eq!(st.doc.scene.len(), 0);
    }

    #[test]
    fn file_roundtrip_and_dirty_flag() {
        let dir = std::env::temp_dir().join(format!("forge3d-app-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut st, _) = cube_state();
        assert!(st.is_dirty());
        let f3d = dir.join("a.f3d");
        st.save_project(&f3d);
        assert!(!st.is_dirty(), "{}", st.status.text);
        st.run(create_primitive(PrimitiveSpec::Cube { size: 1.0 }));
        assert!(st.is_dirty());
        let obj = dir.join("a.obj");
        st.export_file(&obj);
        assert!(obj.exists(), "{}", st.status.text);
        st.import_file(&obj);
        assert_eq!(st.status.kind, StatusKind::Info, "{}", st.status.text);
        assert!(st.doc.scene.len() > 2);
        st.open_project(&f3d);
        assert_eq!(st.doc.scene.len(), 1);
        assert!(!st.is_dirty());
        st.new_document();
        assert!(st.doc.scene.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

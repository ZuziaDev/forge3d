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

/// Key of the preview used by gizmo drags.
const GIZMO_KEY: &str = "gizmo";

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

struct ActiveDrag {
    drag: GizmoDrag,
    target: GizmoTarget,
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

    /// Undo the preview (if it is still the newest entry) and forget it.
    fn discard_preview(&mut self) {
        if self.preview_is_top() {
            if let Err(e) = self.doc.undo() {
                self.report_core(&e);
            }
        }
        self.preview = None;
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
            let ok = d.target.is_valid(&self.doc)
                && (self.preview.is_none() || self.preview_is_top());
            if !ok {
                self.drag = None;
                self.preview = None;
                self.status = Status::info("Drag ended: the scene was changed externally");
            }
        }
        if let Some(p) = &self.preview {
            if p.key != GIZMO_KEY && !self.preview_is_top() {
                self.preview = None;
            }
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
                self.run(CommandSpec::SelectNone { object: active });
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

    /// Current gizmo target (None when nothing transformable is selected).
    pub fn gizmo_target(&self) -> Option<GizmoTarget> {
        GizmoTarget::compute(&self.doc)
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
        let Some(target) = self.gizmo_target() else {
            return false;
        };
        self.end_interactions();
        let frame = target.frame(self.tool);
        let cam = self.camera.to_camera();
        let drag = GizmoDrag::begin(self.tool, handle, frame, &cam, self.viewport, cursor);
        self.drag = Some(ActiveDrag { drag, target });
        true
    }

    /// Update the running drag; applies the live preview command. Returns
    /// false when the drag ended (target vanished, command failed).
    pub fn update_gizmo_drag(&mut self, cursor: Vec2, snap: bool) -> bool {
        let Some(mut active) = self.drag.take() else {
            return false;
        };
        if !active.target.is_valid(&self.doc)
            || (self.preview.is_some() && !self.preview_is_top())
        {
            self.preview = None;
            self.status = Status::info("Drag ended: the scene was changed externally");
            return false;
        }
        let cam = self.camera.to_camera();
        let snap_step = cam.grid_cell_size() * 0.25;
        let delta: DragDelta = active
            .drag
            .update(&cam, self.viewport, cursor, snap, snap_step);
        let ok = if delta.is_identity() {
            // Back at the start: remove the preview so nothing is committed.
            self.discard_preview();
            true
        } else {
            self.preview(GIZMO_KEY, active.target.spec(delta))
        };
        if ok {
            self.drag = Some(active);
        } else {
            self.preview = None;
        }
        ok
    }

    /// Finish the drag, keeping its single history entry. Returns whether
    /// the drag changed anything.
    pub fn end_gizmo_drag(&mut self) -> bool {
        let changed = self.preview.is_some();
        self.drag = None;
        self.preview = None;
        changed
    }

    /// Abort the drag and restore the original state (Escape).
    pub fn cancel_drag(&mut self) {
        if self.drag.take().is_some() {
            self.discard_preview();
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

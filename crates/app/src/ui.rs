//! The egui user interface. It only reads the [`AppState`] and turns user
//! input into [`Action`]s / commands on it; it never touches the scene.

use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Modifiers, PointerButton, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureId, Ui,
};
use forge3d_core::math::{euler_deg_from_quat, Axis};
use forge3d_core::prelude::*;
use forge3d_render::ViewPreset;
use glam::{Quat, Vec2};
use uuid::Uuid;

use crate::actions::{MeshTool, PrimitiveDraft, PrimitiveKind};
use crate::camera_input::{self, Button, Mods};
use crate::gizmo::{self, DragDelta, GizmoMode, Handle, Shape};
use crate::state::{Action, AppState, FileDialog, FileDialogKind, PickRequest, StatusKind};

const ACCENT: Color32 = Color32::from_rgb(255, 153, 51);
const AXIS_COLORS: [Color32; 3] = [
    Color32::from_rgb(232, 70, 70),
    Color32::from_rgb(110, 200, 60),
    Color32::from_rgb(70, 130, 240),
];
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 230, 90);

/// What the UI pass tells the renderer.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiOutput {
    /// Viewport rectangle in points (None when the viewport is hidden).
    pub viewport: Option<Rect>,
}

/// Install the application look.
pub fn setup_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::from_rgb(34, 36, 40);
        v.window_fill = Color32::from_rgb(40, 42, 47);
        v.extreme_bg_color = Color32::from_rgb(22, 23, 26);
        v.faint_bg_color = Color32::from_rgb(40, 42, 46);
        v.selection.bg_fill = Color32::from_rgb(150, 82, 24);
        v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 214, 170));
        v.widgets.hovered.bg_fill = Color32::from_rgb(70, 74, 82);
        v.widgets.inactive.bg_fill = Color32::from_rgb(52, 55, 61);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(52, 55, 61);
        v.hyperlink_color = ACCENT;
        style.spacing.item_spacing = vec2(6.0, 5.0);
        style.spacing.button_padding = vec2(7.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        for (_, font) in style.text_styles.iter_mut() {
            font.size = (font.size * 1.04).round();
        }
    });
}

/// Draw the whole UI for one frame.
pub fn draw(ui: &mut Ui, st: &mut AppState, viewport_tex: TextureId) -> UiOutput {
    handle_shortcuts(ui, st);

    egui::Panel::top("menubar").show(ui, |ui| menu_bar(ui, st));
    egui::Panel::top("toolbar").show(ui, |ui| toolbar(ui, st));
    egui::Panel::top("tool_options").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if st.doc.selection.mode == SelectMode::Object {
                object_tools(ui, st);
            } else {
                element_tools(ui, st);
            }
        });
    });
    egui::Panel::bottom("statusbar").show(ui, |ui| status_bar(ui, st));
    if st.ui.show_outliner || st.ui.show_history {
        egui::Panel::left("left")
            .default_size(250.0)
            .size_range(180.0..=420.0)
            .show(ui, |ui| {
                if st.ui.show_history {
                    egui::Panel::bottom("history")
                        .resizable(true)
                        .default_size(230.0)
                        .show(ui, |ui| history_panel(ui, st));
                }
                if st.ui.show_outliner {
                    egui::CentralPanel::default_margins().show(ui, |ui| outliner(ui, st));
                }
            });
    }
    if st.ui.show_properties {
        egui::Panel::right("properties")
            .default_size(280.0)
            .size_range(220.0..=460.0)
            .show(ui, |ui| properties(ui, st));
    }
    let mut out = UiOutput::default();
    egui::CentralPanel::no_frame().show(ui, |ui| {
        out.viewport = Some(viewport(ui, st, viewport_tex));
    });
    dialogs(ui.ctx(), st);

    let busy = ui.ctx().input(|i| i.pointer.any_down()) || ui.ctx().text_edit_focused();
    st.end_frame(busy);
    out
}

// ---------------------------------------------------------------------------
// shortcuts
// ---------------------------------------------------------------------------

fn handle_shortcuts(ui: &mut Ui, st: &mut AppState) {
    let ctx = ui.ctx().clone();
    if ctx.text_edit_focused() {
        return;
    }
    let mut actions: Vec<Action> = Vec::new();
    let mut save = false;
    let mut open = false;
    let mut new_doc = false;
    ctx.input_mut(|i| {
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if i.consume_key(ctrl_shift, Key::Z) || i.consume_key(ctrl, Key::Y) {
            actions.push(Action::Redo);
        }
        if i.consume_key(ctrl, Key::Z) {
            actions.push(Action::Undo);
        }
        if i.consume_key(ctrl, Key::S) {
            save = true;
        }
        if i.consume_key(ctrl, Key::O) {
            open = true;
        }
        if i.consume_key(ctrl, Key::N) {
            new_doc = true;
        }
        if i.consume_key(ctrl, Key::I) {
            actions.push(Action::SelectInvert);
        }
        if i.consume_key(Modifiers::ALT, Key::A) {
            actions.push(Action::SelectNone);
        }
        if i.consume_key(Modifiers::SHIFT, Key::D) {
            actions.push(Action::DuplicateSelected);
        }
        for (key, preset) in [
            (Key::Num1, ViewPreset::Front),
            (Key::Num2, ViewPreset::Right),
            (Key::Num3, ViewPreset::Top),
            (Key::Num4, ViewPreset::Iso),
        ] {
            if i.consume_key(Modifiers::ALT, key) {
                actions.push(Action::View(preset));
            }
        }
        let plain = |i: &mut egui::InputState, key: Key| i.consume_key(Modifiers::NONE, key);
        for (key, action) in [
            (Key::Num1, Action::SetMode(SelectMode::Object)),
            (Key::Num2, Action::SetMode(SelectMode::Vertex)),
            (Key::Num3, Action::SetMode(SelectMode::Edge)),
            (Key::Num4, Action::SetMode(SelectMode::Face)),
            (Key::Tab, Action::ToggleEditMode),
            (Key::G, Action::SetTool(GizmoMode::Translate)),
            (Key::R, Action::SetTool(GizmoMode::Rotate)),
            (Key::S, Action::SetTool(GizmoMode::Scale)),
            (Key::W, Action::SetTool(GizmoMode::Translate)),
            (Key::E, Action::SetTool(GizmoMode::Rotate)),
            (Key::A, Action::SelectAll),
            (Key::F, Action::FrameSelected),
            (Key::Home, Action::FrameAll),
            (Key::O, Action::ToggleOrtho),
            (Key::X, Action::DeleteSelected),
            (Key::Delete, Action::DeleteSelected),
            (Key::Escape, Action::Cancel),
        ] {
            if plain(i, key) {
                actions.push(action);
            }
        }
    });
    if new_doc {
        st.new_document();
    }
    if open {
        st.ui.file_dialog = Some(FileDialog {
            kind: FileDialogKind::Open,
            path: st.path.as_ref().map_or_else(String::new, |p| p.display().to_string()),
        });
    }
    if save {
        st.save();
    }
    for a in actions {
        st.perform(a);
    }
}

// ---------------------------------------------------------------------------
// menus and toolbar
// ---------------------------------------------------------------------------

fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    let b = egui::Button::new(label).shortcut_text(shortcut);
    let r = ui.add(b).clicked();
    if r {
        ui.close();
    }
    r
}

fn menu_bar(ui: &mut Ui, st: &mut AppState) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if menu_item(ui, "New", "Ctrl+N") {
                st.new_document();
            }
            if menu_item(ui, "Open project...", "Ctrl+O") {
                st.ui.file_dialog = Some(FileDialog {
                    kind: FileDialogKind::Open,
                    path: st.path.as_ref().map_or_else(String::new, |p| p.display().to_string()),
                });
            }
            if menu_item(ui, "Save", "Ctrl+S") {
                st.save();
            }
            if menu_item(ui, "Save as...", "") {
                st.ui.file_dialog = Some(FileDialog {
                    kind: FileDialogKind::SaveAs,
                    path: st
                        .path
                        .as_ref()
                        .map_or_else(|| "scene.f3d".into(), |p| p.display().to_string()),
                });
            }
            ui.separator();
            if menu_item(ui, "Import...", "") {
                st.ui.file_dialog = Some(FileDialog {
                    kind: FileDialogKind::Import,
                    path: String::new(),
                });
            }
            if menu_item(ui, "Export...", "") {
                st.ui.file_dialog = Some(FileDialog {
                    kind: FileDialogKind::Export,
                    path: "export.glb".into(),
                });
            }
            ui.separator();
            if menu_item(ui, "Quit", "") {
                st.should_exit = true;
            }
        });
        ui.menu_button("Edit", |ui| {
            let (can_undo, can_redo) = (st.doc.can_undo(), st.doc.can_redo());
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo").shortcut_text("Ctrl+Z"))
                .clicked()
            {
                st.perform(Action::Undo);
                ui.close();
            }
            if ui
                .add_enabled(
                    can_redo,
                    egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z"),
                )
                .clicked()
            {
                st.perform(Action::Redo);
                ui.close();
            }
            ui.separator();
            if menu_item(ui, "Duplicate", "Shift+D") {
                st.perform(Action::DuplicateSelected);
            }
            if menu_item(ui, "Delete", "X") {
                st.perform(Action::DeleteSelected);
            }
        });
        ui.menu_button("Add", |ui| {
            for kind in PrimitiveKind::ALL {
                ui.horizontal(|ui| {
                    if ui.button(kind.label()).clicked() {
                        st.perform(Action::AddPrimitive(PrimitiveDraft::of(kind).primitive()));
                        ui.close();
                    }
                });
            }
            ui.separator();
            ui.menu_button("With parameters", |ui| {
                for kind in PrimitiveKind::ALL {
                    if ui.button(format!("{}...", kind.label())).clicked() {
                        st.ui.primitive_dialog = Some(PrimitiveDraft::of(kind));
                        ui.close();
                    }
                }
            });
        });
        ui.menu_button("Select", |ui| {
            if menu_item(ui, "All", "A") {
                st.perform(Action::SelectAll);
            }
            if menu_item(ui, "None", "Alt+A") {
                st.perform(Action::SelectNone);
            }
            if menu_item(ui, "Invert", "Ctrl+I") {
                st.perform(Action::SelectInvert);
            }
            ui.separator();
            for (label, mode, key) in [
                ("Object mode", SelectMode::Object, "1"),
                ("Vertex mode", SelectMode::Vertex, "2"),
                ("Edge mode", SelectMode::Edge, "3"),
                ("Face mode", SelectMode::Face, "4"),
            ] {
                if menu_item(ui, label, key) {
                    st.perform(Action::SetMode(mode));
                }
            }
            if menu_item(ui, "Toggle edit mode", "Tab") {
                st.perform(Action::ToggleEditMode);
            }
        });
        ui.menu_button("Mesh", |ui| mesh_menu(ui, st));
        ui.menu_button("View", |ui| {
            if menu_item(ui, "Frame selected", "F") {
                st.perform(Action::FrameSelected);
            }
            if menu_item(ui, "Frame all", "Home") {
                st.perform(Action::FrameAll);
            }
            ui.separator();
            for (label, preset, key) in [
                ("Front", ViewPreset::Front, "Alt+1"),
                ("Back", ViewPreset::Back, ""),
                ("Right", ViewPreset::Right, "Alt+2"),
                ("Left", ViewPreset::Left, ""),
                ("Top", ViewPreset::Top, "Alt+3"),
                ("Bottom", ViewPreset::Bottom, ""),
                ("Isometric", ViewPreset::Iso, "Alt+4"),
            ] {
                if menu_item(ui, label, key) {
                    st.perform(Action::View(preset));
                }
            }
            ui.separator();
            let mut ortho = st.camera.orthographic;
            if ui.checkbox(&mut ortho, "Orthographic (O)").clicked() {
                st.perform(Action::ToggleOrtho);
            }
            let mut wire = st.view.wireframe;
            if ui.checkbox(&mut wire, "Wireframe overlay").clicked() {
                st.perform(Action::ToggleWireframe);
            }
            let mut grid = st.view.grid;
            if ui.checkbox(&mut grid, "Grid").clicked() {
                st.perform(Action::ToggleGrid);
            }
            let mut smooth = st.view.smooth_shading;
            if ui.checkbox(&mut smooth, "Smooth shading").clicked() {
                st.perform(Action::ToggleSmooth);
            }
            ui.separator();
            ui.checkbox(&mut st.ui.show_outliner, "Outliner");
            ui.checkbox(&mut st.ui.show_properties, "Properties");
            ui.checkbox(&mut st.ui.show_history, "History");
        });
    });
}

fn mesh_menu(ui: &mut Ui, st: &mut AppState) {
    for tool in [
        MeshTool::Extrude,
        MeshTool::Inset,
        MeshTool::Bevel,
        MeshTool::LoopCut,
        MeshTool::Subdivide,
        MeshTool::MergeCenter,
        MeshTool::MergeByDistance,
        MeshTool::DeleteElements,
        MeshTool::Mirror(Axis::X),
        MeshTool::Mirror(Axis::Y),
        MeshTool::Mirror(Axis::Z),
        MeshTool::RecalculateNormals,
        MeshTool::FlipNormals,
    ] {
        if ui.button(tool.label()).clicked() {
            st.perform(Action::MeshTool(tool));
            ui.close();
        }
    }
    ui.separator();
    for (label, op) in [
        ("Boolean union", BooleanOp::Union),
        ("Boolean difference", BooleanOp::Difference),
        ("Boolean intersect", BooleanOp::Intersect),
    ] {
        if ui.button(label).clicked() {
            st.perform(Action::Boolean(op));
            ui.close();
        }
    }
}

fn toolbar(ui: &mut Ui, st: &mut AppState) {
    ui.horizontal_wrapped(|ui| {
        // history
        if ui
            .add_enabled(st.doc.can_undo(), egui::Button::new("Undo"))
            .on_hover_text("Ctrl+Z")
            .clicked()
        {
            st.perform(Action::Undo);
        }
        if ui
            .add_enabled(st.doc.can_redo(), egui::Button::new("Redo"))
            .on_hover_text("Ctrl+Shift+Z")
            .clicked()
        {
            st.perform(Action::Redo);
        }
        ui.separator();
        // modes
        let mode = st.doc.selection.mode;
        for (label, m, tip) in [
            ("Object", SelectMode::Object, "1"),
            ("Vertex", SelectMode::Vertex, "2"),
            ("Edge", SelectMode::Edge, "3"),
            ("Face", SelectMode::Face, "4"),
        ] {
            if ui
                .selectable_label(mode == m, label)
                .on_hover_text(format!("{label} mode ({tip}); Tab toggles edit mode"))
                .clicked()
            {
                st.perform(Action::SetMode(m));
            }
        }
        ui.separator();
        for (t, tip) in [
            (GizmoMode::Translate, "G / W"),
            (GizmoMode::Rotate, "R / E"),
            (GizmoMode::Scale, "S"),
        ] {
            if ui
                .selectable_label(st.tool == t, t.label())
                .on_hover_text(tip)
                .clicked()
            {
                st.perform(Action::SetTool(t));
            }
        }
        ui.separator();
        view_toggles(ui, st);
    });
}

fn view_toggles(ui: &mut Ui, st: &mut AppState) {
    for (label, on, action, tip) in [
        ("Wire", st.view.wireframe, Action::ToggleWireframe, "Wireframe overlay"),
        ("Grid", st.view.grid, Action::ToggleGrid, "Ground grid"),
        ("Smooth", st.view.smooth_shading, Action::ToggleSmooth, "Smooth shading"),
        ("Ortho", st.camera.orthographic, Action::ToggleOrtho, "Orthographic camera (O)"),
    ] {
        if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
            st.perform(action);
        }
    }
}

fn param(ui: &mut Ui, value: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>) {
    ui.add(
        egui::DragValue::new(value)
            .speed(speed)
            .range(range)
            .max_decimals(3),
    );
}

fn object_tools(ui: &mut Ui, st: &mut AppState) {
    ui.menu_button("+ Add", |ui| {
        for kind in PrimitiveKind::ALL {
            ui.horizontal(|ui| {
                if ui.button(kind.label()).clicked() {
                    st.perform(Action::AddPrimitive(PrimitiveDraft::of(kind).primitive()));
                    ui.close();
                }
                if ui.small_button("...").on_hover_text("With parameters").clicked() {
                    st.ui.primitive_dialog = Some(PrimitiveDraft::of(kind));
                    ui.close();
                }
            });
        }
    });
    if ui.button("Duplicate").on_hover_text("Shift+D").clicked() {
        st.perform(Action::DuplicateSelected);
    }
    if ui.button("Delete").on_hover_text("X").clicked() {
        st.perform(Action::DeleteSelected);
    }
    ui.menu_button("Boolean", |ui| {
        ui.label("Active object op selected object");
        for (label, op) in [
            ("Union", BooleanOp::Union),
            ("Difference", BooleanOp::Difference),
            ("Intersect", BooleanOp::Intersect),
        ] {
            if ui.button(label).clicked() {
                st.perform(Action::Boolean(op));
                ui.close();
            }
        }
    });
    ui.menu_button("Mesh", |ui| mesh_menu(ui, st));
}

fn element_tools(ui: &mut Ui, st: &mut AppState) {
    let p = &mut st.params;
    let mut fire: Option<MeshTool> = None;
    let mut tool = |ui: &mut Ui, tool: MeshTool, add: &mut dyn FnMut(&mut Ui)| {
        if ui.button(tool.label()).clicked() {
            fire = Some(tool);
        }
        add(ui);
        ui.add_space(6.0);
    };
    tool(ui, MeshTool::Extrude, &mut |ui| {
        param(ui, &mut p.extrude_distance, 0.02, -100.0..=100.0)
    });
    tool(ui, MeshTool::Inset, &mut |ui| {
        param(ui, &mut p.inset_thickness, 0.01, 0.0..=100.0)
    });
    tool(ui, MeshTool::Bevel, &mut |ui| {
        param(ui, &mut p.bevel_width, 0.01, 0.0..=100.0)
    });
    tool(ui, MeshTool::LoopCut, &mut |ui| {
        let mut cuts = p.loop_cuts;
        if ui
            .add(egui::DragValue::new(&mut cuts).range(1..=64).speed(0.1))
            .changed()
        {
            p.loop_cuts = cuts;
        }
    });
    tool(ui, MeshTool::Subdivide, &mut |ui| {
        let mut l = p.subdivide_levels;
        if ui
            .add(egui::DragValue::new(&mut l).range(1..=4).speed(0.05))
            .changed()
        {
            p.subdivide_levels = l;
        }
    });
    {
        ui.menu_button("Merge", |ui| {
            if ui.button("At center").clicked() {
                fire = Some(MeshTool::MergeCenter);
                ui.close();
            }
            if ui.button("By distance").clicked() {
                fire = Some(MeshTool::MergeByDistance);
                ui.close();
            }
            ui.horizontal(|ui| {
                ui.label("Distance");
                param(ui, &mut p.merge_threshold, 0.0005, 0.0..=10.0);
            });
        });
    }
    {
        ui.menu_button("Mirror", |ui| {
            for a in [Axis::X, Axis::Y, Axis::Z] {
                if ui.button(MeshTool::Mirror(a).label()).clicked() {
                    fire = Some(MeshTool::Mirror(a));
                    ui.close();
                }
            }
            ui.checkbox(&mut p.mirror_merge, "Merge seam");
        });
    }
    {
        ui.menu_button("Normals", |ui| {
            if ui.button("Recalculate outside").clicked() {
                p.recalc_inside = false;
                fire = Some(MeshTool::RecalculateNormals);
                ui.close();
            }
            if ui.button("Recalculate inside").clicked() {
                p.recalc_inside = true;
                fire = Some(MeshTool::RecalculateNormals);
                ui.close();
            }
            if ui.button("Flip").clicked() {
                fire = Some(MeshTool::FlipNormals);
                ui.close();
            }
        });
    }
    if ui.button("Delete").on_hover_text("X").clicked() {
        fire = Some(MeshTool::DeleteElements);
    }
    if let Some(t) = fire {
        st.perform(Action::MeshTool(t));
    }
}

fn status_bar(ui: &mut Ui, st: &mut AppState) {
    ui.horizontal(|ui| {
        let color = match st.status.kind {
            StatusKind::Info => ui.visuals().text_color(),
            StatusKind::Error => Color32::from_rgb(255, 110, 100),
        };
        ui.add(egui::Label::new(RichText::new(&st.status.text).color(color)).truncate());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let doc = &st.doc;
            let sel = doc.selection.objects().len();
            let mode = match doc.selection.mode {
                SelectMode::Object => "Object",
                SelectMode::Vertex => "Vertex",
                SelectMode::Edge => "Edge",
                SelectMode::Face => "Face",
            };
            let file = st
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or_else(|| "untitled".to_string(), |n| n.to_string_lossy().into_owned());
            ui.weak(format!(
                "{}{}  |  {} objects, {} selected  |  {} mode",
                file,
                if st.is_dirty() { "*" } else { "" },
                doc.scene.len(),
                sel,
                mode
            ));
        });
    });
}

// ---------------------------------------------------------------------------
// outliner
// ---------------------------------------------------------------------------

enum OutlinerAction {
    Select(Uuid, bool),
    Visible(Uuid, bool),
    Rename(Uuid, String),
    Delete(Uuid),
    Duplicate(Uuid),
    Reparent(Uuid, Option<Uuid>),
}

fn outliner(ui: &mut Ui, st: &mut AppState) {
    ui.heading("Outliner");
    ui.separator();
    let mut acts: Vec<OutlinerAction> = Vec::new();
    let roots: Vec<Uuid> = st.doc.scene.roots().to_vec();
    let dragging = egui::DragAndDrop::has_payload_of_type::<Uuid>(ui.ctx());
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if roots.is_empty() {
                ui.weak("The scene is empty. Use Add to create an object.");
            }
            for id in roots {
                outliner_node(ui, st, id, &mut acts);
            }
            if dragging {
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                let hot = resp.dnd_hover_payload::<Uuid>().is_some();
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    Stroke::new(1.0, if hot { ACCENT } else { Color32::GRAY }),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Drop here to unparent",
                    FontId::proportional(12.0),
                    Color32::GRAY,
                );
                if let Some(p) = resp.dnd_release_payload::<Uuid>() {
                    acts.push(OutlinerAction::Reparent(*p, None));
                }
            }
        });
    for a in acts {
        match a {
            OutlinerAction::Select(id, shift) => st.select_object(id, shift),
            OutlinerAction::Visible(id, v) => st.set_visible(id, v),
            OutlinerAction::Rename(id, n) => st.rename(id, n),
            OutlinerAction::Delete(id) => st.delete_objects(vec![id]),
            OutlinerAction::Duplicate(id) => {
                st.select_object(id, false);
                st.perform(Action::DuplicateSelected);
            }
            OutlinerAction::Reparent(id, p) => st.reparent(id, p),
        }
    }
}

fn paint_eye(painter: &egui::Painter, rect: Rect, visible: bool, color: Color32) {
    let c = rect.center();
    let (w, h) = (rect.width() * 0.42, rect.height() * 0.22);
    let mut pts = Vec::new();
    for k in 0..=12 {
        let t = k as f32 / 12.0 * std::f32::consts::PI;
        pts.push(pos2(c.x - w * t.cos(), c.y - h * 1.6 * t.sin()));
    }
    for k in 0..=12 {
        let t = k as f32 / 12.0 * std::f32::consts::PI;
        pts.push(pos2(c.x + w * t.cos(), c.y + h * 1.6 * t.sin()));
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.3, color)));
    if visible {
        painter.circle_filled(c, h * 1.1, color);
    } else {
        painter.line_segment(
            [pos2(c.x - w, c.y + h * 2.0), pos2(c.x + w, c.y - h * 2.0)],
            Stroke::new(1.5, color),
        );
    }
}

fn outliner_node(ui: &mut Ui, st: &mut AppState, id: Uuid, acts: &mut Vec<OutlinerAction>) {
    let Some(node) = st.doc.scene.get(id) else {
        return;
    };
    let children: Vec<Uuid> = node.children().to_vec();
    let name = node.name.clone();
    let visible = node.visible;
    let has_mesh = node.mesh.is_some();
    let selected = st.doc.selection.is_selected(id);
    let active = st.doc.selection.active() == Some(id);

    let state_id = ui.make_persistent_id(("outliner", id));
    let coll =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, true);

    let row = |ui: &mut Ui, acts: &mut Vec<OutlinerAction>, st: &mut AppState| {
        // visibility
        let (eye_rect, eye) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        let eye_color = if visible {
            Color32::from_gray(200)
        } else {
            Color32::from_gray(100)
        };
        paint_eye(ui.painter(), eye_rect, visible, if eye.hovered() { ACCENT } else { eye_color });
        if eye.on_hover_text("Toggle visibility").clicked() {
            acts.push(OutlinerAction::Visible(id, !visible));
        }
        let renaming = st.ui.renaming.as_ref().filter(|(r, _)| *r == id).is_some();
        if renaming {
            let mut commit = None;
            if let Some((_, buf)) = st.ui.renaming.as_mut() {
                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(ui.available_width()));
                r.request_focus();
                if r.lost_focus() {
                    commit = Some(!ui.input(|i| i.key_pressed(Key::Escape)));
                }
            }
            if let Some(ok) = commit {
                if let Some((rid, buf)) = st.ui.renaming.take() {
                    if ok && buf != name && !buf.trim().is_empty() {
                        acts.push(OutlinerAction::Rename(rid, buf));
                    }
                }
            }
            return;
        }
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width().max(40.0), 22.0),
            Sense::click_and_drag(),
        );
        let hot_drop = resp.dnd_hover_payload::<Uuid>().is_some_and(|p| *p != id);
        if selected || resp.hovered() || hot_drop {
            let fill = if hot_drop {
                Color32::from_rgb(70, 100, 150)
            } else if active {
                Color32::from_rgb(168, 92, 26)
            } else if selected {
                Color32::from_rgb(112, 64, 26)
            } else {
                Color32::from_rgb(52, 55, 61)
            };
            ui.painter().rect_filled(rect, 3.0, fill);
        }
        let icon_color = if active { Color32::WHITE } else { Color32::from_gray(150) };
        let ic = pos2(rect.left() + 9.0, rect.center().y);
        if has_mesh {
            ui.painter()
                .rect_filled(Rect::from_center_size(ic, vec2(8.0, 8.0)), 1.5, icon_color);
        } else {
            ui.painter().circle_stroke(ic, 4.0, Stroke::new(1.3, icon_color));
        }
        let text_color = if visible {
            Color32::from_gray(228)
        } else {
            Color32::from_gray(120)
        };
        ui.painter().text(
            pos2(rect.left() + 20.0, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            FontId::proportional(13.5),
            text_color,
        );
        if resp.clicked() {
            let shift = ui.input(|i| i.modifiers.shift);
            acts.push(OutlinerAction::Select(id, shift));
        }
        if resp.double_clicked() {
            st.ui.renaming = Some((id, name.clone()));
        }
        if resp.drag_started() {
            resp.dnd_set_drag_payload(id);
        }
        if resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if let Some(p) = resp.dnd_release_payload::<Uuid>() {
            if *p != id {
                acts.push(OutlinerAction::Reparent(*p, Some(id)));
            }
        }
        resp.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                st.ui.renaming = Some((id, name.clone()));
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                acts.push(OutlinerAction::Duplicate(id));
                ui.close();
            }
            if ui.button("Delete").clicked() {
                acts.push(OutlinerAction::Delete(id));
                ui.close();
            }
            ui.separator();
            let active_other = st.doc.selection.active().filter(|a| *a != id);
            if ui
                .add_enabled(active_other.is_some(), egui::Button::new("Parent to active object"))
                .clicked()
            {
                if let Some(a) = active_other {
                    acts.push(OutlinerAction::Reparent(id, Some(a)));
                }
                ui.close();
            }
            if ui.button("Clear parent").clicked() {
                acts.push(OutlinerAction::Reparent(id, None));
                ui.close();
            }
        });
    };

    if children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent.min(16.0));
            row(ui, acts, st);
        });
    } else {
        coll.show_header(ui, |ui| row(ui, acts, st))
            .body(|ui| {
                for c in children {
                    outliner_node(ui, st, c, acts);
                }
            });
    }
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

fn history_panel(ui: &mut Ui, st: &mut AppState) {
    ui.heading("History");
    ui.separator();
    let entries = st.doc.history_list();
    let mut goto: Option<Option<u64>> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let applied = entries.iter().filter(|e| !e.undone).count();
            let r = ui.selectable_label(applied == 0, RichText::new("Original").weak());
            if r.clicked() {
                goto = Some(None);
            }
            for (i, e) in entries.iter().enumerate() {
                let current = !e.undone && entries.get(i + 1).is_none_or(|n| n.undone);
                let mut text = RichText::new(format!("{}  {}", i + 1, e.label));
                if e.undone {
                    text = text.color(Color32::from_gray(105)).strikethrough();
                } else if e.label.starts_with("AI:") {
                    text = text.color(Color32::from_rgb(190, 150, 255));
                } else if e.label.starts_with("CLI:") {
                    text = text.color(Color32::from_rgb(120, 205, 220));
                }
                let r = ui.selectable_label(current, text).on_hover_text(&e.description);
                if r.clicked() {
                    goto = Some(Some(e.seq));
                }
            }
        });
    if let Some(g) = goto {
        st.history_goto(g);
    }
}

// ---------------------------------------------------------------------------
// properties
// ---------------------------------------------------------------------------

/// sRGB (0..1) to linear.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear to sRGB (0..1).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn drag3(
    ui: &mut Ui,
    label: &str,
    v: &mut [f32; 3],
    speed: f64,
    suffix: &str,
    colors: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).weak());
        ui.spacing_mut().item_spacing.x = 3.0;
        let w = ((ui.available_width() - 12.0) / 3.0).max(40.0);
        for (i, c) in v.iter_mut().enumerate() {
            if colors {
                let (r, _) = ui.allocate_exact_size(vec2(3.0, 18.0), Sense::hover());
                ui.painter().rect_filled(r, 1.0, AXIS_COLORS[i]);
            }
            let r = ui.add_sized(
                [w - if colors { 6.0 } else { 0.0 }, 20.0],
                egui::DragValue::new(c)
                    .speed(speed)
                    .max_decimals(3)
                    .suffix(suffix),
            );
            changed |= r.changed();
        }
    });
    changed
}

fn properties(ui: &mut Ui, st: &mut AppState) {
    ui.heading("Properties");
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(id) = st.doc.selection.active() else {
                ui.weak("No active object.");
                ui.weak("Click an object in the viewport or outliner.");
                return;
            };
            let Some(node) = st.display_doc().scene.get(id) else {
                return;
            };
            let name = node.name.clone();
            let (scale, rot, trans) = node.transform.to_scale_rotation_translation();
            let material = node.material.clone();
            let multi = st.doc.selection.objects().len();
            if multi > 1 {
                ui.weak(format!("{multi} objects selected; showing the active one"));
            }

            // name
            let mut buf = name.clone();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Name").weak());
                let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY));
                if r.lost_focus() && buf != name && !buf.trim().is_empty() {
                    st.rename(id, buf.clone());
                }
            });
            ui.add_space(4.0);

            // transform
            egui::CollapsingHeader::new("Transform")
                .default_open(true)
                .show(ui, |ui| {
                    let mut t = trans.to_array();
                    if drag3(ui, "Location", &mut t, 0.02, "", true) {
                        st.preview_transform(id, Some(t), None, None);
                    }
                    let mut e = match st.ui.euler_cache {
                        Some((cid, q, e)) if cid == id && q.abs_diff_eq(rot, 1e-5) => e,
                        _ => euler_deg_from_quat(rot),
                    };
                    // avoid displaying "-0.0"
                    for c in &mut e {
                        *c += 0.0;
                    }
                    if drag3(ui, "Rotation", &mut e, 0.5, "\u{b0}", true) {
                        let q = forge3d_core::math::quat_from_euler_deg(e);
                        if st.preview_transform(id, None, Some(e), None) {
                            st.ui.euler_cache = Some((id, q, e));
                        }
                    }
                    let mut s = scale.to_array();
                    if drag3(ui, "Scale", &mut s, 0.01, "", true) {
                        for c in &mut s {
                            if c.abs() < 1e-4 {
                                *c = 1e-4;
                            }
                        }
                        st.preview_transform(id, None, None, Some(s));
                    }
                });

            // mesh
            if let Some(report) = st.validation_for(id) {
                egui::CollapsingHeader::new("Mesh")
                    .default_open(true)
                    .show(ui, |ui| mesh_stats(ui, st, id, &report));
            } else {
                ui.weak("Group node (no mesh)");
            }

            // material
            egui::CollapsingHeader::new("Material")
                .default_open(true)
                .show(ui, |ui| material_editor(ui, st, id, material));
        });
}

fn mesh_stats(ui: &mut Ui, st: &AppState, id: Uuid, r: &ValidationReport) {
    egui::Grid::new("mesh_stats").num_columns(2).show(ui, |ui| {
        ui.label("Vertices");
        ui.label(r.vertex_count.to_string());
        ui.end_row();
        ui.label("Edges");
        ui.label(r.edge_count.to_string());
        ui.end_row();
        ui.label("Faces");
        ui.label(r.face_count.to_string());
        ui.end_row();
        if let Some(sel) = st.doc.selection.elements(id) {
            ui.label("Selected");
            ui.label(format!(
                "{} v / {} e / {} f",
                sel.vertices.len(),
                sel.edges.len(),
                sel.faces.len()
            ));
            ui.end_row();
        }
    });
    let ok = Color32::from_rgb(110, 200, 110);
    let bad = Color32::from_rgb(240, 170, 70);
    let issue = r.non_manifold_edges
        + r.non_manifold_vertices
        + r.inconsistent_winding_edges
        + r.degenerate_faces
        + r.dangling_references;
    if issue == 0 {
        ui.label(RichText::new("Valid").color(ok));
    } else {
        ui.label(RichText::new(format!("{issue} issue(s)")).color(bad));
    }
    ui.horizontal_wrapped(|ui| {
        let flag = |ui: &mut Ui, on: bool, label: &str| {
            ui.label(RichText::new(label).color(if on { ok } else { bad }).small());
        };
        flag(ui, r.is_manifold, "manifold");
        flag(ui, r.is_closed, "closed");
        flag(ui, r.is_consistently_oriented, "oriented");
        if r.boundary_edges > 0 {
            ui.label(RichText::new(format!("{} boundary edges", r.boundary_edges)).small().weak());
        }
        if r.degenerate_faces > 0 {
            ui.label(RichText::new(format!("{} degenerate faces", r.degenerate_faces)).small().weak());
        }
    });
}

fn material_editor(ui: &mut Ui, st: &mut AppState, id: Uuid, material: Option<Material>) {
    let Some(mut m) = material else {
        ui.weak("No material (default gray)");
        if ui.button("Add material").clicked() {
            st.preview_material(id, Material::default());
            st.commit_preview();
        }
        return;
    };
    let before = m.clone();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Base color").weak());
        let mut c = [
            linear_to_srgb(m.base_color[0]),
            linear_to_srgb(m.base_color[1]),
            linear_to_srgb(m.base_color[2]),
            m.base_color[3],
        ];
        if ui.color_edit_button_rgba_unmultiplied(&mut c).changed() {
            m.base_color = [
                srgb_to_linear(c[0]).clamp(0.0, 1.0),
                srgb_to_linear(c[1]).clamp(0.0, 1.0),
                srgb_to_linear(c[2]).clamp(0.0, 1.0),
                c[3].clamp(0.0, 1.0),
            ];
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Metallic").weak());
        ui.add(egui::Slider::new(&mut m.metallic, 0.0..=1.0).fixed_decimals(2));
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Roughness").weak());
        ui.add(egui::Slider::new(&mut m.roughness, 0.0..=1.0).fixed_decimals(2));
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Emissive").weak());
        let mut c = [
            linear_to_srgb(m.emissive[0].min(1.0)),
            linear_to_srgb(m.emissive[1].min(1.0)),
            linear_to_srgb(m.emissive[2].min(1.0)),
        ];
        if ui.color_edit_button_rgb(&mut c).changed() {
            m.emissive = [
                srgb_to_linear(c[0]),
                srgb_to_linear(c[1]),
                srgb_to_linear(c[2]),
            ];
        }
        if ui.small_button("Clear").clicked() {
            m.emissive = [0.0; 3];
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").weak());
        ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(f32::INFINITY));
    });
    if m != before {
        st.preview_material(id, m);
    }
    if ui.small_button("Remove material").clicked() {
        st.run(CommandSpec::SetMaterial {
            objects: Some(vec![id]),
            material: None,
        });
    }
}

// ---------------------------------------------------------------------------
// dialogs
// ---------------------------------------------------------------------------

fn dialogs(ctx: &egui::Context, st: &mut AppState) {
    if let Some(mut dlg) = st.ui.file_dialog.clone() {
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(dlg.kind.title())
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, -60.0))
            .show(ctx, |ui| {
                ui.label("Path (.f3d project, or .gltf .glb .obj .stl):");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut dlg.path)
                        .desired_width(f32::INFINITY)
                        .hint_text("/path/to/file"),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirm = true;
                }
                if let Some(p) = directory_browser(ui, &dlg.path) {
                    dlg.path = p;
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        st.ui.file_dialog = Some(dlg);
        if confirm {
            st.confirm_file_dialog();
        } else if cancel || !open {
            st.ui.file_dialog = None;
        }
    }
    if let Some(mut d) = st.ui.primitive_dialog.clone() {
        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        egui::Window::new(format!("Add {}", d.kind.label()))
            .open(&mut open)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, -60.0))
            .show(ctx, |ui| {
                egui::Grid::new("prim_params").num_columns(2).show(ui, |ui| {
                    primitive_fields(ui, &mut d);
                });
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if create {
            st.ui.primitive_dialog = None;
            st.run(d.spec());
        } else if cancel || !open {
            st.ui.primitive_dialog = None;
        } else {
            st.ui.primitive_dialog = Some(d);
        }
    }
}

fn primitive_fields(ui: &mut Ui, d: &mut PrimitiveDraft) {
    let f = |ui: &mut Ui, label: &str, v: &mut f32| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(0.02).range(0.001..=1000.0));
        ui.end_row();
    };
    let n = |ui: &mut Ui, label: &str, v: &mut u32, lo: u32, hi: u32| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(0.2).range(lo..=hi));
        ui.end_row();
    };
    match d.kind {
        PrimitiveKind::Cube => f(ui, "Size", &mut d.size),
        PrimitiveKind::Plane => {
            f(ui, "Size", &mut d.size);
            n(ui, "Subdivisions", &mut d.subdivisions, 1, 128);
        }
        PrimitiveKind::UvSphere => {
            f(ui, "Radius", &mut d.radius);
            n(ui, "Segments", &mut d.segments, 3, 256);
            n(ui, "Rings", &mut d.rings, 2, 256);
        }
        PrimitiveKind::IcoSphere => {
            f(ui, "Radius", &mut d.radius);
            n(ui, "Subdivisions", &mut d.ico_subdivisions, 0, 6);
        }
        PrimitiveKind::Cylinder => {
            f(ui, "Radius", &mut d.radius);
            f(ui, "Depth", &mut d.depth);
            n(ui, "Segments", &mut d.segments, 3, 256);
        }
        PrimitiveKind::Cone => {
            f(ui, "Bottom radius", &mut d.radius);
            ui.label("Top radius");
            ui.add(egui::DragValue::new(&mut d.radius_top).speed(0.02).range(0.0..=1000.0));
            ui.end_row();
            f(ui, "Depth", &mut d.depth);
            n(ui, "Segments", &mut d.segments, 3, 256);
        }
        PrimitiveKind::Torus => {
            f(ui, "Major radius", &mut d.major_radius);
            f(ui, "Minor radius", &mut d.minor_radius);
            n(ui, "Major segments", &mut d.major_segments, 3, 256);
            n(ui, "Minor segments", &mut d.minor_segments, 3, 128);
        }
    }
}

/// Entries of the folder of `typed` (or the current dir), clickable.
fn directory_browser(ui: &mut Ui, typed: &str) -> Option<String> {
    let typed_path = std::path::Path::new(typed.trim());
    let dir = if typed.trim().is_empty() {
        std::env::current_dir().ok()?
    } else if typed_path.is_dir() {
        typed_path.to_path_buf()
    } else {
        typed_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| std::env::current_dir().unwrap_or_default(), |p| p.to_path_buf())
    };
    let mut picked = None;
    ui.add_space(4.0);
    ui.label(RichText::new(dir.display().to_string()).weak().small());
    egui::ScrollArea::vertical()
        .max_height(180.0)
        .id_salt("dir_browser")
        .show(ui, |ui| {
            if let Some(parent) = dir.parent() {
                if ui.selectable_label(false, "\u{2191} ..").clicked() {
                    picked = Some(format!("{}{}", parent.display(), std::path::MAIN_SEPARATOR));
                }
            }
            let mut entries: Vec<(bool, String, std::path::PathBuf)> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .map(|e| {
                            let p = e.path();
                            (
                                p.is_dir(),
                                e.file_name().to_string_lossy().into_owned(),
                                p,
                            )
                        })
                        .filter(|(_, n, _)| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
            entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
            for (is_dir, name, path) in entries.into_iter().take(300) {
                let label = if is_dir { format!("\u{25B8} {name}") } else { name };
                if ui.selectable_label(false, label).clicked() {
                    let mut s = path.display().to_string();
                    if is_dir {
                        s.push(std::path::MAIN_SEPARATOR);
                    }
                    picked = Some(s);
                }
            }
        });
    picked
}

// ---------------------------------------------------------------------------
// viewport
// ---------------------------------------------------------------------------

fn to_mods(m: Modifiers) -> Mods {
    Mods {
        shift: m.shift,
        ctrl: m.command || m.ctrl,
        alt: m.alt,
    }
}

fn v2(p: egui::Pos2, origin: egui::Pos2) -> Vec2 {
    Vec2::new(p.x - origin.x, p.y - origin.y)
}

fn viewport(ui: &mut Ui, st: &mut AppState, tex: TextureId) -> Rect {
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail.max(vec2(8.0, 8.0)), Sense::click_and_drag());
    st.viewport = (
        rect.width().round().max(1.0) as u32,
        rect.height().round().max(1.0) as u32,
    );
    if st.pending_frame > 0 {
        st.pending_frame -= 1;
        st.frame(st.pending_frame_selected);
        ui.ctx().request_repaint();
    }
    let painter = ui.painter_at(rect);
    painter.image(
        tex,
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );

    if resp.hovered() && !ui.ctx().text_edit_focused() {
        resp.request_focus();
    }
    if resp.has_focus() {
        let filter = egui::EventFilter {
            tab: true,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: true,
        };
        ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, filter));
    }
    let mods = ui.input(|i| to_mods(i.modifiers));
    let origin = rect.min;
    let press = ui.input(|i| i.pointer.press_origin());
    let hover = resp.hover_pos().map(|p| v2(p, origin));
    let viewport_h = rect.height();

    // --- zoom
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            st.pending_frame = 0;
            camera_input::apply_scroll(&mut st.camera, scroll);
        }
    }

    // --- navigation
    for (b, pb) in [
        (Button::Middle, PointerButton::Middle),
        (Button::Primary, PointerButton::Primary),
    ] {
        if resp.dragged_by(pb) {
            if let Some(nav) = camera_input::nav_for_drag(b, mods) {
                st.pending_frame = 0;
                let d = resp.drag_delta();
                camera_input::apply_drag(&mut st.camera, nav, d.x, d.y, viewport_h);
            }
        }
    }

    // --- gizmo hover / drag
    let nav_gesture = mods.alt;
    if !st.is_dragging() {
        let h = if nav_gesture || !resp.hovered() {
            None
        } else {
            hover.and_then(|c| {
                let t = st.gizmo_target()?;
                gizmo::hit_test(
                    st.tool,
                    &t.frame(st.tool),
                    &st.camera.to_camera(),
                    st.viewport,
                    c,
                )
            })
        };
        st.set_hovered_handle(h);
    }
    if resp.drag_started_by(PointerButton::Primary) && !nav_gesture {
        if let Some(p) = press {
            let c = v2(p, origin);
            let handle = st.gizmo_target().and_then(|t| {
                gizmo::hit_test(
                    st.tool,
                    &t.frame(st.tool),
                    &st.camera.to_camera(),
                    st.viewport,
                    c,
                )
            });
            if let Some(handle) = handle {
                st.begin_gizmo_drag(handle, c);
            }
        }
    }
    if st.is_dragging() {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            if !st.update_gizmo_drag(v2(p, origin), mods.ctrl) {
                // the drag ended itself (target vanished); nothing else to do
            }
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            st.end_gizmo_drag();
        }
        ui.ctx().request_repaint();
    }
    if st.hovered_handle().is_some() || st.is_dragging() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }

    // --- picking
    if resp.clicked_by(PointerButton::Primary) && !nav_gesture && st.hovered_handle().is_none() {
        if let Some(p) = resp.interact_pointer_pos() {
            let ppp = ui.ctx().pixels_per_point();
            let c = v2(p, origin) * ppp;
            st.queue_pick(PickRequest {
                pos: c,
                shift: mods.shift,
            });
        }
    }

    // --- overlays
    draw_gizmo(&painter, rect, st);
    draw_hud(&painter, rect, st);
    draw_axis_widget(&painter, rect, st);
    rect
}

fn handle_color(handle: Handle) -> Color32 {
    match handle {
        Handle::Axis(i) => AXIS_COLORS[i],
        Handle::Uniform => Color32::from_gray(225),
    }
}

fn draw_gizmo(painter: &egui::Painter, rect: Rect, st: &AppState) {
    let Some(target) = st.gizmo_target() else {
        return;
    };
    let frame = target.frame(st.tool);
    let cam = st.camera.to_camera();
    let active = st.drag_handle();
    let hovered = st.hovered_handle();
    let o = rect.min;
    let pt = |p: Vec2| pos2(o.x + p.x, o.y + p.y);
    if let Some(c) = gizmo::project(&cam, frame.origin, st.viewport) {
        painter.circle_filled(pt(c), 3.5, Color32::WHITE);
        painter.circle_stroke(pt(c), 3.5, Stroke::new(1.0, Color32::BLACK));
    }
    for part in gizmo::parts(st.tool, &frame, &cam, st.viewport) {
        let lit = active == Some(part.handle) || (active.is_none() && hovered == Some(part.handle));
        let dim = active.is_some() && active != Some(part.handle);
        let mut color = if lit { HIGHLIGHT } else { handle_color(part.handle) };
        if dim {
            color = color.gamma_multiply(0.45);
        }
        let width = if lit { 3.4 } else { 2.4 };
        let shadow = Stroke::new(width + 2.0, Color32::from_black_alpha(110));
        let stroke = Stroke::new(width, color);
        for s in &part.shapes {
            match s {
                Shape::Line(a, b) => {
                    painter.line_segment([pt(*a), pt(*b)], shadow);
                    painter.line_segment([pt(*a), pt(*b)], stroke);
                }
                Shape::Polyline(v) => {
                    let pts: Vec<_> = v.iter().map(|p| pt(*p)).collect();
                    painter.add(egui::Shape::line(pts.clone(), shadow));
                    painter.add(egui::Shape::line(pts, stroke));
                }
                Shape::Triangle(t) => {
                    painter.add(egui::Shape::convex_polygon(
                        t.iter().map(|p| pt(*p)).collect(),
                        color,
                        Stroke::new(1.0, Color32::from_black_alpha(160)),
                    ));
                }
                Shape::Square { center, half } => {
                    let r = Rect::from_center_size(pt(*center), vec2(*half * 2.0, *half * 2.0));
                    painter.rect_filled(r, 1.0, color);
                    painter.rect_stroke(
                        r,
                        1.0,
                        Stroke::new(1.0, Color32::from_black_alpha(160)),
                        StrokeKind::Outside,
                    );
                }
            }
        }
    }
}

fn draw_hud(painter: &egui::Painter, rect: Rect, st: &AppState) {
    let doc = st.display_doc();
    let proj = if st.camera.orthographic {
        "Orthographic"
    } else {
        "Perspective"
    };
    let mode = match doc.selection.mode {
        SelectMode::Object => "Object Mode",
        SelectMode::Vertex => "Edit Mode \u{b7} Vertex",
        SelectMode::Edge => "Edit Mode \u{b7} Edge",
        SelectMode::Face => "Edit Mode \u{b7} Face",
    };
    let mut line = format!("{proj}  \u{b7}  {mode}  \u{b7}  {}", st.tool.label());
    if let Some(n) = doc
        .selection
        .active()
        .and_then(|a| doc.scene.get(a))
        .map(|n| n.name.clone())
    {
        line.push_str(&format!("  \u{b7}  {n}"));
    }
    let font = FontId::proportional(12.5);
    painter.text(
        rect.left_top() + vec2(11.0, 11.0),
        Align2::LEFT_TOP,
        &line,
        font.clone(),
        Color32::from_black_alpha(160),
    );
    painter.text(
        rect.left_top() + vec2(10.0, 10.0),
        Align2::LEFT_TOP,
        &line,
        font.clone(),
        Color32::from_gray(225),
    );
    if let Some(d) = st.drag_delta() {
        let text = match d {
            DragDelta::Translate(t) => format!("Move  {:+.3}  {:+.3}  {:+.3}", t.x, t.y, t.z),
            DragDelta::Rotate { angle_deg, .. } => format!("Rotate  {angle_deg:+.1}\u{b0}"),
            DragDelta::Scale(s) => format!("Scale  {:.3}  {:.3}  {:.3}", s.x, s.y, s.z),
        };
        let c = rect.center_top() + vec2(0.0, 36.0);
        let galley = painter.layout_no_wrap(text, FontId::monospace(14.0), HIGHLIGHT);
        let r = Rect::from_center_size(c, galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(r, 4.0, Color32::from_black_alpha(170));
        painter.galley(r.min + vec2(8.0, 4.0), galley, HIGHLIGHT);
    }
}

fn draw_axis_widget(painter: &egui::Painter, rect: Rect, st: &AppState) {
    let (r, u, _) = st.camera.to_camera().basis();
    let c = rect.left_bottom() + vec2(56.0, -56.0);
    let len = 30.0;
    let mut axes: Vec<(f32, usize, bool)> = Vec::new();
    for (i, e) in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z].into_iter().enumerate() {
        let f = st.camera.to_camera().forward();
        axes.push((e.dot(f), i, true));
        axes.push((-e.dot(f), i, false));
    }
    axes.sort_by(|a, b| b.0.total_cmp(&a.0));
    painter.circle_filled(c, len + 12.0, Color32::from_black_alpha(55));
    for (_, i, positive) in axes {
        let e = [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z][i]
            * if positive { 1.0 } else { -1.0 };
        let d = vec2(e.dot(r), -e.dot(u)) * len;
        let tip = c + d;
        let col = AXIS_COLORS[i];
        if positive {
            painter.line_segment([c, tip], Stroke::new(2.2, col));
            painter.circle_filled(tip, 7.0, col);
            painter.text(
                tip,
                Align2::CENTER_CENTER,
                ["X", "Y", "Z"][i],
                FontId::proportional(10.0),
                Color32::BLACK,
            );
        } else {
            painter.circle_filled(tip, 4.0, col.gamma_multiply(0.45));
        }
    }
}

/// Convert a euler-angle cache helper for tests.
#[doc(hidden)]
pub fn quat_close(a: Quat, b: Quat) -> bool {
    a.abs_diff_eq(b, 1e-5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_conversion_roundtrips() {
        for v in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-5);
        }
        assert!((linear_to_srgb(0.214_041) - 0.5).abs() < 1e-3);
    }
}

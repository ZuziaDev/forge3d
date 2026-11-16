//! The egui user interface. It only reads the [`AppState`] and turns user
//! input into [`Action`]s / commands on it; it never touches the scene.

use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Modifiers, PointerButton, Rect, RichText,
    Sense, Stroke, StrokeKind, TextureId, Ui,
};
use forge3d_core::math::{euler_deg_from_quat, Axis};
use forge3d_core::prelude::*;
use forge3d_render::ViewPreset;
use glam::{Quat, Vec2};
use uuid::Uuid;

use crate::actions::{MeshTool, PrimitiveDraft, PrimitiveKind};
use crate::camera_input::{self, Button, Mods};
use crate::gizmo::{self, DragDelta, GizmoMode, Handle, Shape};
use crate::keymap::{self, KeyContext, ModalKey};
use crate::modal::ModalKind;
use crate::state::{
    Action, AppState, ConfirmChoice, Guarded, PickRequest, PopupKind, PropertiesTab, StatusKind,
};

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
        // Blender-like neutral dark greys with an orange accent.
        v.panel_fill = Color32::from_rgb(36, 36, 38);
        v.window_fill = Color32::from_rgb(42, 42, 45);
        v.window_stroke = Stroke::new(1.0, Color32::from_rgb(20, 20, 22));
        v.window_corner_radius = egui::CornerRadius::same(6);
        v.menu_corner_radius = egui::CornerRadius::same(5);
        v.extreme_bg_color = Color32::from_rgb(24, 24, 26);
        v.faint_bg_color = Color32::from_rgb(44, 44, 47);
        v.selection.bg_fill = Color32::from_rgb(168, 92, 26);
        v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 214, 170));
        v.hyperlink_color = ACCENT;
        let radius = egui::CornerRadius::same(4);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(26, 26, 28));
        v.widgets.inactive.bg_fill = Color32::from_rgb(70, 70, 74);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(62, 62, 66);
        v.widgets.hovered.bg_fill = Color32::from_rgb(88, 88, 94);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(78, 78, 84);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(120, 120, 128));
        v.widgets.active.bg_fill = Color32::from_rgb(168, 92, 26);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(168, 92, 26);
        v.widgets.open.weak_bg_fill = Color32::from_rgb(78, 78, 84);
        style.spacing.item_spacing = vec2(6.0, 5.0);
        style.spacing.button_padding = vec2(8.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        for font in style.text_styles.values_mut() {
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
    if st.is_modal() {
        modal_shortcuts(&ctx, st);
        return;
    }
    let mut actions: Vec<Action> = Vec::new();
    let popup_open = st.ui.popup.is_some() || st.ui.confirm.is_some();
    let key_ctx = KeyContext {
        edit_mode: st.doc.selection.mode != SelectMode::Object,
    };
    ctx.input_mut(|i| {
        let events: Vec<(Key, Modifiers)> = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect();
        for (key, mods) in events {
            if popup_open && key != Key::Escape {
                continue;
            }
            if let Some(action) = keymap::lookup(key, mods, key_ctx) {
                i.consume_key(mods, key);
                actions.push(action);
            }
        }
    });
    for a in actions {
        st.perform(a);
    }
}

/// Keys while a modal operator runs: axis locks, typed numbers, confirm / cancel.
fn modal_shortcuts(ctx: &egui::Context, st: &mut AppState) {
    let mut keys: Vec<ModalKey> = Vec::new();
    let mut typed: Vec<char> = Vec::new();
    ctx.input_mut(|i| {
        let events: Vec<(Key, Modifiers)> = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect();
        for (key, mods) in events {
            if let Some(k) = keymap::modal_key(key, mods) {
                i.consume_key(mods, key);
                keys.push(k);
            }
        }
        for e in &i.events {
            if let egui::Event::Text(t) = e {
                typed.extend(
                    t.chars()
                        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-')),
                );
            }
        }
    });
    for k in keys {
        match k {
            ModalKey::Confirm => {
                st.confirm_modal();
            }
            ModalKey::Cancel => st.cancel_modal(),
            ModalKey::Axis(a, plane) => st.modal_axis(a, plane),
            ModalKey::Backspace => st.modal_backspace(),
        }
    }
    for c in typed {
        st.modal_type(c);
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

/// A menu entry that performs `action`, showing its shortcut.
fn menu_action(ui: &mut Ui, st: &mut AppState, label: &str, action: Action) -> bool {
    let hit = menu_item(ui, label, keymap::shortcut(&action));
    if hit {
        st.perform(action);
    }
    hit
}

fn menu_bar(ui: &mut Ui, st: &mut AppState) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            for (label, action) in [
                ("New", Action::New),
                ("Open project...", Action::Open),
                ("Save", Action::Save),
                ("Save as...", Action::SaveAs),
                ("-", Action::Cancel),
                ("Import...", Action::Import),
                ("Export...", Action::Export),
                ("-", Action::Cancel),
                ("Quit", Action::Quit),
            ] {
                if label == "-" {
                    ui.separator();
                } else if menu_action(ui, st, label, action) {
                    break;
                }
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
            menu_action(ui, st, "Duplicate and move", Action::DuplicateGrab);
            if menu_item(ui, "Duplicate in place offset", "") {
                st.perform(Action::DuplicateSelected);
            }
            menu_action(ui, st, "Delete", Action::DeleteSelected);
            ui.separator();
            menu_action(ui, st, "Hide", Action::Hide);
            menu_action(ui, st, "Show hidden", Action::UnhideAll);
            ui.separator();
            menu_action(
                ui,
                st,
                "Search commands...",
                Action::Popup(PopupKind::Palette),
            );
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
                ("Object mode", SelectMode::Object, ""),
                ("Vertex mode", SelectMode::Vertex, "1"),
                ("Edge mode", SelectMode::Edge, "2"),
                ("Face mode", SelectMode::Face, "3"),
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
            if menu_item(ui, "Frame selected", "Numpad .") {
                st.perform(Action::FrameSelected);
            }
            if menu_item(ui, "Frame all", "Home") {
                st.perform(Action::FrameAll);
            }
            ui.separator();
            for (label, preset, key) in [
                ("Front", ViewPreset::Front, "Numpad 1"),
                ("Back", ViewPreset::Back, "Ctrl+Numpad 1"),
                ("Right", ViewPreset::Right, "Numpad 3"),
                ("Left", ViewPreset::Left, "Ctrl+Numpad 3"),
                ("Top", ViewPreset::Top, "Numpad 7"),
                ("Bottom", ViewPreset::Bottom, "Ctrl+Numpad 7"),
                ("Isometric", ViewPreset::Iso, "Alt+4"),
            ] {
                if menu_item(ui, label, key) {
                    st.perform(Action::View(preset));
                }
            }
            ui.separator();
            let mut ortho = st.camera.orthographic;
            if ui.checkbox(&mut ortho, "Orthographic (Numpad 5)").clicked() {
                st.perform(Action::ToggleOrtho);
            }
            let mut wire = st.view.wireframe;
            if ui.checkbox(&mut wire, "Wireframe overlay (Z)").clicked() {
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
    for &tool in MeshTool::MENU {
        let action = match tool {
            MeshTool::Extrude => Action::Modal(ModalKind::Extrude),
            MeshTool::Inset => Action::Modal(ModalKind::Inset),
            MeshTool::Bevel => Action::Modal(ModalKind::Bevel),
            t => Action::MeshTool(t),
        };
        let sc = keymap::shortcut(&action);
        if ui
            .add(egui::Button::new(tool.label()).shortcut_text(sc))
            .clicked()
        {
            // Menu entries apply the toolbar parameters at once; the keys start the modal version.
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
            ("Object", SelectMode::Object, "Tab"),
            ("Vertex", SelectMode::Vertex, "1, in edit mode"),
            ("Edge", SelectMode::Edge, "2, in edit mode"),
            ("Face", SelectMode::Face, "3, in edit mode"),
        ] {
            if ui
                .selectable_label(mode == m, label)
                .on_hover_text(format!("{label} mode ({tip}). Tab toggles edit mode."))
                .clicked()
            {
                st.perform(Action::SetMode(m));
            }
        }
        ui.separator();
        for (t, tip) in [
            (
                GizmoMode::Translate,
                "Move gizmo. Press G for the modal move.",
            ),
            (
                GizmoMode::Rotate,
                "Rotate gizmo. Press R for the modal rotate.",
            ),
            (
                GizmoMode::Scale,
                "Scale gizmo. Press S for the modal scale.",
            ),
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
        (
            "Wire",
            st.view.wireframe,
            Action::ToggleWireframe,
            "Wireframe overlay (Z)",
        ),
        ("Grid", st.view.grid, Action::ToggleGrid, "Ground grid"),
        (
            "Smooth",
            st.view.smooth_shading,
            Action::ToggleSmooth,
            "Smooth shading",
        ),
        (
            "Ortho",
            st.camera.orthographic,
            Action::ToggleOrtho,
            "Orthographic camera (Numpad 5)",
        ),
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
                if ui
                    .small_button("...")
                    .on_hover_text("With parameters")
                    .clicked()
                {
                    st.ui.primitive_dialog = Some(PrimitiveDraft::of(kind));
                    ui.close();
                }
            });
        }
    });
    if ui
        .button("Duplicate")
        .on_hover_text("Duplicate with a small offset (Shift+D duplicates and moves)")
        .clicked()
    {
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
        let tip = match tool {
            MeshTool::Extrude => {
                "E: modal extrude along the normals. The button applies the value."
            }
            MeshTool::Inset => "I: modal inset. The button applies the value.",
            MeshTool::Bevel => "Ctrl+B: modal bevel. The button applies the value.",
            MeshTool::LoopCut => "Ctrl+R: loop cut across the selected edge",
            _ => "",
        };
        if ui.button(tool.label()).on_hover_text(tip).clicked() {
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
        param(ui, &mut p.bevel_width, 0.01, 0.0..=100.0);
        let mut seg = p.bevel_segments;
        if ui
            .add(
                egui::DragValue::new(&mut seg)
                    .range(1..=16)
                    .speed(0.1)
                    .suffix(" seg"),
            )
            .on_hover_text("Bevel segments")
            .changed()
        {
            p.bevel_segments = seg;
        }
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
            let file = st.path.as_ref().and_then(|p| p.file_name()).map_or_else(
                || "untitled".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
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
        paint_eye(
            ui.painter(),
            eye_rect,
            visible,
            if eye.hovered() { ACCENT } else { eye_color },
        );
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
        let icon_color = if active {
            Color32::WHITE
        } else {
            Color32::from_gray(150)
        };
        let ic = pos2(rect.left() + 9.0, rect.center().y);
        if has_mesh {
            ui.painter()
                .rect_filled(Rect::from_center_size(ic, vec2(8.0, 8.0)), 1.5, icon_color);
        } else {
            ui.painter()
                .circle_stroke(ic, 4.0, Stroke::new(1.3, icon_color));
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
                .add_enabled(
                    active_other.is_some(),
                    egui::Button::new("Parent to active object"),
                )
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
        coll.show_header(ui, |ui| row(ui, acts, st)).body(|ui| {
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
                let r = ui
                    .selectable_label(current, text)
                    .on_hover_text(&e.description);
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
    ui.horizontal(|ui| {
        for (tab, label, tip) in [
            (
                PropertiesTab::Object,
                "Object",
                "Transform, mesh, shading and material",
            ),
            (
                PropertiesTab::Modifiers,
                "Modifiers",
                "Non-destructive modifier stack",
            ),
        ] {
            if ui
                .selectable_label(st.ui.properties_tab == tab, label)
                .on_hover_text(tip)
                .clicked()
            {
                st.ui.properties_tab = tab;
            }
        }
    });
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
            if st.ui.properties_tab == PropertiesTab::Modifiers {
                modifiers_panel(ui, st, id);
                return;
            }
            let name = node.name.clone();
            let (scale, rot, trans) = node.transform.to_scale_rotation_translation();
            let material = node.material.clone();
            let (smooth, auto_angle) = (node.shade_smooth, node.auto_smooth_angle_deg);
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

            // shading
            egui::CollapsingHeader::new("Shading")
                .default_open(true)
                .show(ui, |ui| shading_editor(ui, st, id, smooth, auto_angle));

            // material
            egui::CollapsingHeader::new("Material")
                .default_open(true)
                .show(ui, |ui| material_editor(ui, st, id, material));
        });
}

fn shading_editor(ui: &mut Ui, st: &mut AppState, id: Uuid, smooth: bool, auto_angle: Option<f32>) {
    let mut smooth_now = smooth;
    if ui
        .checkbox(&mut smooth_now, "Smooth")
        .on_hover_text("Smooth shading for this object (flat otherwise)")
        .changed()
    {
        st.run(CommandSpec::SetShading {
            objects: Some(vec![id]),
            smooth: smooth_now,
            auto_smooth_angle_deg: auto_angle,
        });
    }
    ui.add_enabled_ui(smooth, |ui| {
        let mut auto = auto_angle.is_some();
        let mut angle = auto_angle.unwrap_or(30.0);
        let mut changed = ui
            .checkbox(&mut auto, "Auto smooth")
            .on_hover_text("Keep edges sharper than the angle hard")
            .changed();
        ui.add_enabled_ui(auto, |ui| {
            ui.horizontal(|ui| {
                ui.label("Angle");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut angle)
                            .range(0.0..=180.0)
                            .suffix("\u{b0}"),
                    )
                    .changed();
            });
        });
        if changed {
            st.preview(
                &format!("shading:{id}"),
                CommandSpec::SetShading {
                    objects: Some(vec![id]),
                    smooth: true,
                    auto_smooth_angle_deg: auto.then_some(angle),
                },
            );
        }
    });
}

fn default_modifier(label: &str) -> Option<ModifierKind> {
    Some(match label {
        "Mirror" => ModifierKind::Mirror {
            axis: Axis::X,
            merge: true,
            merge_threshold: 1e-3,
        },
        "Array" => ModifierKind::Array {
            count: 2,
            offset: [1.0, 0.0, 0.0],
            merge: false,
            merge_threshold: 1e-3,
        },
        "Subdivision" => ModifierKind::Subdivision { levels: 1 },
        "Solidify" => ModifierKind::Solidify { thickness: 0.1 },
        "Bevel" => ModifierKind::Bevel {
            width: 0.1,
            segments: 2,
            angle_deg: 30.0,
        },
        "Triangulate" => ModifierKind::Triangulate,
        _ => return None,
    })
}

/// Edit the parameters of `kind`; true when something changed.
fn modifier_params(ui: &mut Ui, kind: &mut ModifierKind) -> bool {
    let mut changed = false;
    let drag = |ui: &mut Ui,
                label: &str,
                v: &mut f32,
                speed: f64,
                range: std::ops::RangeInclusive<f32>|
     -> bool {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.add(
                egui::DragValue::new(v)
                    .speed(speed)
                    .range(range)
                    .max_decimals(3),
            )
            .changed()
        })
        .inner
    };
    match kind {
        ModifierKind::Mirror {
            axis,
            merge,
            merge_threshold,
        } => {
            changed |= ui
                .horizontal(|ui| {
                    let mut c = false;
                    ui.label("Axis");
                    for (a, n) in [(Axis::X, "X"), (Axis::Y, "Y"), (Axis::Z, "Z")] {
                        if ui.selectable_label(*axis == a, n).clicked() {
                            *axis = a;
                            c = true;
                        }
                    }
                    c
                })
                .inner;
            changed |= ui.checkbox(merge, "Merge at the plane").changed();
            changed |= drag(ui, "Threshold", merge_threshold, 0.0005, 0.0..=10.0);
        }
        ModifierKind::Array {
            count,
            offset,
            merge,
            merge_threshold,
        } => {
            changed |= ui
                .horizontal(|ui| {
                    ui.label("Count");
                    ui.add(egui::DragValue::new(count).range(1..=256)).changed()
                })
                .inner;
            for (i, n) in ["Offset X", "Offset Y", "Offset Z"].into_iter().enumerate() {
                changed |= drag(ui, n, &mut offset[i], 0.02, -1000.0..=1000.0);
            }
            changed |= ui.checkbox(merge, "Merge touching copies").changed();
            changed |= drag(ui, "Threshold", merge_threshold, 0.0005, 0.0..=10.0);
        }
        ModifierKind::Subdivision { levels } => {
            changed |= ui
                .horizontal(|ui| {
                    ui.label("Levels");
                    ui.add(egui::DragValue::new(levels).range(1..=4)).changed()
                })
                .inner;
        }
        ModifierKind::Solidify { thickness } => {
            changed |= drag(ui, "Thickness", thickness, 0.01, -100.0..=100.0);
        }
        ModifierKind::Bevel {
            width,
            segments,
            angle_deg,
        } => {
            changed |= drag(ui, "Width", width, 0.01, 0.0..=100.0);
            changed |= ui
                .horizontal(|ui| {
                    ui.label("Segments");
                    ui.add(egui::DragValue::new(segments).range(1..=16))
                        .changed()
                })
                .inner;
            changed |= drag(ui, "Angle", angle_deg, 0.5, 0.0..=180.0);
        }
        ModifierKind::Triangulate => {
            ui.weak("No parameters");
        }
    }
    changed
}

/// The Modifiers tab: add, enable, reorder, edit, apply and remove.
fn modifiers_panel(ui: &mut Ui, st: &mut AppState, id: Uuid) {
    let Some(node) = st.display_doc().scene.get(id) else {
        return;
    };
    if node.mesh.is_none() {
        ui.weak("Group nodes have no modifiers.");
        return;
    }
    let stack = node.modifiers.clone();
    ui.menu_button("+ Add modifier", |ui| {
        for label in [
            "Mirror",
            "Array",
            "Subdivision",
            "Solidify",
            "Bevel",
            "Triangulate",
        ] {
            if ui.button(label).clicked() {
                if let Some(kind) = default_modifier(label) {
                    st.run(CommandSpec::AddModifier {
                        object: Some(id),
                        modifier: kind,
                        enabled: true,
                        index: None,
                    });
                }
                ui.close();
            }
        }
    });
    if stack.is_empty() {
        ui.add_space(6.0);
        ui.weak("No modifiers. They change the shape without touching the base mesh.");
        return;
    }
    let last = stack.len() - 1;
    for (i, m) in stack.iter().enumerate() {
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut enabled = m.enabled;
                if ui
                    .checkbox(&mut enabled, "")
                    .on_hover_text("Enable or disable in the viewport")
                    .changed()
                {
                    st.run(CommandSpec::SetModifier {
                        object: Some(id),
                        index: i,
                        modifier: None,
                        enabled: Some(enabled),
                    });
                }
                ui.label(RichText::new(m.kind.type_name()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{2715}")
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        st.run(CommandSpec::RemoveModifier {
                            object: Some(id),
                            index: i,
                        });
                    }
                    if ui
                        .small_button("Apply")
                        .on_hover_text("Bake into the base mesh")
                        .clicked()
                    {
                        st.run(CommandSpec::ApplyModifier {
                            object: Some(id),
                            index: Some(i),
                        });
                    }
                    if ui
                        .add_enabled(i < last, egui::Button::new("\u{25bc}").small())
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        st.run(CommandSpec::MoveModifier {
                            object: Some(id),
                            from: i,
                            to: i + 1,
                        });
                    }
                    if ui
                        .add_enabled(i > 0, egui::Button::new("\u{25b2}").small())
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        st.run(CommandSpec::MoveModifier {
                            object: Some(id),
                            from: i,
                            to: i - 1,
                        });
                    }
                });
            });
            let mut kind = m.kind.clone();
            ui.add_enabled_ui(m.enabled, |ui| {
                if modifier_params(ui, &mut kind) {
                    st.preview(
                        &format!("modifier:{id}:{i}"),
                        CommandSpec::SetModifier {
                            object: Some(id),
                            index: i,
                            modifier: Some(kind),
                            enabled: None,
                        },
                    );
                }
            });
        });
    }
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
            ui.label(
                RichText::new(label)
                    .color(if on { ok } else { bad })
                    .small(),
            );
        };
        flag(ui, r.is_manifold, "manifold");
        flag(ui, r.is_closed, "closed");
        flag(ui, r.is_consistently_oriented, "oriented");
        if r.boundary_edges > 0 {
            ui.label(
                RichText::new(format!("{} boundary edges", r.boundary_edges))
                    .small()
                    .weak(),
            );
        }
        if r.degenerate_faces > 0 {
            ui.label(
                RichText::new(format!("{} degenerate faces", r.degenerate_faces))
                    .small()
                    .weak(),
            );
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
    popups(ctx, st);
    confirm_dialog(ctx, st);
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
                egui::Grid::new("prim_params")
                    .num_columns(2)
                    .show(ui, |ui| {
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

/// Fill in the popup position on first draw (the cursor) and keep it on screen.
fn popup_pos(ctx: &egui::Context, st: &mut AppState) -> egui::Pos2 {
    let screen = ctx.content_rect();
    let Some(p) = st.ui.popup.as_mut() else {
        return screen.center();
    };
    if p.pos.is_none() {
        let at = ctx
            .input(|i| i.pointer.latest_pos())
            .unwrap_or_else(|| screen.center());
        p.pos = Some((at.x, at.y));
    }
    let (x, y) = p.pos.unwrap_or((0.0, 0.0));
    pos2(
        x.clamp(screen.left(), (screen.right() - 220.0).max(screen.left())),
        y.clamp(screen.top(), (screen.bottom() - 260.0).max(screen.top())),
    )
}

fn popups(ctx: &egui::Context, st: &mut AppState) {
    let Some(popup) = st.ui.popup else {
        return;
    };
    let pos = popup_pos(ctx, st);
    let mut chosen: Option<Action> = None;
    let area = egui::Area::new(egui::Id::unique("forge3d_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(190.0);
                match popup.kind {
                    PopupKind::Add => {
                        ui.label(RichText::new("Add").strong().color(ACCENT));
                        ui.separator();
                        for kind in PrimitiveKind::ALL {
                            if ui.button(kind.label()).clicked() {
                                chosen = Some(Action::AddPrimitive(
                                    PrimitiveDraft::of(kind).primitive(),
                                ));
                            }
                        }
                    }
                    PopupKind::Delete => {
                        ui.label(RichText::new("Delete").strong().color(ACCENT));
                        ui.separator();
                        if st.doc.selection.mode == SelectMode::Object {
                            if ui.button("Objects").clicked() {
                                chosen = Some(Action::DeleteSelected);
                            }
                        } else {
                            for (label, kind) in [
                                ("Vertices", ElementKind::Vertex),
                                ("Edges", ElementKind::Edge),
                                ("Faces", ElementKind::Face),
                            ] {
                                if ui.button(label).clicked() {
                                    st.perform(Action::Cancel);
                                    st.run(CommandSpec::DeleteElements {
                                        object: st.doc.selection.active(),
                                        kind: Some(kind),
                                        indices: None,
                                    });
                                }
                            }
                        }
                    }
                    PopupKind::Palette => palette(ui, st, &mut chosen),
                }
            });
        });
    let outside_click = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    if st.ui.popup.is_none() {
        return;
    }
    if let Some(a) = chosen {
        st.ui.popup = None;
        st.perform(a);
    } else if outside_click || ctx.input(|i| i.key_pressed(Key::Escape)) {
        st.ui.popup = None;
    }
}

/// F3 command search: fuzzy list of every action.
fn palette(ui: &mut Ui, st: &mut AppState, chosen: &mut Option<Action>) {
    ui.set_min_width(380.0);
    let r = ui.add(
        egui::TextEdit::singleline(&mut st.ui.palette_query)
            .hint_text("Search commands...")
            .desired_width(f32::INFINITY),
    );
    r.request_focus();
    let hits = keymap::search(&st.ui.palette_query);
    let shown = hits.len().min(14);
    if r.changed() {
        st.ui.palette_sel = 0;
    }
    ui.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            st.ui.palette_sel = (st.ui.palette_sel + 1).min(shown.saturating_sub(1));
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            st.ui.palette_sel = st.ui.palette_sel.saturating_sub(1);
        }
    });
    st.ui.palette_sel = st.ui.palette_sel.min(shown.saturating_sub(1));
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    ui.separator();
    for (i, c) in hits.iter().take(shown).enumerate() {
        let selected = i == st.ui.palette_sel;
        let resp = ui.add(
            egui::Button::new(&c.label)
                .shortcut_text(c.shortcut)
                .selected(selected)
                .min_size(vec2(ui.available_width(), 0.0)),
        );
        if resp.clicked() || (enter && selected) {
            *chosen = Some(c.action.clone());
        }
    }
    if hits.is_empty() {
        ui.weak("No matching command");
    }
}

fn confirm_dialog(ctx: &egui::Context, st: &mut AppState) {
    let Some(what) = st.ui.confirm else {
        return;
    };
    let mut choice: Option<ConfirmChoice> = None;
    egui::Window::new("Unsaved changes")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -60.0))
        .show(ctx, |ui| {
            let what = match what {
                Guarded::New => "start a new scene",
                Guarded::Open => "open another project",
                Guarded::Exit => "quit",
            };
            ui.label(format!(
                "The scene has unsaved changes. Save them before you {what}?"
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(ConfirmChoice::Save);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(ConfirmChoice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(ConfirmChoice::Cancel);
                }
            });
        });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        choice = Some(ConfirmChoice::Cancel);
    }
    if let Some(c) = choice {
        st.resolve_confirm(c);
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
            ui.add(
                egui::DragValue::new(&mut d.radius_top)
                    .speed(0.02)
                    .range(0.0..=1000.0),
            );
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
            .map_or_else(
                || std::env::current_dir().unwrap_or_default(),
                |p| p.to_path_buf(),
            )
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
                            (p.is_dir(), e.file_name().to_string_lossy().into_owned(), p)
                        })
                        .filter(|(_, n, _)| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
            entries.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
            for (is_dir, name, path) in entries.into_iter().take(300) {
                let label = if is_dir {
                    format!("\u{25B8} {name}")
                } else {
                    name
                };
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
    let modal_frame = st.is_modal();
    if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
        if rect.contains(p) {
            st.cursor = v2(p, origin);
        }
    }
    if modal_frame {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            st.update_modal(v2(p, origin), mods.ctrl);
        }
        if ui.input(|i| i.pointer.button_released(PointerButton::Primary)) {
            st.confirm_modal();
        } else if ui.input(|i| i.pointer.button_released(PointerButton::Secondary)) {
            st.cancel_modal();
        }
        ui.ctx().request_repaint();
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    let press = ui.input(|i| i.pointer.press_origin());
    let hover = resp.hover_pos().map(|p| v2(p, origin));
    let viewport_h = rect.height();

    // --- zoom
    if resp.hovered() && !modal_frame {
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
        if resp.dragged_by(pb) && !modal_frame {
            if let Some(nav) = camera_input::nav_for_drag(b, mods) {
                st.pending_frame = 0;
                let d = resp.drag_delta();
                camera_input::apply_drag(&mut st.camera, nav, d.x, d.y, viewport_h);
            }
        }
    }

    // --- gizmo hover / drag
    let nav_gesture = mods.alt || modal_frame;
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
    if !st.is_modal() {
        draw_gizmo(&painter, rect, st);
    }
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
        let mut color = if lit {
            HIGHLIGHT
        } else {
            handle_color(part.handle)
        };
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
    if let Some(text) = st.modal_hud() {
        let c = rect.center_top() + vec2(0.0, 36.0);
        let galley = painter.layout_no_wrap(text, FontId::monospace(14.0), HIGHLIGHT);
        let r = Rect::from_center_size(c, galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(r, 4.0, Color32::from_black_alpha(170));
        painter.galley(r.min + vec2(8.0, 4.0), galley, HIGHLIGHT);
    }
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
    for (i, e) in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z]
        .into_iter()
        .enumerate()
    {
        let f = st.camera.to_camera().forward();
        axes.push((e.dot(f), i, true));
        axes.push((-e.dot(f), i, false));
    }
    axes.sort_by(|a, b| b.0.total_cmp(&a.0));
    painter.circle_filled(c, len + 12.0, Color32::from_black_alpha(55));
    for (_, i, positive) in axes {
        let e =
            [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z][i] * if positive { 1.0 } else { -1.0 };
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

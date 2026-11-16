//! Blender-style keymap: key chords to [`Action`]s, plus the catalogue of all
//! actions (command palette, tooltips) with their shortcut text.

use egui::{Key, Modifiers};
use forge3d_core::prelude::*;
use forge3d_render::ViewPreset;

use crate::actions::{MeshTool, PrimitiveDraft, PrimitiveKind};
use crate::modal::ModalKind;
use crate::state::{Action, PopupKind};

/// What the keymap needs to know about the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyContext {
    /// An element selection mode (edit mode) is active.
    pub edit_mode: bool,
}

/// Map a key press to an action. Modal operators are handled separately
/// (see [`modal_key`]).
pub fn lookup(key: Key, m: Modifiers, ctx: KeyContext) -> Option<Action> {
    let ctrl = m.command || m.ctrl;
    let (shift, alt) = (m.shift, m.alt);
    use Action as A;
    Some(match (key, ctrl, shift, alt) {
        // --- file / history
        (Key::N, true, false, false) => A::New,
        (Key::O, true, false, false) => A::Open,
        (Key::S, true, false, false) => A::Save,
        (Key::S, true, true, false) => A::SaveAs,
        (Key::Q, true, false, false) => A::Quit,
        (Key::Z, true, false, false) => A::Undo,
        (Key::Z, true, true, false) | (Key::Y, true, false, false) => A::Redo,
        (Key::F3, false, false, false) => A::Popup(PopupKind::Palette),
        (Key::Escape, false, false, false) => A::Cancel,
        // --- selection
        (Key::A, false, false, false) => A::SelectAll,
        (Key::A, false, false, true) => A::SelectNone,
        (Key::I, true, false, false) => A::SelectInvert,
        (Key::H, false, false, false) => A::Hide,
        (Key::H, false, false, true) => A::UnhideAll,
        // --- add / delete / duplicate
        (Key::A, false, true, false) => A::Popup(PopupKind::Add),
        (Key::X, false, false, false) => A::Popup(PopupKind::Delete),
        (Key::Delete, false, false, false) => A::DeleteSelected,
        (Key::D, false, true, false) => A::DuplicateGrab,
        // --- modal transforms
        (Key::G, false, false, false) => A::Modal(ModalKind::Grab),
        (Key::R, false, false, false) => A::Modal(ModalKind::Rotate),
        (Key::S, false, false, false) => A::Modal(ModalKind::Scale),
        // --- modes
        (Key::Tab, false, false, false) => A::ToggleEditMode,
        (Key::Num1, false, false, false) if ctx.edit_mode => A::SetMode(SelectMode::Vertex),
        (Key::Num2, false, false, false) if ctx.edit_mode => A::SetMode(SelectMode::Edge),
        (Key::Num3, false, false, false) if ctx.edit_mode => A::SetMode(SelectMode::Face),
        // --- mesh tools (edit mode)
        (Key::E, false, false, false) if ctx.edit_mode => A::Modal(ModalKind::Extrude),
        (Key::I, false, false, false) if ctx.edit_mode => A::Modal(ModalKind::Inset),
        (Key::B, true, false, false) if ctx.edit_mode => A::Modal(ModalKind::Bevel),
        (Key::R, true, false, false) if ctx.edit_mode => A::MeshTool(MeshTool::LoopCut),
        (Key::P, false, false, false) if ctx.edit_mode => A::MeshTool(MeshTool::Separate),
        (Key::J, true, false, false) => A::MeshTool(MeshTool::JoinObjects),
        // --- view
        (Key::Z, false, false, false) => A::ToggleWireframe,
        (Key::Home, false, false, false) => A::FrameAll,
        (Key::Num1, false, false, true) => A::View(ViewPreset::Front),
        (Key::Num2, false, false, true) => A::View(ViewPreset::Right),
        (Key::Num3, false, false, true) => A::View(ViewPreset::Top),
        (Key::Num4, false, false, true) => A::View(ViewPreset::Iso),
        _ => return None,
    })
}

/// Keypad keys (egui cannot tell them from the digit row, so the window layer
/// calls this with winit's physical key names).
pub fn numpad(name: &str, ctrl: bool) -> Option<Action> {
    Some(match (name, ctrl) {
        ("1", false) => Action::View(ViewPreset::Front),
        ("1", true) => Action::View(ViewPreset::Back),
        ("3", false) => Action::View(ViewPreset::Right),
        ("3", true) => Action::View(ViewPreset::Left),
        ("7", false) => Action::View(ViewPreset::Top),
        ("7", true) => Action::View(ViewPreset::Bottom),
        ("5", _) => Action::ToggleOrtho,
        (".", _) => Action::FrameSelected,
        _ => return None,
    })
}

/// Keys that steer a running modal operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKey {
    Confirm,
    Cancel,
    Axis(usize, bool),
    Backspace,
}

/// Key handling while a modal operator runs (digits arrive as text events).
pub fn modal_key(key: Key, m: Modifiers) -> Option<ModalKey> {
    Some(match key {
        Key::Enter => ModalKey::Confirm,
        Key::Escape => ModalKey::Cancel,
        Key::X => ModalKey::Axis(0, m.shift),
        Key::Y => ModalKey::Axis(1, m.shift),
        Key::Z => ModalKey::Axis(2, m.shift),
        Key::Backspace => ModalKey::Backspace,
        _ => return None,
    })
}

/// One entry of the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub label: String,
    pub shortcut: &'static str,
    pub action: Action,
}

fn cmd(label: impl Into<String>, shortcut: &'static str, action: Action) -> Command {
    Command {
        label: label.into(),
        shortcut,
        action,
    }
}

/// Every action reachable from the palette, with its shortcut.
pub fn catalog() -> Vec<Command> {
    use Action as A;
    let mut v = vec![
        cmd("File: New", "Ctrl+N", A::New),
        cmd("File: Open project", "Ctrl+O", A::Open),
        cmd("File: Save", "Ctrl+S", A::Save),
        cmd("File: Save as", "Ctrl+Shift+S", A::SaveAs),
        cmd("File: Import", "", A::Import),
        cmd("File: Export", "", A::Export),
        cmd("File: Quit", "Ctrl+Q", A::Quit),
        cmd("Edit: Undo", "Ctrl+Z", A::Undo),
        cmd("Edit: Redo", "Ctrl+Shift+Z", A::Redo),
        cmd("Select: All", "A", A::SelectAll),
        cmd("Select: None", "Alt+A", A::SelectNone),
        cmd("Select: Invert", "Ctrl+I", A::SelectInvert),
        cmd("Object: Hide", "H", A::Hide),
        cmd("Object: Show hidden", "Alt+H", A::UnhideAll),
        cmd("Object: Duplicate and move", "Shift+D", A::DuplicateGrab),
        cmd("Object: Delete", "X", A::DeleteSelected),
        cmd("Transform: Move", "G", A::Modal(ModalKind::Grab)),
        cmd("Transform: Rotate", "R", A::Modal(ModalKind::Rotate)),
        cmd("Transform: Scale", "S", A::Modal(ModalKind::Scale)),
        cmd("Mode: Toggle edit mode", "Tab", A::ToggleEditMode),
        cmd("Mode: Object", "", A::SetMode(SelectMode::Object)),
        cmd("Mode: Vertex", "1", A::SetMode(SelectMode::Vertex)),
        cmd("Mode: Edge", "2", A::SetMode(SelectMode::Edge)),
        cmd("Mode: Face", "3", A::SetMode(SelectMode::Face)),
        cmd("Mesh: Extrude", "E", A::Modal(ModalKind::Extrude)),
        cmd("Mesh: Inset", "I", A::Modal(ModalKind::Inset)),
        cmd("Mesh: Bevel", "Ctrl+B", A::Modal(ModalKind::Bevel)),
        cmd("Mesh: Loop cut", "Ctrl+R", A::MeshTool(MeshTool::LoopCut)),
        cmd(
            "Mesh: Separate selection",
            "P",
            A::MeshTool(MeshTool::Separate),
        ),
        cmd(
            "Object: Join objects",
            "Ctrl+J",
            A::MeshTool(MeshTool::JoinObjects),
        ),
        cmd("View: Front", "Numpad 1", A::View(ViewPreset::Front)),
        cmd("View: Back", "Ctrl+Numpad 1", A::View(ViewPreset::Back)),
        cmd("View: Right", "Numpad 3", A::View(ViewPreset::Right)),
        cmd("View: Left", "Ctrl+Numpad 3", A::View(ViewPreset::Left)),
        cmd("View: Top", "Numpad 7", A::View(ViewPreset::Top)),
        cmd("View: Bottom", "Ctrl+Numpad 7", A::View(ViewPreset::Bottom)),
        cmd("View: Isometric", "Alt+4", A::View(ViewPreset::Iso)),
        cmd("View: Toggle orthographic", "Numpad 5", A::ToggleOrtho),
        cmd("View: Frame selected", "Numpad .", A::FrameSelected),
        cmd("View: Frame all", "Home", A::FrameAll),
        cmd("View: Toggle wireframe", "Z", A::ToggleWireframe),
        cmd("View: Toggle grid", "", A::ToggleGrid),
        cmd("View: Toggle smooth shading", "", A::ToggleSmooth),
        cmd("Add: Menu", "Shift+A", A::Popup(PopupKind::Add)),
    ];
    for kind in PrimitiveKind::ALL {
        v.push(cmd(
            format!("Add: {}", kind.label()),
            "",
            A::AddPrimitive(PrimitiveDraft::of(kind).primitive()),
        ));
    }
    for &tool in MeshTool::MENU {
        if v.iter().any(|c| c.action == A::MeshTool(tool)) || tool.is_modal() {
            continue;
        }
        v.push(cmd(
            format!("Mesh: {}", tool.label()),
            "",
            A::MeshTool(tool),
        ));
    }
    for (label, op) in [
        ("Boolean: Union", BooleanOp::Union),
        ("Boolean: Difference", BooleanOp::Difference),
        ("Boolean: Intersect", BooleanOp::Intersect),
    ] {
        v.push(cmd(label, "", A::Boolean(op)));
    }
    v
}

/// Shortcut text of `action` ("" when it has none).
pub fn shortcut(action: &Action) -> &'static str {
    static CATALOG: std::sync::OnceLock<Vec<Command>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(catalog)
        .iter()
        .find(|c| &c.action == action)
        .map_or("", |c| c.shortcut)
}

/// Subsequence fuzzy match score (higher is better, `None` = no match).
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 10;
            if last == Some(i.wrapping_sub(1)) {
                score += 15; // consecutive
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                score += 12; // word start
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score - t.len() as i32 / 4)
}

/// Catalogue entries matching `query`, best first.
pub fn search(query: &str) -> Vec<Command> {
    let mut hits: Vec<(i32, Command)> = catalog()
        .into_iter()
        .filter_map(|c| fuzzy(query, &c.label).map(|s| (s, c)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    hits.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJ: KeyContext = KeyContext { edit_mode: false };
    const EDIT: KeyContext = KeyContext { edit_mode: true };

    fn plain() -> Modifiers {
        Modifiers::NONE
    }

    #[test]
    fn join_and_separate_keys() {
        assert_eq!(
            lookup(Key::J, Modifiers::COMMAND, OBJ),
            Some(Action::MeshTool(MeshTool::JoinObjects))
        );
        assert_eq!(lookup(Key::P, plain(), OBJ), None);
        assert_eq!(
            lookup(Key::P, plain(), EDIT),
            Some(Action::MeshTool(MeshTool::Separate))
        );
        assert!(catalog().iter().any(|c| c.label == "Mesh: Solidify"));
    }

    #[test]
    fn blender_transform_keys() {
        assert_eq!(
            lookup(Key::G, plain(), OBJ),
            Some(Action::Modal(ModalKind::Grab))
        );
        assert_eq!(
            lookup(Key::R, plain(), OBJ),
            Some(Action::Modal(ModalKind::Rotate))
        );
        assert_eq!(
            lookup(Key::S, plain(), OBJ),
            Some(Action::Modal(ModalKind::Scale))
        );
        assert_eq!(
            lookup(Key::D, Modifiers::SHIFT, OBJ),
            Some(Action::DuplicateGrab)
        );
        assert_eq!(
            lookup(Key::A, Modifiers::SHIFT, OBJ),
            Some(Action::Popup(PopupKind::Add))
        );
        assert_eq!(
            lookup(Key::X, plain(), OBJ),
            Some(Action::Popup(PopupKind::Delete))
        );
        assert_eq!(
            lookup(Key::Delete, plain(), OBJ),
            Some(Action::DeleteSelected)
        );
    }

    #[test]
    fn edit_mode_keys_only_work_in_edit_mode() {
        assert_eq!(lookup(Key::E, plain(), OBJ), None);
        assert_eq!(
            lookup(Key::E, plain(), EDIT),
            Some(Action::Modal(ModalKind::Extrude))
        );
        assert_eq!(
            lookup(Key::I, plain(), EDIT),
            Some(Action::Modal(ModalKind::Inset))
        );
        assert_eq!(
            lookup(Key::B, Modifiers::COMMAND, EDIT),
            Some(Action::Modal(ModalKind::Bevel))
        );
        assert_eq!(
            lookup(Key::R, Modifiers::COMMAND, EDIT),
            Some(Action::MeshTool(MeshTool::LoopCut))
        );
        assert_eq!(lookup(Key::Num1, plain(), OBJ), None);
        assert_eq!(
            lookup(Key::Num1, plain(), EDIT),
            Some(Action::SetMode(SelectMode::Vertex))
        );
        assert_eq!(
            lookup(Key::Num2, plain(), EDIT),
            Some(Action::SetMode(SelectMode::Edge))
        );
        assert_eq!(
            lookup(Key::Num3, plain(), EDIT),
            Some(Action::SetMode(SelectMode::Face))
        );
        assert_eq!(lookup(Key::Tab, plain(), OBJ), Some(Action::ToggleEditMode));
    }

    #[test]
    fn selection_visibility_and_file_keys() {
        assert_eq!(lookup(Key::A, plain(), OBJ), Some(Action::SelectAll));
        assert_eq!(
            lookup(Key::A, Modifiers::ALT, OBJ),
            Some(Action::SelectNone)
        );
        assert_eq!(
            lookup(Key::I, Modifiers::COMMAND, OBJ),
            Some(Action::SelectInvert)
        );
        assert_eq!(lookup(Key::H, plain(), OBJ), Some(Action::Hide));
        assert_eq!(lookup(Key::H, Modifiers::ALT, OBJ), Some(Action::UnhideAll));
        assert_eq!(lookup(Key::S, Modifiers::COMMAND, OBJ), Some(Action::Save));
        assert_eq!(lookup(Key::O, Modifiers::COMMAND, OBJ), Some(Action::Open));
        assert_eq!(lookup(Key::N, Modifiers::COMMAND, OBJ), Some(Action::New));
        assert_eq!(lookup(Key::Z, Modifiers::COMMAND, OBJ), Some(Action::Undo));
        assert_eq!(
            lookup(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT, OBJ),
            Some(Action::Redo)
        );
        assert_eq!(lookup(Key::Z, plain(), OBJ), Some(Action::ToggleWireframe));
        assert_eq!(
            lookup(Key::F3, plain(), OBJ),
            Some(Action::Popup(PopupKind::Palette))
        );
        assert_eq!(lookup(Key::Q, plain(), OBJ), None);
    }

    #[test]
    fn numpad_views() {
        assert_eq!(numpad("1", false), Some(Action::View(ViewPreset::Front)));
        assert_eq!(numpad("3", true), Some(Action::View(ViewPreset::Left)));
        assert_eq!(numpad("7", false), Some(Action::View(ViewPreset::Top)));
        assert_eq!(numpad("5", false), Some(Action::ToggleOrtho));
        assert_eq!(numpad(".", false), Some(Action::FrameSelected));
        assert_eq!(numpad("9", false), None);
    }

    #[test]
    fn modal_keys() {
        assert_eq!(modal_key(Key::Enter, plain()), Some(ModalKey::Confirm));
        assert_eq!(modal_key(Key::Escape, plain()), Some(ModalKey::Cancel));
        assert_eq!(modal_key(Key::X, plain()), Some(ModalKey::Axis(0, false)));
        assert_eq!(
            modal_key(Key::Z, Modifiers::SHIFT),
            Some(ModalKey::Axis(2, true))
        );
        assert_eq!(modal_key(Key::G, plain()), None);
    }

    #[test]
    fn palette_search_is_fuzzy_and_ranked() {
        let hits = search("extr");
        assert_eq!(hits[0].label, "Mesh: Extrude");
        let hits = search("tgl edit");
        assert_eq!(hits[0].action, Action::ToggleEditMode);
        assert!(search("zzzzqq").is_empty());
        assert!(search("").len() > 30);
        let hits = search("wire");
        assert_eq!(hits[0].action, Action::ToggleWireframe);
        assert!(fuzzy("sv", "Save").is_some());
        assert!(fuzzy("ev", "Save").is_none());
    }

    #[test]
    fn every_shortcut_in_the_catalogue_is_reachable_by_a_chord_or_numpad() {
        // Spot-check that catalogue text and keymap agree for the plain keys.
        for (text, key) in [("G", Key::G), ("R", Key::R), ("S", Key::S), ("Z", Key::Z)] {
            let entry = catalog().into_iter().find(|c| c.shortcut == text).unwrap();
            assert_eq!(lookup(key, plain(), EDIT), Some(entry.action));
        }
    }
}

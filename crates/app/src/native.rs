//! Native file dialogs (`rfd`) on Windows and macOS. Elsewhere, and in unit
//! tests, [`pick`] reports [`Pick::Unavailable`] and the UI falls back to its
//! typed-path dialog.

use std::path::{Path, PathBuf};

use crate::state::FileDialogKind;

/// Outcome of a native dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Chosen(PathBuf),
    Cancelled,
    /// No native dialog on this platform: use the typed-path window.
    Unavailable,
}

#[cfg(all(any(windows, target_os = "macos"), not(test)))]
pub fn pick(kind: FileDialogKind, current: Option<&Path>) -> Pick {
    let mut dlg = rfd::FileDialog::new().set_title(kind.title());
    if let Some(dir) = current.and_then(Path::parent).filter(|d| d.is_dir()) {
        dlg = dlg.set_directory(dir);
    }
    let chosen = match kind {
        FileDialogKind::Open => dlg
            .add_filter("forge3d project", &["f3d"])
            .add_filter("All files", &["*"])
            .pick_file(),
        FileDialogKind::SaveAs => dlg
            .add_filter("forge3d project", &["f3d"])
            .set_file_name("scene.f3d")
            .save_file(),
        FileDialogKind::Import => dlg
            .add_filter("Models", &["gltf", "glb", "obj", "stl", "f3d"])
            .add_filter("All files", &["*"])
            .pick_file(),
        FileDialogKind::Export => dlg
            .add_filter("glTF binary", &["glb"])
            .add_filter("glTF", &["gltf"])
            .add_filter("Wavefront OBJ", &["obj"])
            .add_filter("STL", &["stl"])
            .set_file_name("export.glb")
            .save_file(),
    };
    chosen.map_or(Pick::Cancelled, Pick::Chosen)
}

#[cfg(not(all(any(windows, target_os = "macos"), not(test))))]
pub fn pick(_kind: FileDialogKind, _current: Option<&Path>) -> Pick {
    Pick::Unavailable
}

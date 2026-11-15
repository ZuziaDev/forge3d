//! # forge3d-io
//!
//! File import/export for forge3d:
//!
//! | format | import | export | notes |
//! |---|---|---|---|
//! | glTF 2.0 `.gltf` / `.glb` | yes | yes | node hierarchy, TRS/matrix transforms, PBR metallic-roughness materials; Y-up in the file, Z-up in forge3d |
//! | Wavefront `.obj` (+ `.mtl`) | yes | yes | n-gons preserved, named objects, world-space geometry |
//! | STL (binary + ASCII) | yes | yes | merged world-space solid on export, vertices welded on import |
//! | `.f3d` project | yes | yes | full [`DocumentState`](forge3d_core::document::DocumentState) as versioned JSON |
//!
//! Importing is a two step process so the caller controls how the content
//! enters the document: [`import_file`] / [`import_bytes`] parse a file into an
//! [`Imported`] sub-scene, which can then be merged directly
//! ([`import_into`]) or turned into a single undoable command
//! ([`import_command`]) that goes through `Document::execute`.
//!
//! ```
//! use forge3d_core::prelude::*;
//! use forge3d_io::{export_bytes, import_bytes, import_command, ExportFormat, ExportOptions,
//!                  ImportFormat, ImportOptions};
//!
//! let mut doc = Document::new();
//! doc.execute_json(r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#, Origin::Gui)
//!     .unwrap();
//! let glb = export_bytes(&doc.scene, ExportFormat::Glb, &ExportOptions::default()).unwrap();
//! let imported = import_bytes(&glb.files[0].bytes, ImportFormat::Glb, "cube", &ImportOptions::default())
//!     .unwrap();
//! doc.execute(import_command(imported, None), Origin::Gui).unwrap();
//! assert_eq!(doc.scene.len(), 2);
//! doc.undo().unwrap();
//! assert_eq!(doc.scene.len(), 1);
//! ```

#![forbid(unsafe_code)]

mod error;
mod gltf_export;
mod gltf_import;
mod import;
mod obj;
mod project;
mod stl;
mod util;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use forge3d_core::scene::Scene;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use error::{IoError, IoResult};
pub use import::{
    import_command, import_file_command, import_file_into_document, import_into, Imported,
};
pub use project::{
    load_project, project_from_bytes, project_to_bytes, save_project, state_from_bytes,
    state_to_bytes, PROJECT_FORMAT, PROJECT_VERSION,
};

/// Formats a scene can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// glTF 2.0 JSON (`.gltf`), buffer embedded as base64 or as external `.bin`.
    Gltf,
    /// glTF 2.0 binary container (`.glb`).
    Glb,
    /// Wavefront OBJ (`.obj`, plus `.mtl` when materials are used).
    Obj,
    /// STL, binary by default, ASCII with [`ExportOptions::stl_ascii`].
    Stl,
    /// Native project file (`.f3d`).
    F3d,
}

impl ExportFormat {
    /// Detect the format from a file extension (without dot, case-insensitive).
    pub fn from_extension(ext: &str) -> Option<ExportFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "gltf" => Some(ExportFormat::Gltf),
            "glb" => Some(ExportFormat::Glb),
            "obj" => Some(ExportFormat::Obj),
            "stl" => Some(ExportFormat::Stl),
            "f3d" => Some(ExportFormat::F3d),
            _ => None,
        }
    }

    /// Detect the format from a path's extension.
    pub fn from_path(path: &Path) -> IoResult<ExportFormat> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(ExportFormat::from_extension)
            .ok_or_else(|| unsupported(path))
    }

    /// Canonical file extension.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Gltf => "gltf",
            ExportFormat::Glb => "glb",
            ExportFormat::Obj => "obj",
            ExportFormat::Stl => "stl",
            ExportFormat::F3d => "f3d",
        }
    }
}

/// Formats that can be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// glTF 2.0 JSON; also accepts GLB content.
    Gltf,
    /// glTF 2.0 binary container; also accepts JSON content.
    Glb,
    Obj,
    /// Binary or ASCII STL (auto-detected).
    Stl,
    /// Native project file; its scene is merged as a sub-tree.
    F3d,
}

impl ImportFormat {
    /// Detect the format from a file extension (without dot, case-insensitive).
    pub fn from_extension(ext: &str) -> Option<ImportFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "gltf" => Some(ImportFormat::Gltf),
            "glb" => Some(ImportFormat::Glb),
            "obj" => Some(ImportFormat::Obj),
            "stl" => Some(ImportFormat::Stl),
            "f3d" => Some(ImportFormat::F3d),
            _ => None,
        }
    }

    /// Detect the format from a path's extension.
    pub fn from_path(path: &Path) -> IoResult<ImportFormat> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(ImportFormat::from_extension)
            .ok_or_else(|| unsupported(path))
    }
}

fn unsupported(path: &Path) -> IoError {
    IoError::UnsupportedFormat(format!(
        "'{}' has no known extension (expected .gltf, .glb, .obj, .stl or .f3d)",
        path.display()
    ))
}

/// Up axis used by OBJ files. forge3d itself is Z-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    /// Y-up (the common OBJ convention): `(x, y, z)` is stored as `(x, z, -y)`.
    #[default]
    Y,
    /// Z-up: coordinates are stored unchanged.
    Z,
}

/// Options for exporting.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOptions {
    /// Export only these nodes and their descendants (`None` = whole scene).
    pub nodes: Option<Vec<Uuid>>,
    /// Include nodes with `visible == false` (glTF keeps the flag in `extras`).
    pub include_hidden: bool,
    /// glTF/OBJ: write vertex normals (split along creases > 40 degrees).
    pub normals: bool,
    /// glTF: embed the buffer as a base64 data URI (otherwise a sibling `.bin`).
    pub embed_buffers: bool,
    /// Pretty-print JSON outputs (`.gltf`).
    pub pretty_json: bool,
    /// STL: write ASCII instead of binary.
    pub stl_ascii: bool,
    /// OBJ: up axis of the written coordinates.
    pub obj_up_axis: UpAxis,
    /// Base name for the generated files and sidecars (`<name>.bin`,
    /// `<name>.mtl`). [`export_scene`] fills it from the output path.
    pub name: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            nodes: None,
            include_hidden: true,
            normals: true,
            embed_buffers: true,
            pretty_json: false,
            stl_ascii: false,
            obj_up_axis: UpAxis::Y,
            name: None,
        }
    }
}

/// Options for importing.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOptions {
    /// Merge vertices at identical positions so the result is connected
    /// (glTF/STL files store split vertices; OBJ is usually already indexed).
    pub weld: bool,
    /// Folder from which external resources (glTF `.bin`, OBJ `.mtl`) are read.
    /// [`import_file`] sets it to the file's folder.
    pub resource_dir: Option<PathBuf>,
    /// In-memory external resources by relative name; consulted before the folder.
    pub resources: HashMap<String, Vec<u8>>,
    /// OBJ: up axis of the file.
    pub obj_up_axis: UpAxis,
    /// Name for unnamed objects (defaults to the file stem).
    pub name: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            weld: true,
            resource_dir: None,
            resources: HashMap::new(),
            obj_up_axis: UpAxis::Y,
            name: None,
        }
    }
}

/// One file produced by an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    /// File name relative to the output folder (`scene.obj`, `scene.mtl`, ...).
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The files of an export. The first file is the primary one; further files
/// are sidecars referenced by it (`.mtl`, external `.bin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBundle {
    pub files: Vec<ExportedFile>,
}

impl ExportBundle {
    /// The main file.
    pub fn primary(&self) -> &ExportedFile {
        &self.files[0]
    }
}

/// Export `scene` to memory in the given format.
pub fn export_bytes(
    scene: &Scene,
    format: ExportFormat,
    opts: &ExportOptions,
) -> IoResult<ExportBundle> {
    let stem = util::file_stem_safe(opts.name.as_deref().unwrap_or("scene"));
    match format {
        ExportFormat::Gltf => gltf_export::export_gltf(scene, opts, false, &stem),
        ExportFormat::Glb => gltf_export::export_gltf(scene, opts, true, &stem),
        ExportFormat::Obj => obj::export_obj(scene, opts, &stem),
        ExportFormat::Stl => stl::export_stl(scene, opts, &stem),
        ExportFormat::F3d => {
            let planned = util::plan_export(scene, opts)?;
            let sub = if opts.nodes.is_none() && opts.include_hidden {
                scene.clone()
            } else {
                import::sub_scene(scene, &planned)?
            };
            let state = forge3d_core::document::DocumentState {
                scene: sub,
                selection: Default::default(),
            };
            Ok(ExportBundle {
                files: vec![ExportedFile {
                    name: format!("{stem}.f3d"),
                    bytes: state_to_bytes(&state, opts.pretty_json)?,
                }],
            })
        }
    }
}

/// Export `scene` to `path`; the format follows the file extension. Sidecar
/// files (`.mtl`, `.bin`) are written next to it, named after the path's stem.
/// Every file is written atomically. Returns the written paths, primary first.
pub fn export_scene(
    scene: &Scene,
    path: impl AsRef<Path>,
    opts: &ExportOptions,
) -> IoResult<Vec<PathBuf>> {
    let path = path.as_ref();
    export_scene_as(scene, path, ExportFormat::from_path(path)?, opts)
}

/// Like [`export_scene`] with an explicit format.
pub fn export_scene_as(
    scene: &Scene,
    path: impl AsRef<Path>,
    format: ExportFormat,
    opts: &ExportOptions,
) -> IoResult<Vec<PathBuf>> {
    let path = path.as_ref();
    let mut opts = opts.clone();
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        opts.name = Some(stem.to_string());
    }
    let bundle = export_bytes(scene, format, &opts)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let mut written = Vec::new();
    for (i, f) in bundle.files.iter().enumerate() {
        // The primary file goes exactly to `path` (keeping the caller's extension spelling).
        let target = if i == 0 {
            path.to_path_buf()
        } else {
            dir.map_or_else(|| PathBuf::from(&f.name), |d| d.join(&f.name))
        };
        util::write_atomic(&target, &f.bytes)?;
        written.push(target);
    }
    Ok(written)
}

/// Parse file content of a known format into an [`Imported`] sub-scene.
/// `name` labels the import (usually the file stem) and names unnamed objects.
pub fn import_bytes(
    bytes: &[u8],
    format: ImportFormat,
    name: &str,
    opts: &ImportOptions,
) -> IoResult<Imported> {
    match format {
        ImportFormat::Gltf | ImportFormat::Glb => gltf_import::import_gltf(bytes, opts, name),
        ImportFormat::Obj => obj::import_obj(bytes, opts, name),
        ImportFormat::Stl => stl::import_stl(bytes, opts, name),
        ImportFormat::F3d => {
            let state = state_from_bytes(bytes)?;
            if state.scene.is_empty() {
                return Err(IoError::NothingToImport(
                    "the project contains no objects".into(),
                ));
            }
            Ok(Imported::from_scene(
                &state.scene,
                name.to_string(),
                Vec::new(),
            ))
        }
    }
}

/// Read and parse a file; the format follows the extension and external
/// resources are looked up next to the file.
pub fn import_file(path: impl AsRef<Path>, opts: &ImportOptions) -> IoResult<Imported> {
    let path = path.as_ref();
    import_file_as(path, ImportFormat::from_path(path)?, opts)
}

/// Like [`import_file`] with an explicit format.
pub fn import_file_as(
    path: impl AsRef<Path>,
    format: ImportFormat,
    opts: &ImportOptions,
) -> IoResult<Imported> {
    let path = path.as_ref();
    let bytes = util::read_file(path)?;
    let mut opts = opts.clone();
    if opts.resource_dir.is_none() {
        opts.resource_dir = path
            .parent()
            .map(Path::to_path_buf)
            .filter(|d| !d.as_os_str().is_empty())
            .or_else(|| Some(PathBuf::from(".")));
    }
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Import")
        .to_string();
    import_bytes(&bytes, format, &name, &opts)
}

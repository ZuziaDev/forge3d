//! Error type of the io crate.

use std::path::PathBuf;

use forge3d_core::error::{CoreError, ErrorReport};

/// Result alias used throughout the crate.
pub type IoResult<T> = Result<T, IoError>;

/// All errors produced by `forge3d-io`.
///
/// Like [`CoreError`], every variant has a stable machine-readable
/// [`IoError::code`] and a corrective [`IoError::hint`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IoError {
    /// The file extension / format name is not one of the supported formats.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// The input file does not exist.
    #[error("file not found: {}", .0.display())]
    FileNotFound(PathBuf),
    /// Reading or writing a file failed.
    #[error("i/o error on {}: {message}", path.display())]
    Io { path: PathBuf, message: String },
    /// The file content is malformed for its format.
    #[error("{format} parse error{}: {message}", line.map(|l| format!(" at line {l}")).unwrap_or_default())]
    Parse {
        format: &'static str,
        message: String,
        line: Option<usize>,
    },
    /// The content parses but cannot be represented (bad geometry, missing data).
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// The file contains nothing that can be imported.
    #[error("nothing to import: {0}")]
    NothingToImport(String),
    /// The scene contains nothing that the target format can store.
    #[error("nothing to export: {0}")]
    NothingToExport(String),
    /// A `.f3d` file is not a forge3d project.
    #[error("not a forge3d project: {0}")]
    InvalidProject(String),
    /// A `.f3d` file has a version this build cannot read.
    #[error("unsupported project version {found} (this build reads versions 1..={supported})")]
    VersionMismatch { found: String, supported: u32 },
    /// An error from the core library (mesh construction, scene edits, commands).
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl IoError {
    pub(crate) fn parse(format: &'static str, message: impl Into<String>) -> IoError {
        IoError::Parse {
            format,
            message: message.into(),
            line: None,
        }
    }

    pub(crate) fn parse_at(
        format: &'static str,
        line: usize,
        message: impl Into<String>,
    ) -> IoError {
        IoError::Parse {
            format,
            message: message.into(),
            line: Some(line),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, e: &std::io::Error) -> IoError {
        let path = path.into();
        if e.kind() == std::io::ErrorKind::NotFound {
            IoError::FileNotFound(path)
        } else {
            IoError::Io {
                path,
                message: e.to_string(),
            }
        }
    }

    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            IoError::UnsupportedFormat(_) => "unsupported_format",
            IoError::FileNotFound(_) => "file_not_found",
            IoError::Io { .. } => "io_error",
            IoError::Parse { .. } => "parse_error",
            IoError::InvalidData(_) => "invalid_data",
            IoError::NothingToImport(_) => "nothing_to_import",
            IoError::NothingToExport(_) => "nothing_to_export",
            IoError::InvalidProject(_) => "invalid_project",
            IoError::VersionMismatch { .. } => "version_mismatch",
            IoError::Core(e) => e.code(),
        }
    }

    /// A corrective suggestion for the caller.
    pub fn hint(&self) -> String {
        match self {
            IoError::UnsupportedFormat(_) => {
                "Use one of the extensions .gltf, .glb, .obj, .stl or .f3d.".into()
            }
            IoError::FileNotFound(_) => {
                "Check the path; relative paths are resolved against the server's working directory.".into()
            }
            IoError::Io { .. } => {
                "Check that the path is readable/writable and that the disk has free space.".into()
            }
            IoError::Parse { format, .. } => format!(
                "The file is not valid {format}; re-export it from its source application or pick the right format."
            ),
            IoError::InvalidData(_) => {
                "The file parsed but its geometry or references are unusable; inspect or repair it in the source application.".into()
            }
            IoError::NothingToImport(_) => {
                "The file has no polygons or meshes; check that the right file was chosen.".into()
            }
            IoError::NothingToExport(_) => {
                "Create or select at least one object with mesh geometry before exporting (OBJ and STL store meshes only).".into()
            }
            IoError::InvalidProject(_) => {
                "Only files saved by forge3d (.f3d) can be opened as projects; use import for glTF/OBJ/STL.".into()
            }
            IoError::VersionMismatch { .. } => {
                "Open the project with a newer forge3d, or re-save it from the application that wrote it.".into()
            }
            IoError::Core(e) => e.hint(),
        }
    }

    /// Build a serializable `{code, message, hint}` report.
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code().to_string(),
            message: self.to_string(),
            hint: self.hint(),
        }
    }
}

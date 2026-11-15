//! Confinement of file operations to a workspace directory.
//!
//! Relative paths resolve inside the workspace; absolute paths are accepted
//! only when they lie inside it. Paths are canonicalized (for files that do
//! not exist yet: their deepest existing ancestor) so neither `..` segments
//! nor symlinks can escape.

use std::path::{Component, Path, PathBuf};

use crate::error::{ApiError, ApiResult};

/// The directory all file tools are confined to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

/// Whether the resolved file must already exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAccess {
    /// Reading: the file must exist.
    Read,
    /// Writing: the file may not exist yet; missing parent directories are
    /// created inside the workspace.
    Write,
}

impl Workspace {
    /// Use `root` (which must be an existing directory) as workspace.
    pub fn new(root: impl AsRef<Path>) -> ApiResult<Workspace> {
        let root = root.as_ref();
        let canon = root.canonicalize().map_err(|e| {
            ApiError::new(
                "invalid_workspace",
                format!("workspace '{}' is not accessible: {e}", root.display()),
                "Pass an existing directory with --workspace or FORGE3D_WORKSPACE.",
            )
        })?;
        if !canon.is_dir() {
            return Err(ApiError::new(
                "invalid_workspace",
                format!("workspace '{}' is not a directory", root.display()),
                "Pass an existing directory with --workspace or FORGE3D_WORKSPACE.",
            ));
        }
        Ok(Workspace { root: canon })
    }

    /// The current working directory as workspace.
    pub fn current_dir() -> ApiResult<Workspace> {
        let cwd = std::env::current_dir()
            .map_err(|e| ApiError::internal(format!("cannot read the current directory: {e}")))?;
        Workspace::new(cwd)
    }

    /// Canonical workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path relative to the workspace for display (falls back to the full path).
    pub fn display_path(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| p.display().to_string())
    }

    /// Resolve a user-supplied path to a canonical path inside the workspace.
    pub fn resolve(&self, input: &str, access: PathAccess) -> ApiResult<PathBuf> {
        let deny = |why: &str| ApiError::path_not_allowed(input, &self.root, why);
        if input.trim().is_empty() {
            return Err(ApiError::invalid_arguments("`path` must not be empty"));
        }
        if input.contains('\0') {
            return Err(deny("contains a NUL byte"));
        }
        let raw = Path::new(input);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };

        // Split into the deepest existing ancestor and the remaining tail.
        let mut existing = joined.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match std::fs::symlink_metadata(&existing) {
                Ok(_) => break,
                Err(_) => {
                    let name = match existing.file_name() {
                        Some(n) => n.to_os_string(),
                        // `..` (or a root) as the last component of a
                        // not-yet-existing path.
                        None => return Err(deny("'..' segments are not allowed in new paths")),
                    };
                    tail.push(name);
                    if !existing.pop() {
                        return Err(deny("it has no existing ancestor directory"));
                    }
                }
            }
        }
        tail.reverse();
        // `file_name()` is `None` for `..`, so the loop above only pops
        // normal components; reject any remaining `.`/`..` in the tail
        // (they could walk out once the directories are created).
        let tail_path: PathBuf = tail.iter().collect();
        if tail_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(deny("'..' segments are not allowed"));
        }

        let canon_base = existing.canonicalize().map_err(|_| {
            // Exists as a link but cannot be resolved: a dangling symlink.
            deny("it is a symlink whose target does not exist")
        })?;
        // `join("")` would append a trailing separator.
        let resolved = if tail.is_empty() {
            canon_base
        } else {
            canon_base.join(&tail_path)
        };
        if !resolved.starts_with(&self.root) {
            return Err(deny("it resolves to a location outside the workspace"));
        }
        match access {
            PathAccess::Read => {
                if !tail.is_empty() {
                    return Err(ApiError::new(
                        "file_not_found",
                        format!("file not found: {input}"),
                        format!(
                            "Paths are resolved inside the workspace '{}'; check the spelling (relative paths are recommended).",
                            self.root.display()
                        ),
                    ));
                }
                if resolved.is_dir() {
                    return Err(ApiError::invalid_arguments(format!(
                        "'{input}' is a directory, expected a file"
                    )));
                }
            }
            PathAccess::Write => {
                if resolved == self.root || resolved.is_dir() {
                    return Err(ApiError::invalid_arguments(format!(
                        "'{input}' is a directory, expected a file path"
                    )));
                }
                if let Some(parent) = resolved.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            ApiError::new(
                                "io_error",
                                format!("cannot create directory '{}': {e}", parent.display()),
                                "Check the workspace permissions.",
                            )
                        })?;
                        // Re-check after creation (defends against races with
                        // symlinks created concurrently).
                        let p = parent.canonicalize().map_err(|e| {
                            ApiError::internal(format!(
                                "cannot resolve '{}': {e}",
                                parent.display()
                            ))
                        })?;
                        if !p.starts_with(&self.root) {
                            return Err(deny("it resolves to a location outside the workspace"));
                        }
                    }
                }
            }
        }
        Ok(resolved)
    }
}

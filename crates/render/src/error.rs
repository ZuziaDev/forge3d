//! Error type of the render crate.
//!
//! Like [`forge3d_core::error::CoreError`], every variant maps to a stable
//! machine-readable [`RenderError::code`] and a corrective
//! [`RenderError::hint`], so API layers can surface `{code, message, hint}`.

use forge3d_core::error::ErrorReport;

/// Result alias of the render crate.
pub type RenderResult<T> = Result<T, RenderError>;

/// All errors produced by `forge3d-render`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderError {
    /// A render option is out of range.
    #[error("invalid render option `{name}`: {reason}")]
    InvalidOptions { name: String, reason: String },
    /// The camera is degenerate (e.g. eye == target, non-finite values).
    #[error("invalid camera: {0}")]
    InvalidCamera(String),
    /// A node referenced by the request does not exist or has no mesh.
    #[error("node {0} not found or has no renderable mesh")]
    NodeNotFound(uuid::Uuid),
    /// No GPU adapter is available on this machine.
    #[error("no GPU adapter available: {0}")]
    NoAdapter(String),
    /// A GPU operation failed (validation error, device lost, readback failure).
    #[error("GPU error: {0}")]
    Gpu(String),
    /// The render target / texture format is not supported by this operation.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// PNG encoding failed.
    #[error("PNG encoding failed: {0}")]
    Png(String),
}

impl RenderError {
    /// Convenience constructor for [`RenderError::InvalidOptions`].
    pub fn option(name: impl Into<String>, reason: impl Into<String>) -> Self {
        RenderError::InvalidOptions {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            RenderError::InvalidOptions { .. } => "invalid_render_options",
            RenderError::InvalidCamera(_) => "invalid_camera",
            RenderError::NodeNotFound(_) => "node_not_found",
            RenderError::NoAdapter(_) => "no_gpu_adapter",
            RenderError::Gpu(_) => "gpu_error",
            RenderError::UnsupportedFormat(_) => "unsupported_format",
            RenderError::Png(_) => "png_encode_failed",
        }
    }

    /// A corrective suggestion for the caller.
    pub fn hint(&self) -> String {
        match self {
            RenderError::InvalidOptions { name, .. } => format!(
                "Adjust `{name}`: width/height must be 1..=8192, supersample 1..=4 and colors finite values in 0..=1."
            ),
            RenderError::InvalidCamera(_) => {
                "Give the camera distinct eye and target points, a non-zero up vector not parallel to the view direction, fov_y_deg in (0, 180) and 0 < near < far; or use the `auto` camera to frame the scene.".into()
            }
            RenderError::NodeNotFound(_) => {
                "List the scene objects to obtain valid node ids of objects that carry a mesh.".into()
            }
            RenderError::NoAdapter(_) => {
                "Use the software renderer (render_png), which needs no GPU.".into()
            }
            RenderError::Gpu(_) => {
                "Retry, or fall back to the software renderer (render_png).".into()
            }
            RenderError::UnsupportedFormat(_) => {
                "Create the renderer with an 8-bit RGBA/BGRA color format (e.g. Rgba8UnormSrgb) for image readback.".into()
            }
            RenderError::Png(_) => "Use a smaller image size and retry.".into(),
        }
    }

    /// Serializable `{code, message, hint}` report.
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code().to_string(),
            message: self.to_string(),
            hint: self.hint(),
        }
    }
}

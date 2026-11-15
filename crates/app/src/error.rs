//! Application error type.

use forge3d_core::error::CoreError;
use forge3d_io::IoError;
use forge3d_render::RenderError;

/// Everything that can go wrong in the application layer.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A command was rejected by the core.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// File import / export / project I/O failed.
    #[error(transparent)]
    Io(#[from] IoError),
    /// The renderer reported an error.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The requested action does not apply to the current state.
    #[error("{0}")]
    Usage(String),
    /// No usable GPU adapter / surface could be created.
    #[error("graphics initialisation failed: {0}")]
    Graphics(String),
    /// Window or event-loop creation failed.
    #[error("window system error: {0}")]
    Window(String),
    /// Screenshot capture or encoding failed.
    #[error("screenshot failed: {0}")]
    Screenshot(String),
    /// A startup script line could not be parsed or executed.
    #[error("script error: {0}")]
    Script(String),
}

impl AppError {
    /// Stable short code (the core's codes are passed through).
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Core(e) => e.code(),
            AppError::Io(e) => e.code(),
            AppError::Render(_) => "render_error",
            AppError::Usage(_) => "invalid_action",
            AppError::Graphics(_) => "graphics_error",
            AppError::Window(_) => "window_error",
            AppError::Screenshot(_) => "screenshot_error",
            AppError::Script(_) => "script_error",
        }
    }

    /// A corrective suggestion for the user.
    pub fn hint(&self) -> String {
        match self {
            AppError::Core(e) => e.hint(),
            AppError::Io(e) => e.hint(),
            AppError::Render(_) => "Check the GPU driver; the software rasterizer is a fallback for images.".into(),
            AppError::Usage(_) => "Adjust the selection or the tool parameters and try again.".into(),
            AppError::Graphics(_) => {
                "Install a Vulkan/Metal/DX12 driver (on Linux e.g. mesa-vulkan-drivers for a software adapter).".into()
            }
            AppError::Window(_) => "Make sure a display is available (on a headless Linux box use xvfb-run).".into(),
            AppError::Screenshot(_) => "Check that the output path is writable.".into(),
            AppError::Script(_) => "Each script line must be one CommandSpec JSON object; '#' starts a comment.".into(),
        }
    }
}

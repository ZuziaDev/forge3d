//! Render options and the fixed viewport style shared by both renderers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{RenderError, RenderResult};

/// Largest accepted image side in pixels.
pub const MAX_IMAGE_SIZE: u32 = 8192;
/// Largest accepted supersampling factor per axis.
pub const MAX_SUPERSAMPLE: u32 = 4;

fn d_true() -> bool {
    true
}
fn d_width() -> u32 {
    800
}
fn d_height() -> u32 {
    600
}
fn d_background() -> [f32; 4] {
    [0.20, 0.21, 0.23, 1.0]
}
fn d_supersample() -> u32 {
    2
}

/// What to draw on top of the shaded scene (shared by the software
/// renderer and the GPU viewport).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ViewOptions {
    /// Draw all mesh edges as a wireframe overlay.
    #[serde(default)]
    pub wireframe: bool,
    /// Draw the ground grid (z = 0) with X (red) and Y (green) axis lines.
    #[serde(default = "d_true")]
    pub grid: bool,
    /// Highlight the selection (object outlines, selected vertices/edges/faces in edit modes).
    #[serde(default = "d_true")]
    pub show_selection: bool,
    /// Background color, sRGB RGBA in 0..=1 (alpha < 1 gives a transparent PNG).
    #[serde(default = "d_background")]
    pub background: [f32; 4],
    /// Interpolate vertex normals instead of flat per-face shading.
    #[serde(default)]
    pub smooth_shading: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            wireframe: false,
            grid: true,
            show_selection: true,
            background: d_background(),
            smooth_shading: false,
        }
    }
}

impl ViewOptions {
    /// Check value ranges.
    pub fn validate(&self) -> RenderResult<()> {
        if !self
            .background
            .iter()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
        {
            return Err(RenderError::option(
                "background",
                "components must be in 0..=1",
            ));
        }
        Ok(())
    }
}

/// Options of an offscreen render (image size, anti-aliasing and view options).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderOptions {
    /// Image width in pixels (1..=8192).
    #[serde(default = "d_width")]
    pub width: u32,
    /// Image height in pixels (1..=8192).
    #[serde(default = "d_height")]
    pub height: u32,
    /// Supersampling factor per axis (1 = off, 2 = 4 samples per pixel, max 4).
    #[serde(default = "d_supersample", alias = "samples")]
    pub supersample: u32,
    #[serde(flatten)]
    pub view: ViewOptions,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            width: d_width(),
            height: d_height(),
            supersample: d_supersample(),
            view: ViewOptions::default(),
        }
    }
}

impl RenderOptions {
    /// Options with a given size and defaults otherwise.
    pub fn with_size(width: u32, height: u32) -> Self {
        RenderOptions {
            width,
            height,
            ..RenderOptions::default()
        }
    }

    /// Check value ranges.
    pub fn validate(&self) -> RenderResult<()> {
        if self.width == 0 || self.width > MAX_IMAGE_SIZE {
            return Err(RenderError::option("width", "must be in 1..=8192"));
        }
        if self.height == 0 || self.height > MAX_IMAGE_SIZE {
            return Err(RenderError::option("height", "must be in 1..=8192"));
        }
        if self.supersample == 0 || self.supersample > MAX_SUPERSAMPLE {
            return Err(RenderError::option("supersample", "must be in 1..=4"));
        }
        let samples = self.width as u64
            * self.height as u64
            * (self.supersample as u64 * self.supersample as u64);
        if samples > 8192 * 8192 {
            return Err(RenderError::option(
                "supersample",
                "width × height × supersample² must not exceed 67 million samples",
            ));
        }
        self.view.validate()
    }

    /// Aspect ratio width / height.
    pub fn aspect(&self) -> f32 {
        self.width.max(1) as f32 / self.height.max(1) as f32
    }
}

/// Fixed viewport colors (linear RGB(A)) and sizes, shared by both renderers
/// so their output matches.
pub mod style {
    /// Wireframe overlay color (object mode) with alpha.
    pub const WIRE: [f32; 4] = [0.0, 0.0, 0.0, 0.55];
    /// Wireframe of objects in an edit mode.
    pub const WIRE_EDIT: [f32; 4] = [0.0, 0.0, 0.0, 0.85];
    /// Selected edges / vertices in edit modes.
    pub const ELEMENT_SELECTED: [f32; 4] = [1.0, 0.32, 0.02, 1.0];
    /// Unselected vertex points in vertex mode.
    pub const VERTEX: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    /// Selected face tint color and amount.
    pub const FACE_SELECTED: [f32; 4] = [1.0, 0.32, 0.02, 0.35];
    /// Outline of selected (non-active) objects.
    pub const OUTLINE_SELECTED: [f32; 4] = [0.85, 0.22, 0.01, 1.0];
    /// Outline of the active object.
    pub const OUTLINE_ACTIVE: [f32; 4] = [1.0, 0.6, 0.12, 1.0];
    /// Tint applied to selected objects in object mode (rgb, amount).
    pub const OBJECT_TINT: [f32; 4] = [1.0, 0.45, 0.1, 0.06];
    /// Grid minor line color and opacity.
    pub const GRID_MINOR: [f32; 4] = [0.08, 0.08, 0.09, 0.45];
    /// Grid major line color and opacity.
    pub const GRID_MAJOR: [f32; 4] = [0.11, 0.11, 0.12, 0.7];
    /// X axis line (y = 0).
    pub const AXIS_X: [f32; 4] = [0.75, 0.08, 0.08, 0.9];
    /// Y axis line (x = 0).
    pub const AXIS_Y: [f32; 4] = [0.2, 0.6, 0.05, 0.9];
    /// Grid major line every N minor cells.
    pub const GRID_MAJOR_EVERY: f32 = 10.0;
    /// Wireframe line width in pixels.
    pub const WIRE_WIDTH: f32 = 1.0;
    /// Selected edge line width in pixels.
    pub const SELECTED_EDGE_WIDTH: f32 = 2.5;
    /// Vertex point size in pixels.
    pub const VERTEX_SIZE: f32 = 4.0;
    /// Selected vertex point size in pixels.
    pub const SELECTED_VERTEX_SIZE: f32 = 6.0;
    /// Outline width in pixels.
    pub const OUTLINE_WIDTH: f32 = 2.0;
    /// Overlay lines/points are pulled towards the camera by this fraction of
    /// their view depth to win the depth test against their own faces.
    pub const LINE_DEPTH_BIAS: f32 = 0.002;
    /// Grid fade starts at this multiple of the camera distance (and ends at 4×).
    pub const GRID_FADE_START: f32 = 4.0;
    pub const GRID_FADE_END: f32 = 16.0;
}

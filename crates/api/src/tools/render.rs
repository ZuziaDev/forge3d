//! `viewport_render`: software-rendered PNG of the scene so the model can
//! see its work.

use base64::Engine;
use forge3d_render::{Camera, CameraSpec, OrbitCamera, RenderError, RenderOptions, ViewPreset};
use glam::Vec3;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{blocking, Forge3dTools, ToolOutput};
use crate::error::{ApiError, ApiResult};
use crate::info::{resolve_ref, ObjectRef};
use crate::tools::plan::r3;

/// Largest image side accepted by the API.
pub const MAX_RENDER_SIZE: u32 = 2048;

impl From<RenderError> for ApiError {
    fn from(e: RenderError) -> Self {
        let hint = match &e {
            RenderError::NodeNotFound(_) => {
                "Call scene_get for valid object ids/names; `focus` objects must exist.".to_string()
            }
            other => other.hint(),
        };
        ApiError::new(e.code(), e.to_string(), hint)
    }
}

fn d_fov() -> f32 {
    50.0
}
fn d_yaw() -> f32 {
    -60.0
}
fn d_pitch() -> f32 {
    25.0
}
fn d_up() -> [f32; 3] {
    [0.0, 0.0, 1.0]
}
fn d_distance() -> f32 {
    10.0
}
fn d_width() -> u32 {
    768
}
fn d_height() -> u32 {
    576
}
fn d_true() -> bool {
    true
}
fn d_supersample() -> u32 {
    2
}

/// Named view directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewName {
    /// From −Y looking towards +Y.
    Front,
    /// From +Y.
    Back,
    /// From +X.
    Right,
    /// From −X.
    Left,
    /// From above.
    Top,
    /// From below.
    Bottom,
    /// Three-quarter view from front-right-above (default).
    Iso,
}

impl From<ViewName> for ViewPreset {
    fn from(v: ViewName) -> Self {
        match v {
            ViewName::Front => ViewPreset::Front,
            ViewName::Back => ViewPreset::Back,
            ViewName::Right => ViewPreset::Right,
            ViewName::Left => ViewPreset::Left,
            ViewName::Top => ViewPreset::Top,
            ViewName::Bottom => ViewPreset::Bottom,
            ViewName::Iso => ViewPreset::Iso,
        }
    }
}

/// Camera for `viewport_render`, tagged by `kind`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CameraParams {
    /// Frame the whole scene (or the `focus` objects) automatically.
    Auto {
        /// Named view; overrides yaw/pitch.
        #[serde(default)]
        view: Option<ViewName>,
        /// Rotation around Z in degrees (0 = from +X, −90 = from front/−Y). Default −60.
        #[serde(default = "d_yaw")]
        yaw_deg: f32,
        /// Elevation in degrees (90 = from above). Default 25.
        #[serde(default = "d_pitch")]
        pitch_deg: f32,
        #[serde(default = "d_fov")]
        fov_y_deg: f32,
        #[serde(default)]
        orthographic: bool,
        /// Objects (ids or unique names) to frame; default all visible objects.
        #[serde(default)]
        focus: Vec<ObjectRef>,
    },
    /// Orbit around `target` at `distance`.
    Orbit {
        #[serde(default)]
        target: [f32; 3],
        #[serde(default = "d_distance")]
        distance: f32,
        #[serde(default = "d_yaw")]
        yaw_deg: f32,
        #[serde(default = "d_pitch")]
        pitch_deg: f32,
        #[serde(default = "d_fov")]
        fov_y_deg: f32,
        #[serde(default)]
        orthographic: bool,
    },
    /// Explicit eye position looking at `target`.
    LookAt {
        eye: [f32; 3],
        target: [f32; 3],
        /// Up vector, default [0,0,1].
        #[serde(default = "d_up")]
        up: [f32; 3],
        #[serde(default = "d_fov")]
        fov_y_deg: f32,
    },
}

/// Parameters of `viewport_render`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewportRenderParams {
    /// Camera; default `{"kind":"auto"}` (iso view framing everything).
    #[serde(default)]
    pub camera: Option<CameraParams>,
    /// Image width in pixels (16..=2048, default 768).
    #[serde(default = "d_width")]
    pub width: u32,
    /// Image height in pixels (16..=2048, default 576).
    #[serde(default = "d_height")]
    pub height: u32,
    /// Overlay all mesh edges.
    #[serde(default)]
    pub wireframe: bool,
    /// Draw the ground grid with X (red) / Y (green) axes (default true).
    #[serde(default = "d_true")]
    pub grid: bool,
    /// Highlight selected objects/elements (default true).
    #[serde(default = "d_true")]
    pub show_selection: bool,
    /// Smooth (vertex-normal) shading instead of flat faces.
    #[serde(default)]
    pub smooth_shading: bool,
    /// Anti-aliasing supersampling factor 1..=4 (default 2).
    #[serde(default = "d_supersample")]
    pub supersample: u32,
}

/// A rendered image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RenderedImage {
    /// Base64 of the image bytes.
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }

    /// JSON form with base64 data.
    pub fn to_json(&self) -> Value {
        json!({
            "mime_type": self.mime_type,
            "width": self.width,
            "height": self.height,
            "data_base64": self.base64(),
        })
    }
}

fn check_size(name: &str, v: u32) -> ApiResult<()> {
    if (16..=MAX_RENDER_SIZE).contains(&v) {
        Ok(())
    } else {
        Err(ApiError::invalid_arguments(format!(
            "`{name}` must be in 16..={MAX_RENDER_SIZE} (got {v})"
        )))
    }
}

pub(crate) async fn viewport_render(
    tools: &Forge3dTools,
    p: ViewportRenderParams,
) -> ApiResult<ToolOutput> {
    check_size("width", p.width)?;
    check_size("height", p.height)?;
    if !(1..=4).contains(&p.supersample) {
        return Err(ApiError::invalid_arguments(
            "`supersample` must be in 1..=4",
        ));
    }
    let camera = p.camera.clone().unwrap_or(CameraParams::Auto {
        view: None,
        yaw_deg: d_yaw(),
        pitch_deg: d_pitch(),
        fov_y_deg: d_fov(),
        orthographic: false,
        focus: Vec::new(),
    });
    // Resolve object references and snapshot the scene on the document
    // thread; rasterize on a blocking worker so the owner (e.g. the GUI) is
    // not stalled.
    let (scene, selection, spec, visible) = tools
        .handle()
        .run(move |doc| {
            let spec = match camera {
                CameraParams::Auto {
                    view,
                    yaw_deg,
                    pitch_deg,
                    fov_y_deg,
                    orthographic,
                    focus,
                } => CameraSpec::Auto {
                    view: view.map(ViewPreset::from),
                    yaw_deg,
                    pitch_deg,
                    fov_y_deg,
                    orthographic,
                    focus: focus
                        .iter()
                        .map(|r| resolve_ref(&doc.scene, r))
                        .collect::<ApiResult<Vec<_>>>()?,
                },
                CameraParams::Orbit {
                    target,
                    distance,
                    yaw_deg,
                    pitch_deg,
                    fov_y_deg,
                    orthographic,
                } => {
                    let mut o =
                        OrbitCamera::new(Vec3::from_array(target), distance, yaw_deg, pitch_deg);
                    o.fov_y_deg = fov_y_deg;
                    o.orthographic = orthographic;
                    CameraSpec::Orbit(o)
                }
                CameraParams::LookAt {
                    eye,
                    target,
                    up,
                    fov_y_deg,
                } => CameraSpec::LookAt(Camera::look_at(
                    Vec3::from_array(eye),
                    Vec3::from_array(target),
                    Vec3::from_array(up),
                    fov_y_deg,
                )),
            };
            let visible = doc
                .scene
                .nodes()
                .filter(|n| n.visible && n.mesh.is_some())
                .count();
            Ok::<_, ApiError>((doc.scene.clone(), doc.selection.clone(), spec, visible))
        })
        .await??;

    let mut options = RenderOptions::with_size(p.width, p.height);
    options.supersample = p.supersample;
    options.view.wireframe = p.wireframe;
    options.view.grid = p.grid;
    options.view.show_selection = p.show_selection;
    options.view.smooth_shading = p.smooth_shading;

    let (png, cam) = blocking(move || {
        options.validate()?;
        let cam = spec.resolve(&scene, options.aspect())?;
        let png = forge3d_render::render_png(
            &scene,
            Some(&selection),
            &CameraSpec::LookAt(cam),
            &options,
        )?;
        Ok((png, cam))
    })
    .await?;

    let description = format!(
        "Rendered {}x{} PNG of {} visible mesh object{} from eye {:?} looking at {:?} (Z up; grid {}, wireframe {}).",
        p.width,
        p.height,
        visible,
        if visible == 1 { "" } else { "s" },
        r3(cam.eye),
        r3(cam.target),
        if p.grid { "on" } else { "off" },
        if p.wireframe { "on" } else { "off" },
    );
    Ok(ToolOutput {
        value: json!({
            "description": description,
            "width": p.width,
            "height": p.height,
            "camera": {
                "eye": r3(cam.eye),
                "target": r3(cam.target),
                "up": r3(cam.up),
            },
            "visible_mesh_objects": visible,
        }),
        images: vec![RenderedImage {
            mime_type: "image/png".into(),
            width: p.width,
            height: p.height,
            data: png,
        }],
    })
}

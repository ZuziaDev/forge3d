//! Cameras (Z-up): an explicit look-at [`Camera`], an interactive
//! [`OrbitCamera`] and the serializable [`CameraSpec`] used by API layers.
//!
//! Matrices follow wgpu conventions: right-handed view space, clip-space depth
//! in `0..=1`, NDC `y` up. Screen/pixel coordinates have their origin at the
//! top-left corner with `y` pointing down.

use forge3d_core::math::Aabb;
use forge3d_core::scene::Scene;
use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{RenderError, RenderResult};

fn d_fov() -> f32 {
    45.0
}
fn d_up() -> Vec3 {
    Vec3::Z
}
fn d_near() -> f32 {
    0.01
}
fn d_far() -> f32 {
    1000.0
}
fn d_ortho_height() -> f32 {
    10.0
}

/// Smallest allowed orbit distance.
pub const MIN_DISTANCE: f32 = 1e-4;
/// Largest allowed orbit distance.
pub const MAX_DISTANCE: f32 = 1e7;
/// Pitch is clamped to ±this many degrees to keep the up vector well defined.
pub const MAX_PITCH_DEG: f32 = 89.9;

/// Camera projection.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Projection {
    /// Perspective projection with a vertical field of view in degrees.
    Perspective {
        #[serde(default = "d_fov")]
        fov_y_deg: f32,
    },
    /// Orthographic projection; `height` is the visible world-space height.
    Orthographic {
        #[serde(default = "d_ortho_height")]
        height: f32,
    },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Perspective { fov_y_deg: d_fov() }
    }
}

/// A ray with origin and unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// Point at parameter `t`.
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }

    /// Intersection parameter with the plane `z = 0` (`None` when parallel or behind).
    pub fn intersect_ground(&self) -> Option<f32> {
        if self.dir.z.abs() < 1e-12 {
            return None;
        }
        let t = -self.origin.z / self.dir.z;
        (t > 0.0).then_some(t)
    }
}

/// An explicit camera: eye, target, up, projection and clip planes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Camera {
    /// Camera position (world space).
    #[schemars(with = "[f32; 3]")]
    pub eye: Vec3,
    /// Point the camera looks at.
    #[schemars(with = "[f32; 3]")]
    pub target: Vec3,
    /// Up direction (default +Z); must not be parallel to the view direction.
    #[serde(default = "d_up")]
    #[schemars(with = "[f32; 3]")]
    pub up: Vec3,
    #[serde(default)]
    pub projection: Projection,
    /// Near clip distance (> 0).
    #[serde(default = "d_near")]
    pub near: f32,
    /// Far clip distance (> near).
    #[serde(default = "d_far")]
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        OrbitCamera::default().to_camera()
    }
}

/// Automatic clip planes for a camera at `distance` from its target.
fn auto_clip(distance: f32) -> (f32, f32) {
    let d = distance.max(MIN_DISTANCE);
    ((d * 0.002).max(1e-5), d * 200.0 + 10.0)
}

impl Camera {
    /// Perspective camera looking from `eye` at `target`; clip planes are
    /// chosen from the eye–target distance.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3, fov_y_deg: f32) -> Camera {
        let (near, far) = auto_clip(eye.distance(target));
        Camera {
            eye,
            target,
            up,
            projection: Projection::Perspective { fov_y_deg },
            near,
            far,
        }
    }

    /// Orthographic camera showing `height` world units vertically.
    pub fn orthographic(eye: Vec3, target: Vec3, up: Vec3, height: f32) -> Camera {
        let dist = eye.distance(target);
        let (_, far) = auto_clip(dist.max(height));
        Camera {
            eye,
            target,
            up,
            projection: Projection::Orthographic { height },
            near: 0.0,
            far: far + dist,
        }
    }

    /// Check for a usable camera.
    pub fn validate(&self) -> RenderResult<()> {
        let finite = self.eye.is_finite()
            && self.target.is_finite()
            && self.up.is_finite()
            && self.near.is_finite()
            && self.far.is_finite();
        if !finite {
            return Err(RenderError::InvalidCamera("non-finite value".into()));
        }
        let dir = self.target - self.eye;
        if dir.length() < 1e-9 {
            return Err(RenderError::InvalidCamera("eye equals target".into()));
        }
        if self.up.length() < 1e-9 {
            return Err(RenderError::InvalidCamera("up vector is zero".into()));
        }
        match self.projection {
            Projection::Perspective { fov_y_deg } => {
                if !(fov_y_deg > 0.0 && fov_y_deg < 180.0) {
                    return Err(RenderError::InvalidCamera(format!(
                        "fov_y_deg {fov_y_deg} must be in (0, 180)"
                    )));
                }
                if !(self.near > 0.0) {
                    return Err(RenderError::InvalidCamera(
                        "perspective near must be > 0".into(),
                    ));
                }
            }
            Projection::Orthographic { height } => {
                if !(height > 0.0 && height.is_finite()) {
                    return Err(RenderError::InvalidCamera(
                        "orthographic height must be > 0".into(),
                    ));
                }
                if self.near < 0.0 {
                    return Err(RenderError::InvalidCamera(
                        "orthographic near must be >= 0".into(),
                    ));
                }
            }
        }
        if !(self.far > self.near) {
            return Err(RenderError::InvalidCamera("far must be > near".into()));
        }
        Ok(())
    }

    /// Whether the projection is orthographic.
    pub fn is_orthographic(&self) -> bool {
        matches!(self.projection, Projection::Orthographic { .. })
    }

    /// Eye–target distance.
    pub fn distance(&self) -> f32 {
        self.eye.distance(self.target)
    }

    /// Unit view direction.
    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye).normalize_or(Vec3::NEG_Y)
    }

    /// Up vector actually used (falls back when `up` is parallel to the view direction).
    fn effective_up(&self) -> Vec3 {
        let f = self.forward();
        let up = self.up.normalize_or(Vec3::Z);
        if f.cross(up).length() > 1e-5 {
            return up;
        }
        // Looking straight along the up axis: pick a stable alternative.
        if f.cross(Vec3::Y).length() > 1e-5 {
            Vec3::Y
        } else {
            Vec3::X
        }
    }

    /// Unit right / up / forward basis of the view.
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(self.effective_up()).normalize();
        let u = r.cross(f);
        (r, u, f)
    }

    /// World → view matrix (right-handed, camera looks down −Z).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye, self.target, self.effective_up())
    }

    /// View → clip matrix for the given aspect ratio (width / height), depth `0..=1`.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        match self.projection {
            Projection::Perspective { fov_y_deg } => {
                Mat4::perspective_rh(fov_y_deg.to_radians(), aspect, self.near, self.far)
            }
            Projection::Orthographic { height } => {
                let h = height * 0.5;
                let w = h * aspect;
                Mat4::orthographic_rh(-w, w, -h, h, self.near, self.far)
            }
        }
    }

    /// World → clip matrix.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }

    /// View-space depth (distance along the view direction) of an NDC depth value.
    pub fn linear_depth(&self, ndc_z: f32) -> f32 {
        match self.projection {
            Projection::Perspective { .. } => {
                // perspective_rh: ndc_z = r*near/d - r with r = far/(near-far).
                let r = self.far / (self.near - self.far);
                r * self.near / (ndc_z + r)
            }
            Projection::Orthographic { .. } => self.near + ndc_z * (self.far - self.near),
        }
    }

    /// Ray through a point in NDC (`x`, `y` in −1..=1, `y` up).
    pub fn ray_from_ndc(&self, ndc: Vec2, aspect: f32) -> Ray {
        let inv = self.view_projection(aspect).inverse();
        let p0 = inv * glam::Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let p1 = inv * glam::Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let a = p0.xyz() / p0.w;
        let b = p1.xyz() / p1.w;
        let origin = if self.is_orthographic() { a } else { self.eye };
        Ray {
            origin,
            dir: (b - a).normalize_or(self.forward()),
        }
    }

    /// Ray through a pixel position (`x`, `y` in pixels, top-left origin; use
    /// `+0.5` for pixel centers) of a `width` × `height` viewport.
    pub fn ray_through_pixel(&self, x: f32, y: f32, width: u32, height: u32) -> Ray {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let ndc = Vec2::new(x / w * 2.0 - 1.0, 1.0 - y / h * 2.0);
        self.ray_from_ndc(ndc, w / h)
    }

    /// Project a world point to pixel coordinates (x, y, ndc depth); `None`
    /// when behind the camera.
    pub fn project_to_pixel(&self, p: Vec3, width: u32, height: u32) -> Option<Vec3> {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let c = self.view_projection(w / h) * p.extend(1.0);
        if c.w <= 1e-12 {
            return None;
        }
        let n = c.xyz() / c.w;
        Some(Vec3::new(
            (n.x * 0.5 + 0.5) * w,
            (0.5 - n.y * 0.5) * h,
            n.z,
        ))
    }

    /// Size of the grid cell (power of ten) suited to this view; shared by
    /// the software and GPU renderers.
    pub fn grid_cell_size(&self) -> f32 {
        let scale = match self.projection {
            Projection::Perspective { .. } => self.distance(),
            Projection::Orthographic { height } => height,
        };
        let s = (scale * 0.25).max(1e-6);
        10f32.powf(s.log10().floor())
    }

    /// Grow the clip range so that `bounds` (and some margin) fits between near and far.
    pub fn fit_clip_to_bounds(&mut self, bounds: &Aabb) {
        let center = bounds.center();
        let radius = (bounds.diagonal() * 0.5).max(1e-3);
        let along = (center - self.eye).dot(self.forward());
        let far_needed = along + radius * 2.0;
        if far_needed > self.far {
            self.far = far_needed;
        }
        if !self.is_orthographic() {
            let near_needed = (along - radius * 2.0).max(self.far * 1e-6);
            if near_needed > 0.0 && near_needed < self.near {
                self.near = near_needed;
            }
        } else if along - radius * 2.0 < self.near {
            // Move the ortho eye back would change the camera; extend near
            // towards (but not past) zero instead.
            self.near = (along - radius * 2.0).max(0.0);
        }
    }
}

/// Interactive orbit camera (Z-up turntable).
///
/// The eye sits at `target + distance * (cos(pitch)·cos(yaw), cos(pitch)·sin(yaw), sin(pitch))`:
/// yaw 0 looks from +X, yaw −90 from −Y (front view), pitch 90 from above.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrbitCamera {
    /// Orbit center.
    #[serde(default)]
    #[schemars(with = "[f32; 3]")]
    pub target: Vec3,
    /// Distance from eye to target (> 0).
    #[serde(default = "d_distance")]
    pub distance: f32,
    /// Rotation around the Z axis in degrees.
    #[serde(default = "d_yaw")]
    pub yaw_deg: f32,
    /// Elevation above the XY plane in degrees (clamped to ±89.9).
    #[serde(default = "d_pitch")]
    pub pitch_deg: f32,
    /// Vertical field of view in degrees (perspective).
    #[serde(default = "d_fov")]
    pub fov_y_deg: f32,
    /// Use an orthographic projection with the same apparent size at the target.
    #[serde(default)]
    pub orthographic: bool,
}

fn d_distance() -> f32 {
    10.0
}
fn d_yaw() -> f32 {
    -60.0
}
fn d_pitch() -> f32 {
    25.0
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera {
            target: Vec3::ZERO,
            distance: d_distance(),
            yaw_deg: d_yaw(),
            pitch_deg: d_pitch(),
            fov_y_deg: d_fov(),
            orthographic: false,
        }
    }
}

/// Unit direction from target to eye for yaw/pitch in degrees.
pub fn orbit_direction(yaw_deg: f32, pitch_deg: f32) -> Vec3 {
    let (y, p) = (
        yaw_deg.to_radians(),
        pitch_deg.clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG).to_radians(),
    );
    Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
}

impl OrbitCamera {
    /// Orbit camera from yaw/pitch around a target.
    pub fn new(target: Vec3, distance: f32, yaw_deg: f32, pitch_deg: f32) -> OrbitCamera {
        OrbitCamera {
            target,
            distance: distance.clamp(MIN_DISTANCE, MAX_DISTANCE),
            yaw_deg,
            pitch_deg: pitch_deg.clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG),
            ..OrbitCamera::default()
        }
    }

    /// Orbit camera reproducing an explicit camera's eye and target.
    pub fn from_camera(cam: &Camera) -> OrbitCamera {
        let d = cam.eye - cam.target;
        let distance = d.length().clamp(MIN_DISTANCE, MAX_DISTANCE);
        let n = d / d.length().max(1e-12);
        let pitch = n.z.clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = n.y.atan2(n.x).to_degrees();
        let (fov, ortho) = match cam.projection {
            Projection::Perspective { fov_y_deg } => (fov_y_deg, false),
            Projection::Orthographic { .. } => (d_fov(), true),
        };
        OrbitCamera {
            target: cam.target,
            distance,
            yaw_deg: yaw,
            pitch_deg: pitch.clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG),
            fov_y_deg: fov,
            orthographic: ortho,
        }
    }

    /// Eye position.
    pub fn eye(&self) -> Vec3 {
        self.target + orbit_direction(self.yaw_deg, self.pitch_deg) * self.distance
    }

    /// World-space height visible at the target distance.
    pub fn view_height(&self) -> f32 {
        2.0 * self.distance * (self.fov_y_deg.to_radians() * 0.5).tan()
    }

    /// Explicit camera for rendering.
    pub fn to_camera(&self) -> Camera {
        let eye = self.eye();
        if self.orthographic {
            // Place the ortho eye far enough back to see everything in front of the target.
            let h = self.view_height();
            let dir = orbit_direction(self.yaw_deg, self.pitch_deg);
            let back = self.distance.max(h) * 4.0;
            let eye = self.target + dir * back;
            let mut c = Camera::orthographic(eye, self.target, Vec3::Z, h);
            c.far = back + self.distance.max(h) * 200.0;
            c
        } else {
            Camera::look_at(eye, self.target, Vec3::Z, self.fov_y_deg)
        }
    }

    /// Rotate by yaw/pitch deltas in degrees.
    pub fn orbit(&mut self, d_yaw_deg: f32, d_pitch_deg: f32) {
        self.yaw_deg = (self.yaw_deg + d_yaw_deg).rem_euclid(360.0);
        if self.yaw_deg > 180.0 {
            self.yaw_deg -= 360.0;
        }
        self.pitch_deg = (self.pitch_deg + d_pitch_deg).clamp(-MAX_PITCH_DEG, MAX_PITCH_DEG);
    }

    /// Move the target in the view plane. `dx`/`dy` are fractions of the
    /// visible height (positive `dx` moves the view content left, i.e. the
    /// camera right; positive `dy` moves the camera up).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (r, u, _) = self.to_camera().basis();
        let h = self.view_height();
        self.target += (r * dx + u * dy) * h;
    }

    /// Pan by a mouse drag in pixels (content follows the cursor).
    pub fn pan_pixels(&mut self, dx_px: f32, dy_px: f32, viewport_height_px: f32) {
        let vh = viewport_height_px.max(1.0);
        self.pan(-dx_px / vh, dy_px / vh);
    }

    /// Multiply the distance by `factor` (< 1 zooms in).
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        }
    }

    /// Center on `bounds` and pick a distance at which it fits the view
    /// (with a small margin) for the given aspect ratio (width / height).
    pub fn frame_bounds(&mut self, bounds: &Aabb, aspect: f32) {
        self.target = bounds.center();
        let radius = (bounds.diagonal() * 0.5).max(1e-3);
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let half_y = (self.fov_y_deg.to_radians() * 0.5).clamp(1e-3, 1.55);
        let half_x = (half_y.tan() * aspect).atan();
        let half = half_y.min(half_x);
        // Bounding sphere fits inside the narrower half-angle.
        let margin = 1.08;
        self.distance = (radius * margin / half.sin()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

/// Named view directions for [`CameraSpec::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewPreset {
    /// From −Y looking towards +Y.
    Front,
    /// From +Y looking towards −Y.
    Back,
    /// From +X looking towards −X.
    Right,
    /// From −X looking towards +X.
    Left,
    /// From above (+Z).
    Top,
    /// From below (−Z).
    Bottom,
    /// Three-quarter view from front-right-above (the default).
    Iso,
}

impl ViewPreset {
    /// (yaw, pitch) in degrees.
    pub fn yaw_pitch(self) -> (f32, f32) {
        match self {
            ViewPreset::Front => (-90.0, 0.0),
            ViewPreset::Back => (90.0, 0.0),
            ViewPreset::Right => (0.0, 0.0),
            ViewPreset::Left => (180.0, 0.0),
            ViewPreset::Top => (-90.0, MAX_PITCH_DEG),
            ViewPreset::Bottom => (-90.0, -MAX_PITCH_DEG),
            ViewPreset::Iso => (d_yaw(), d_pitch()),
        }
    }
}

/// Serializable camera request used by API layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CameraSpec {
    /// Automatically frame the whole scene (or the `focus` nodes) from a view direction.
    Auto {
        /// Named view; overrides `yaw_deg`/`pitch_deg` when given.
        #[serde(default)]
        view: Option<ViewPreset>,
        #[serde(default = "d_yaw")]
        yaw_deg: f32,
        #[serde(default = "d_pitch")]
        pitch_deg: f32,
        #[serde(default = "d_fov")]
        fov_y_deg: f32,
        #[serde(default)]
        orthographic: bool,
        /// Frame only these nodes (and their descendants); empty = all visible meshes.
        #[serde(default)]
        focus: Vec<Uuid>,
    },
    /// An explicit orbit camera.
    Orbit(OrbitCamera),
    /// An explicit eye/target camera.
    LookAt(Camera),
}

impl Default for CameraSpec {
    fn default() -> Self {
        CameraSpec::Auto {
            view: None,
            yaw_deg: d_yaw(),
            pitch_deg: d_pitch(),
            fov_y_deg: d_fov(),
            orthographic: false,
            focus: Vec::new(),
        }
    }
}

/// World bounds of all visible meshes (or of `focus` nodes and their descendants).
pub fn scene_bounds(scene: &Scene, focus: &[Uuid]) -> RenderResult<Option<Aabb>> {
    let ids: Vec<Uuid> = if focus.is_empty() {
        scene.traverse()
    } else {
        let mut v = Vec::new();
        for &f in focus {
            if !scene.contains(f) {
                return Err(RenderError::NodeNotFound(f));
            }
            v.push(f);
            v.extend(scene.descendants(f));
        }
        v
    };
    let mut out: Option<Aabb> = None;
    for id in ids {
        let Some(node) = scene.get(id) else { continue };
        if focus.is_empty() && !node.visible {
            continue;
        }
        if let Ok(Some(b)) = scene.world_bounding_box(id) {
            out = Some(match out {
                Some(o) => o.union(&b),
                None => b,
            });
        }
    }
    Ok(out)
}

impl CameraSpec {
    /// Resolve to an explicit camera for a viewport with the given aspect ratio.
    pub fn resolve(&self, scene: &Scene, aspect: f32) -> RenderResult<Camera> {
        let cam = match self {
            CameraSpec::Auto {
                view,
                yaw_deg,
                pitch_deg,
                fov_y_deg,
                orthographic,
                focus,
            } => {
                let (yaw, pitch) = view.map(|v| v.yaw_pitch()).unwrap_or((*yaw_deg, *pitch_deg));
                let mut orbit = OrbitCamera::new(Vec3::ZERO, d_distance(), yaw, pitch);
                orbit.fov_y_deg = *fov_y_deg;
                orbit.orthographic = *orthographic;
                if !(fov_y_deg.is_finite() && *fov_y_deg > 0.0 && *fov_y_deg < 180.0) {
                    return Err(RenderError::InvalidCamera(format!(
                        "fov_y_deg {fov_y_deg} must be in (0, 180)"
                    )));
                }
                let bounds = scene_bounds(scene, focus)?;
                let bounds = bounds.unwrap_or(Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)));
                orbit.frame_bounds(&bounds, aspect);
                let mut c = orbit.to_camera();
                c.fit_clip_to_bounds(&bounds);
                c
            }
            CameraSpec::Orbit(o) => {
                if !(o.distance.is_finite() && o.distance > 0.0) {
                    return Err(RenderError::InvalidCamera("distance must be > 0".into()));
                }
                o.to_camera()
            }
            CameraSpec::LookAt(c) => *c,
        };
        cam.validate()?;
        Ok(cam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_eye_directions() {
        let o = OrbitCamera::new(Vec3::ZERO, 5.0, 0.0, 0.0);
        assert!(o.eye().abs_diff_eq(Vec3::new(5.0, 0.0, 0.0), 1e-5));
        let o = OrbitCamera::new(Vec3::ONE, 2.0, -90.0, 0.0);
        assert!(o.eye().abs_diff_eq(Vec3::new(1.0, -1.0, 1.0), 1e-5));
    }

    #[test]
    fn linear_depth_roundtrip() {
        let c = Camera::look_at(Vec3::new(0.0, -10.0, 0.0), Vec3::ZERO, Vec3::Z, 50.0);
        let vp = c.view_projection(1.5);
        for d in [0.5f32, 3.0, 10.0, 70.0] {
            let p = c.eye + c.forward() * d;
            let clip = vp * p.extend(1.0);
            let z = clip.z / clip.w;
            assert!((c.linear_depth(z) - d).abs() < d * 1e-3, "{d}");
        }
    }
}

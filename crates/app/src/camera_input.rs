//! Mapping of mouse / keyboard input to orbit-camera motion.

use forge3d_core::math::Aabb;
use forge3d_render::{OrbitCamera, ViewPreset};

/// Pointer buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Middle,
    Secondary,
}

/// Keyboard modifiers relevant to the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// What a viewport drag does to the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Orbit,
    Pan,
}

/// Camera navigation for a drag with `button` and `mods`, or `None` when the
/// drag is not a navigation gesture (e.g. a plain left drag).
///
/// * middle drag → orbit, Shift + middle drag → pan,
/// * Alt + left drag → orbit, Alt + Shift + left drag → pan.
pub fn nav_for_drag(button: Button, mods: Mods) -> Option<Nav> {
    match button {
        Button::Middle if mods.shift => Some(Nav::Pan),
        Button::Middle => Some(Nav::Orbit),
        Button::Primary if mods.alt && mods.shift => Some(Nav::Pan),
        Button::Primary if mods.alt => Some(Nav::Orbit),
        _ => None,
    }
}

/// Degrees of rotation per dragged pixel.
pub const ORBIT_DEG_PER_PIXEL: f32 = 0.4;

/// Apply a drag of `(dx, dy)` pixels to the camera.
pub fn apply_drag(cam: &mut OrbitCamera, nav: Nav, dx: f32, dy: f32, viewport_h: f32) {
    match nav {
        // Dragging right turns the scene with the cursor (camera yaw decreases).
        Nav::Orbit => cam.orbit(-dx * ORBIT_DEG_PER_PIXEL, dy * ORBIT_DEG_PER_PIXEL),
        Nav::Pan => cam.pan_pixels(dx, dy, viewport_h),
    }
}

/// Apply scroll (positive = away from the user = zoom in) in points.
pub fn apply_scroll(cam: &mut OrbitCamera, scroll_y: f32) {
    if scroll_y != 0.0 && scroll_y.is_finite() {
        cam.zoom((-scroll_y * 0.0015).exp());
    }
}

/// Switch to a named view, keeping target, distance and projection.
pub fn apply_preset(cam: &mut OrbitCamera, preset: ViewPreset) {
    let (yaw, pitch) = preset.yaw_pitch();
    cam.yaw_deg = yaw;
    cam.pitch_deg = pitch;
}

/// Frame `bounds` (or reset to the default distance around the origin).
pub fn frame(cam: &mut OrbitCamera, bounds: Option<Aabb>, aspect: f32) {
    match bounds {
        Some(b) => cam.frame_bounds(&b, aspect),
        None => {
            cam.target = glam::Vec3::ZERO;
            cam.distance = OrbitCamera::default().distance;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn navigation_mapping() {
        let none = Mods::default();
        let shift = Mods {
            shift: true,
            ..none
        };
        let alt = Mods { alt: true, ..none };
        assert_eq!(nav_for_drag(Button::Middle, none), Some(Nav::Orbit));
        assert_eq!(nav_for_drag(Button::Middle, shift), Some(Nav::Pan));
        assert_eq!(nav_for_drag(Button::Primary, alt), Some(Nav::Orbit));
        assert_eq!(
            nav_for_drag(
                Button::Primary,
                Mods {
                    alt: true,
                    shift: true,
                    ctrl: false
                }
            ),
            Some(Nav::Pan)
        );
        assert_eq!(nav_for_drag(Button::Primary, none), None);
        assert_eq!(nav_for_drag(Button::Secondary, alt), None);
    }

    #[test]
    fn orbit_pan_zoom_change_the_camera() {
        let mut cam = OrbitCamera::default();
        let before = cam;
        apply_drag(&mut cam, Nav::Orbit, 10.0, 5.0, 600.0);
        assert!(cam.yaw_deg != before.yaw_deg && cam.pitch_deg != before.pitch_deg);
        let t = cam.target;
        apply_drag(&mut cam, Nav::Pan, 30.0, 0.0, 600.0);
        assert!((cam.target - t).length() > 0.1);
        let d = cam.distance;
        apply_scroll(&mut cam, 100.0);
        assert!(cam.distance < d);
        apply_scroll(&mut cam, -100.0);
        assert!((cam.distance - d).abs() < 1e-3);
        apply_scroll(&mut cam, f32::NAN);
        assert!(cam.distance.is_finite());
    }

    #[test]
    fn presets_and_framing() {
        let mut cam = OrbitCamera::default();
        apply_preset(&mut cam, ViewPreset::Top);
        assert!(cam.pitch_deg > 89.0);
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)).transformed(
            &glam::Affine3A::from_translation(Vec3::new(5.0, 0.0, 0.0)),
        );
        frame(&mut cam, Some(b), 1.5);
        assert!((cam.target - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-4);
        frame(&mut cam, None, 1.5);
        assert_eq!(cam.target, Vec3::ZERO);
    }
}

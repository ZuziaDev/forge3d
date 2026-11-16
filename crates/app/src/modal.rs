//! Blender-style modal operators (G / R / S / E / I / Ctrl+B).
//!
//! [`Modal`] is a pure state machine: it is fed cursor positions, axis keys
//! and typed digits and answers with the [`DragDelta`] (transforms) or scalar
//! (extrude distance, inset thickness, bevel width) that the state applies as
//! a live preview. It knows nothing about documents or egui.

use forge3d_render::Camera;
use glam::{Vec2, Vec3};

use crate::gizmo::{
    line_param, project, ray_plane, world_per_pixel, DragDelta, ROTATE_SNAP_DEG, SCALE_SNAP,
};

/// What the modal operator edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    Grab,
    Rotate,
    Scale,
    Extrude,
    Inset,
    Bevel,
}

impl ModalKind {
    pub fn label(self) -> &'static str {
        match self {
            ModalKind::Grab => "Move",
            ModalKind::Rotate => "Rotate",
            ModalKind::Scale => "Scale",
            ModalKind::Extrude => "Extrude",
            ModalKind::Inset => "Inset",
            ModalKind::Bevel => "Bevel",
        }
    }

    /// Transform kinds produce a [`DragDelta`]; the others a scalar.
    pub fn is_transform(self) -> bool {
        matches!(self, ModalKind::Grab | ModalKind::Rotate | ModalKind::Scale)
    }
}

/// Axis constraint of a transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisLock {
    Free,
    /// Only this axis (0 = X, 1 = Y, 2 = Z).
    Axis(usize),
    /// Everything except this axis (Shift+axis).
    Plane(usize),
}

impl AxisLock {
    /// Pressing the key of `axis` (with Shift for a plane). Pressing the same
    /// constraint again releases it.
    pub fn press(self, axis: usize, shift: bool) -> AxisLock {
        let next = if shift {
            AxisLock::Plane(axis)
        } else {
            AxisLock::Axis(axis)
        };
        if self == next {
            AxisLock::Free
        } else {
            next
        }
    }

    pub fn label(self) -> String {
        const N: [&str; 3] = ["X", "Y", "Z"];
        match self {
            AxisLock::Free => String::new(),
            AxisLock::Axis(a) => N[a].to_string(),
            AxisLock::Plane(a) => format!("{} {}", N[(a + 1) % 3], N[(a + 2) % 3]),
        }
    }
}

/// Typed numeric value (digits, `.`, `-` toggles the sign, Backspace).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NumericInput {
    buf: String,
}

impl NumericInput {
    /// Feed a typed character. Returns whether it was accepted.
    pub fn push(&mut self, c: char) -> bool {
        match c {
            '0'..='9' => {
                self.buf.push(c);
                true
            }
            '.' | ',' if !self.buf.contains('.') => {
                self.buf.push('.');
                true
            }
            '-' => {
                if let Some(rest) = self.buf.strip_prefix('-') {
                    self.buf = rest.to_string();
                } else {
                    self.buf.insert(0, '-');
                }
                true
            }
            _ => false,
        }
    }

    pub fn backspace(&mut self) {
        self.buf.pop();
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn text(&self) -> &str {
        &self.buf
    }

    /// The typed number; a lone `-` or `.` counts as 0.
    pub fn value(&self) -> Option<f32> {
        if self.buf.is_empty() {
            None
        } else {
            Some(self.buf.parse().unwrap_or(0.0))
        }
    }
}

/// A running modal operator.
#[derive(Debug, Clone)]
pub struct Modal {
    pub kind: ModalKind,
    pub lock: AxisLock,
    pub numeric: NumericInput,
    /// Ctrl held: snap the result.
    pub snap: bool,
    origin: Vec3,
    axes: [Vec3; 3],
    start: Vec2,
    cursor: Vec2,
    delta: DragDelta,
    scalar: f32,
    // Start values of the pointer measurements (None = not measurable yet).
    start_world: Option<Vec3>,
    start_angle: f32,
}

fn identity(kind: ModalKind) -> DragDelta {
    match kind {
        ModalKind::Rotate => DragDelta::Rotate {
            axis: Vec3::Z,
            angle_deg: 0.0,
        },
        ModalKind::Scale => DragDelta::Scale(Vec3::ONE),
        _ => DragDelta::Translate(Vec3::ZERO),
    }
}

fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let mut r = a % t;
    if r > std::f32::consts::PI {
        r -= t;
    } else if r < -std::f32::consts::PI {
        r += t;
    }
    r
}

impl Modal {
    /// Start an operator at pivot `origin`; `axes` are the axes that X/Y/Z
    /// refer to, `cursor` the pointer in viewport points.
    pub fn begin(kind: ModalKind, origin: Vec3, axes: [Vec3; 3], cursor: Vec2) -> Modal {
        Modal {
            kind,
            lock: AxisLock::Free,
            numeric: NumericInput::default(),
            snap: false,
            origin,
            axes,
            start: cursor,
            cursor,
            delta: identity(kind),
            scalar: 0.0,
            start_world: None,
            start_angle: 0.0,
        }
    }

    pub fn delta(&self) -> DragDelta {
        self.delta
    }

    /// Value of the scalar kinds (extrude distance, inset thickness, bevel width).
    pub fn scalar(&self) -> f32 {
        self.scalar
    }

    /// True while nothing changed yet.
    pub fn is_identity(&self) -> bool {
        if self.kind.is_transform() {
            self.delta.is_identity()
        } else {
            self.scalar.abs() < 1e-7
        }
    }

    /// Pressing an axis key (`shift` = plane constraint). Ignored for the
    /// scalar kinds.
    pub fn press_axis(&mut self, axis: usize, shift: bool) {
        if self.kind.is_transform() {
            self.lock = self.lock.press(axis, shift);
            self.start_world = None;
        }
    }

    /// The pointer moved to `cursor` (viewport points).
    pub fn set_cursor(&mut self, cursor: Vec2) {
        self.cursor = cursor;
    }

    /// Recompute the result for the current cursor, lock and typed value.
    pub fn update(&mut self, cam: &Camera, size: (u32, u32)) {
        if self.kind.is_transform() {
            self.update_transform(cam, size);
        } else {
            self.update_scalar(cam, size);
        }
    }

    fn view_normal(cam: &Camera) -> Vec3 {
        cam.forward()
    }

    fn ray(cam: &Camera, size: (u32, u32), p: Vec2) -> forge3d_render::Ray {
        cam.ray_through_pixel(p.x, p.y, size.0, size.1)
    }

    /// World point under the pointer for the current constraint.
    fn world_at(&self, cam: &Camera, size: (u32, u32), p: Vec2) -> Option<Vec3> {
        let ray = Self::ray(cam, size, p);
        match self.lock {
            AxisLock::Free => ray_plane(&ray, self.origin, Self::view_normal(cam)),
            AxisLock::Axis(i) => {
                line_param(&ray, self.origin, self.axes[i]).map(|t| self.origin + self.axes[i] * t)
            }
            AxisLock::Plane(i) => ray_plane(&ray, self.origin, self.axes[i]),
        }
    }

    fn update_transform(&mut self, cam: &Camera, size: (u32, u32)) {
        match self.kind {
            ModalKind::Grab => self.update_grab(cam, size),
            ModalKind::Rotate => self.update_rotate(cam, size),
            _ => self.update_scale(cam, size),
        }
    }

    fn update_grab(&mut self, cam: &Camera, size: (u32, u32)) {
        if let Some(v) = self.numeric.value() {
            let d = match self.lock {
                AxisLock::Free => Vec3::X * v,
                AxisLock::Axis(i) => self.axes[i] * v,
                AxisLock::Plane(i) => self.axes[(i + 1) % 3] * v + self.axes[(i + 2) % 3] * v,
            };
            self.delta = DragDelta::Translate(d);
            return;
        }
        let Some(now) = self.world_at(cam, size, self.cursor) else {
            return;
        };
        let Some(start) = self
            .start_world
            .or_else(|| self.world_at(cam, size, self.start))
        else {
            return;
        };
        self.start_world = Some(start);
        let mut d = now - start;
        if self.snap {
            let step = cam.grid_cell_size() * 0.25;
            if step > 0.0 {
                d = (d / step).round() * step;
            }
        }
        self.delta = DragDelta::Translate(d);
    }

    fn rotation_axis(&self, cam: &Camera) -> Vec3 {
        match self.lock {
            AxisLock::Axis(i) => self.axes[i],
            AxisLock::Plane(i) => self.axes[i],
            AxisLock::Free => -Self::view_normal(cam),
        }
    }

    fn update_rotate(&mut self, cam: &Camera, size: (u32, u32)) {
        let axis = self.rotation_axis(cam);
        if let Some(v) = self.numeric.value() {
            self.delta = DragDelta::Rotate { axis, angle_deg: v };
            return;
        }
        let Some(center) = project(cam, self.origin, size) else {
            return;
        };
        let angle_of = |p: Vec2| (p.y - center.y).atan2(p.x - center.x);
        self.start_angle = angle_of(self.start);
        // Screen y points down, so a counter-clockwise turn is a decreasing angle.
        let mut a = -wrap_pi(angle_of(self.cursor) - self.start_angle).to_degrees();
        // An axis that points away from the viewer turns the other way.
        if axis.dot(-Self::view_normal(cam)) < 0.0 {
            a = -a;
        }
        if self.snap {
            a = (a / ROTATE_SNAP_DEG).round() * ROTATE_SNAP_DEG;
        }
        self.delta = DragDelta::Rotate { axis, angle_deg: a };
    }

    fn scale_vector(&self, f: f32) -> Vec3 {
        match self.lock {
            AxisLock::Free => Vec3::splat(f),
            AxisLock::Axis(i) => {
                let mut s = Vec3::ONE;
                s[i] = f;
                s
            }
            AxisLock::Plane(i) => {
                let mut s = Vec3::splat(f);
                s[i] = 1.0;
                s
            }
        }
    }

    fn update_scale(&mut self, cam: &Camera, size: (u32, u32)) {
        if let Some(v) = self.numeric.value() {
            self.delta = DragDelta::Scale(self.scale_vector(v));
            return;
        }
        let Some(center) = project(cam, self.origin, size) else {
            return;
        };
        let d0 = (self.start - center).length().max(8.0);
        let mut f = ((self.cursor - center).length() / d0).max(0.001);
        if self.snap {
            f = ((f / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP);
        }
        self.delta = DragDelta::Scale(self.scale_vector(f));
    }

    fn update_scalar(&mut self, cam: &Camera, size: (u32, u32)) {
        if let Some(v) = self.numeric.value() {
            self.scalar = v;
            return;
        }
        let wpp = world_per_pixel(cam, self.origin, size.1 as f32);
        let mut v = match self.kind {
            // Dragging up pushes along the normal, down pulls inwards.
            ModalKind::Extrude => (self.start.y - self.cursor.y) * wpp,
            _ => (self.cursor - self.start).length() * wpp * 0.5,
        };
        if self.snap {
            let step = cam.grid_cell_size() * 0.25;
            if step > 0.0 {
                v = (v / step).round() * step;
            }
        }
        self.scalar = v;
    }

    /// One-line description for the HUD, e.g. `Move  X  +1.250`.
    pub fn hud(&self) -> String {
        let lock = self.lock.label();
        let lock = if lock.is_empty() {
            String::new()
        } else {
            format!("  [{lock}]")
        };
        let typed = if self.numeric.is_empty() {
            String::new()
        } else {
            format!("  typed: {}", self.numeric.text())
        };
        let value = match (self.kind, self.delta) {
            (ModalKind::Grab, DragDelta::Translate(t)) => {
                format!("{:+.3}  {:+.3}  {:+.3}", t.x, t.y, t.z)
            }
            (ModalKind::Rotate, DragDelta::Rotate { angle_deg, .. }) => {
                format!("{angle_deg:+.1}\u{b0}")
            }
            (ModalKind::Scale, DragDelta::Scale(s)) => {
                format!("{:.3}  {:.3}  {:.3}", s.x, s.y, s.z)
            }
            _ => format!("{:+.3}", self.scalar),
        };
        format!("{}{lock}  {value}{typed}", self.kind.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        forge3d_render::OrbitCamera::default().to_camera()
    }

    const SIZE: (u32, u32) = (800, 600);

    fn modal(kind: ModalKind) -> Modal {
        let c = cam();
        let start = project(&c, Vec3::ZERO, SIZE).unwrap();
        Modal::begin(kind, Vec3::ZERO, [Vec3::X, Vec3::Y, Vec3::Z], start)
    }

    #[test]
    fn axis_lock_toggles_and_switches() {
        let l = AxisLock::Free.press(0, false);
        assert_eq!(l, AxisLock::Axis(0));
        assert_eq!(l.press(0, false), AxisLock::Free);
        assert_eq!(l.press(1, false), AxisLock::Axis(1));
        assert_eq!(l.press(2, true), AxisLock::Plane(2));
        assert_eq!(AxisLock::Plane(2).press(2, true), AxisLock::Free);
        assert_eq!(AxisLock::Plane(2).label(), "X Y");
    }

    #[test]
    fn numeric_input_parses_signs_and_backspace() {
        let mut n = NumericInput::default();
        assert_eq!(n.value(), None);
        for c in "1.25".chars() {
            assert!(n.push(c));
        }
        assert!(!n.push('.'), "second dot rejected");
        assert!(!n.push('x'));
        assert_eq!(n.value(), Some(1.25));
        n.push('-');
        assert_eq!(n.value(), Some(-1.25));
        n.push('-');
        n.backspace();
        n.backspace();
        n.backspace();
        assert_eq!(n.text(), "1");
        n.backspace();
        n.push('-');
        assert_eq!(n.value(), Some(0.0), "lone minus counts as zero");
    }

    #[test]
    fn typed_value_overrides_the_mouse_and_respects_the_lock() {
        let c = cam();
        let mut m = modal(ModalKind::Grab);
        m.press_axis(1, false);
        for ch in "-2".chars() {
            m.numeric.push(ch);
        }
        m.set_cursor(Vec2::new(10.0, 10.0));
        m.update(&c, SIZE);
        assert_eq!(m.delta(), DragDelta::Translate(Vec3::new(0.0, -2.0, 0.0)));
        // plane lock: both plane axes
        m.press_axis(2, true);
        m.update(&c, SIZE);
        assert_eq!(m.delta(), DragDelta::Translate(Vec3::new(-2.0, -2.0, 0.0)));
    }

    #[test]
    fn grab_with_axis_lock_moves_only_along_the_axis() {
        let c = cam();
        let mut m = modal(ModalKind::Grab);
        m.press_axis(0, false);
        let target = project(&c, Vec3::new(1.0, 0.0, 0.0), SIZE).unwrap();
        m.set_cursor(target);
        m.update(&c, SIZE);
        let DragDelta::Translate(t) = m.delta() else {
            panic!("not a translation")
        };
        assert!((t - Vec3::X).length() < 1e-3, "got {t:?}");
    }

    #[test]
    fn grab_in_a_plane_never_leaves_it() {
        let c = cam();
        let mut m = modal(ModalKind::Grab);
        m.press_axis(2, true);
        let target = project(&c, Vec3::new(1.0, 2.0, 0.0), SIZE).unwrap();
        m.set_cursor(target);
        m.update(&c, SIZE);
        let DragDelta::Translate(t) = m.delta() else {
            panic!("not a translation")
        };
        assert!(t.z.abs() < 1e-4);
        assert!((t - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-2, "got {t:?}");
    }

    #[test]
    fn scale_grows_with_pointer_distance_and_locks_to_axes() {
        let c = cam();
        let mut m = modal(ModalKind::Scale);
        let center = project(&c, Vec3::ZERO, SIZE).unwrap();
        // Start 40 px away, then move to 80 px away: factor 2.
        m.start = center + Vec2::new(40.0, 0.0);
        m.set_cursor(center + Vec2::new(80.0, 0.0));
        m.update(&c, SIZE);
        let DragDelta::Scale(s) = m.delta() else {
            panic!("not a scale")
        };
        assert!((s - Vec3::splat(2.0)).length() < 1e-4);
        m.press_axis(1, false);
        m.update(&c, SIZE);
        assert_eq!(m.delta(), DragDelta::Scale(Vec3::new(1.0, 2.0, 1.0)));
        m.press_axis(1, true);
        m.update(&c, SIZE);
        assert_eq!(m.delta(), DragDelta::Scale(Vec3::new(2.0, 1.0, 2.0)));
        m.numeric.push('3');
        m.update(&c, SIZE);
        assert_eq!(m.delta(), DragDelta::Scale(Vec3::new(3.0, 1.0, 3.0)));
    }

    #[test]
    fn rotate_follows_the_pointer_and_accepts_degrees() {
        let c = cam();
        let mut m = modal(ModalKind::Rotate);
        let center = project(&c, Vec3::ZERO, SIZE).unwrap();
        m.start = center + Vec2::new(50.0, 0.0);
        m.set_cursor(center + Vec2::new(0.0, -50.0)); // a quarter turn counter-clockwise on screen
        m.update(&c, SIZE);
        let DragDelta::Rotate { axis, angle_deg } = m.delta() else {
            panic!("not a rotation")
        };
        assert!((angle_deg - 90.0).abs() < 1e-3, "got {angle_deg}");
        assert!((axis + c.forward()).length() < 1e-4);
        m.press_axis(2, false);
        for ch in "45".chars() {
            m.numeric.push(ch);
        }
        m.update(&c, SIZE);
        assert_eq!(
            m.delta(),
            DragDelta::Rotate {
                axis: Vec3::Z,
                angle_deg: 45.0
            }
        );
    }

    #[test]
    fn scalar_kinds_use_the_typed_value() {
        let c = cam();
        let mut m = modal(ModalKind::Extrude);
        m.press_axis(0, false);
        assert_eq!(m.lock, AxisLock::Free, "no axis lock for scalar tools");
        let start = m.start;
        m.set_cursor(start + Vec2::new(0.0, -100.0));
        m.update(&c, SIZE);
        assert!(m.scalar() > 0.0);
        m.numeric.push('1');
        m.numeric.push('.');
        m.numeric.push('5');
        m.update(&c, SIZE);
        assert_eq!(m.scalar(), 1.5);
        assert!(m.hud().contains("typed: 1.5"));
    }
}

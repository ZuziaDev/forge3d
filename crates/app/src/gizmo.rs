//! Transform gizmo: screen-space geometry, hit testing and drag math.
//!
//! Everything here is plain math on a [`Camera`] and a viewport size in
//! pixels (top-left origin, like picking), so it is unit-testable without a
//! window or a GPU. The UI draws the [`Shape`]s returned by [`parts`] and
//! feeds pointer positions to [`hit_test`] and [`GizmoDrag`].

use forge3d_render::camera::{Camera, Projection, Ray};
use glam::{Vec2, Vec3};

/// Which manipulation the gizmo performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl GizmoMode {
    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Translate => "Move",
            GizmoMode::Rotate => "Rotate",
            GizmoMode::Scale => "Scale",
        }
    }
}

/// A draggable part of the gizmo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Axis arrow / rotation ring / scale box of axis 0 (X), 1 (Y) or 2 (Z).
    Axis(usize),
    /// Uniform scale box at the gizmo center.
    Uniform,
}

/// Gizmo size on screen in pixels (axis length).
pub const GIZMO_PIXELS: f32 = 92.0;
/// Pointer distance in pixels within which a handle is hit.
pub const HIT_RADIUS: f32 = 9.0;
/// Rotation snap in degrees while Ctrl is held.
pub const ROTATE_SNAP_DEG: f32 = 15.0;
/// Scale snap while Ctrl is held.
pub const SCALE_SNAP: f32 = 0.1;

const RING_SEGMENTS: usize = 64;

/// Origin and axes (unit length, world space) of the gizmo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoFrame {
    pub origin: Vec3,
    pub axes: [Vec3; 3],
}

impl GizmoFrame {
    /// Frame with the world axes.
    pub fn world(origin: Vec3) -> GizmoFrame {
        GizmoFrame {
            origin,
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
        }
    }

    /// Frame whose axes are the normalized columns of a linear map
    /// (e.g. an object's world transform).
    pub fn oriented(origin: Vec3, columns: [Vec3; 3]) -> GizmoFrame {
        let fallback = [Vec3::X, Vec3::Y, Vec3::Z];
        let mut axes = fallback;
        for i in 0..3 {
            axes[i] = columns[i].try_normalize().unwrap_or(fallback[i]);
        }
        GizmoFrame { origin, axes }
    }
}

/// A 2D drawing primitive in viewport pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line(Vec2, Vec2),
    Polyline(Vec<Vec2>),
    Triangle([Vec2; 3]),
    Square { center: Vec2, half: f32 },
}

/// A handle with the shapes that draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct GizmoPart {
    pub handle: Handle,
    pub shapes: Vec<Shape>,
}

/// World size of one pixel at point `p` (perspective: at its depth).
pub fn world_per_pixel(cam: &Camera, p: Vec3, height_px: f32) -> f32 {
    let h = height_px.max(1.0);
    match cam.projection {
        Projection::Perspective { fov_y_deg } => {
            let depth = (p - cam.eye).dot(cam.forward()).max(1e-4);
            2.0 * depth * (fov_y_deg.to_radians() * 0.5).tan() / h
        }
        Projection::Orthographic { height } => height / h,
    }
}

/// Project a world point to viewport pixels.
pub fn project(cam: &Camera, p: Vec3, size: (u32, u32)) -> Option<Vec2> {
    cam.project_to_pixel(p, size.0, size.1)
        .map(|v| Vec2::new(v.x, v.y))
}

/// World length of the gizmo axes for the current view.
pub fn axis_length(cam: &Camera, frame: &GizmoFrame, size: (u32, u32)) -> f32 {
    GIZMO_PIXELS * world_per_pixel(cam, frame.origin, size.1 as f32)
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 1e-9 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

fn point_in_triangle(p: Vec2, t: &[Vec2; 3]) -> bool {
    let s = |a: Vec2, b: Vec2, c: Vec2| (a.x - c.x) * (b.y - c.y) - (b.x - c.x) * (a.y - c.y);
    let d1 = s(p, t[0], t[1]);
    let d2 = s(p, t[1], t[2]);
    let d3 = s(p, t[2], t[0]);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

impl Shape {
    /// Distance from a point to the shape (0 inside filled shapes).
    pub fn distance(&self, p: Vec2) -> f32 {
        match self {
            Shape::Line(a, b) => distance_to_segment(p, *a, *b),
            Shape::Polyline(pts) => pts
                .windows(2)
                .map(|w| distance_to_segment(p, w[0], w[1]))
                .fold(f32::INFINITY, f32::min),
            Shape::Triangle(t) => {
                if point_in_triangle(p, t) {
                    0.0
                } else {
                    (0..3)
                        .map(|i| distance_to_segment(p, t[i], t[(i + 1) % 3]))
                        .fold(f32::INFINITY, f32::min)
                }
            }
            Shape::Square { center, half } => {
                let d = (p - *center).abs() - Vec2::splat(*half);
                d.max(Vec2::ZERO).length()
            }
        }
    }
}

/// Screen-space shapes of all handles of the gizmo.
pub fn parts(
    mode: GizmoMode,
    frame: &GizmoFrame,
    cam: &Camera,
    size: (u32, u32),
) -> Vec<GizmoPart> {
    let len = axis_length(cam, frame, size);
    let Some(o2) = project(cam, frame.origin, size) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..3 {
        let axis = frame.axes[i];
        let tip_w = frame.origin + axis * len;
        let Some(tip) = project(cam, tip_w, size) else {
            continue;
        };
        let shapes = match mode {
            GizmoMode::Translate => {
                let head_w = frame.origin + axis * (len * 0.78);
                let head = project(cam, head_w, size).unwrap_or(tip);
                let dir = (tip - head).try_normalize().unwrap_or(Vec2::X);
                let perp = Vec2::new(-dir.y, dir.x) * 5.5;
                vec![
                    Shape::Line(o2, head),
                    Shape::Triangle([head + perp, head - perp, tip]),
                ]
            }
            GizmoMode::Scale => {
                let box_w = frame.origin + axis * (len * 0.9);
                let b = project(cam, box_w, size).unwrap_or(tip);
                vec![
                    Shape::Line(o2, b),
                    Shape::Square {
                        center: b,
                        half: 5.5,
                    },
                ]
            }
            GizmoMode::Rotate => ring_arcs(frame, cam, size, i, len * 0.85),
        };
        out.push(GizmoPart {
            handle: Handle::Axis(i),
            shapes,
        });
    }
    if mode == GizmoMode::Scale {
        out.push(GizmoPart {
            handle: Handle::Uniform,
            shapes: vec![Shape::Square {
                center: o2,
                half: 7.0,
            }],
        });
    }
    out
}

/// Visible (camera-facing) arcs of the rotation ring around axis `i`.
fn ring_arcs(
    frame: &GizmoFrame,
    cam: &Camera,
    size: (u32, u32),
    i: usize,
    radius: f32,
) -> Vec<Shape> {
    let u = frame.axes[(i + 1) % 3];
    let v = frame.axes[(i + 2) % 3];
    let to_eye = |p: Vec3| -> Vec3 {
        if cam.is_orthographic() {
            -cam.forward()
        } else {
            cam.eye - p
        }
    };
    let mut arcs: Vec<Shape> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for k in 0..=RING_SEGMENTS {
        let a = k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
        let dir = u * a.cos() + v * a.sin();
        let p = frame.origin + dir * radius;
        let facing = dir.dot(to_eye(p)) >= 0.0;
        match (facing, project(cam, p, size)) {
            (true, Some(s)) => cur.push(s),
            _ => {
                if cur.len() > 1 {
                    arcs.push(Shape::Polyline(std::mem::take(&mut cur)));
                } else {
                    cur.clear();
                }
            }
        }
    }
    if cur.len() > 1 {
        arcs.push(Shape::Polyline(cur));
    }
    arcs
}

/// The handle under `cursor` (viewport pixels), if any. The closest handle
/// within [`HIT_RADIUS`] wins; the uniform-scale box has priority.
pub fn hit_test(
    mode: GizmoMode,
    frame: &GizmoFrame,
    cam: &Camera,
    size: (u32, u32),
    cursor: Vec2,
) -> Option<Handle> {
    let parts = parts(mode, frame, cam, size);
    if let Some(p) = parts.iter().find(|p| p.handle == Handle::Uniform) {
        if p.shapes.iter().any(|s| s.distance(cursor) <= 2.0) {
            return Some(Handle::Uniform);
        }
    }
    parts
        .iter()
        .filter(|p| p.handle != Handle::Uniform)
        .filter_map(|p| {
            let d = p
                .shapes
                .iter()
                .map(|s| s.distance(cursor))
                .fold(f32::INFINITY, f32::min);
            (d <= HIT_RADIUS).then_some((d, p.handle))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, h)| h)
}

/// Parameter `t` of the point `origin + t * dir` on the line closest to `ray`.
/// `None` when the ray is (nearly) parallel to the line.
pub fn line_param(ray: &Ray, origin: Vec3, dir: Vec3) -> Option<f32> {
    let w0 = origin - ray.origin;
    let b = dir.dot(ray.dir);
    let denom = 1.0 - b * b;
    if denom < 1e-5 {
        return None;
    }
    let d = dir.dot(w0);
    let e = ray.dir.dot(w0);
    Some((b * e - d) / denom)
}

/// Intersection of `ray` with the plane through `origin` with `normal`.
pub fn ray_plane(ray: &Ray, origin: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = normal.dot(ray.dir);
    if denom.abs() < 1e-4 {
        return None;
    }
    let t = normal.dot(origin - ray.origin) / denom;
    Some(ray.at(t))
}

/// Result of a drag, relative to the drag start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragDelta {
    /// World-space offset.
    Translate(Vec3),
    /// Rotation around a unit world axis through the gizmo origin.
    Rotate { axis: Vec3, angle_deg: f32 },
    /// Scale factors along the gizmo axes (or uniform in all three).
    Scale(Vec3),
}

impl DragDelta {
    /// True when applying the delta changes nothing.
    pub fn is_identity(&self) -> bool {
        match *self {
            DragDelta::Translate(t) => t.length_squared() < 1e-14,
            DragDelta::Rotate { angle_deg, .. } => angle_deg.abs() < 1e-6,
            DragDelta::Scale(s) => (s - Vec3::ONE).abs().max_element() < 1e-6,
        }
    }
}

/// An in-progress drag of one handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoDrag {
    pub mode: GizmoMode,
    pub handle: Handle,
    pub frame: GizmoFrame,
    start_param: f32,
    start_angle: f32,
    start_cursor: Vec2,
    last: DragDelta,
}

fn plane_angle(frame: &GizmoFrame, axis: usize, p: Vec3) -> f32 {
    let u = frame.axes[(axis + 1) % 3];
    let v = frame.axes[(axis + 2) % 3];
    let d = p - frame.origin;
    d.dot(v).atan2(d.dot(u))
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

impl GizmoDrag {
    /// Start dragging `handle` with the pointer at `cursor`.
    pub fn begin(
        mode: GizmoMode,
        handle: Handle,
        frame: GizmoFrame,
        cam: &Camera,
        size: (u32, u32),
        cursor: Vec2,
    ) -> GizmoDrag {
        let ray = cam.ray_through_pixel(cursor.x, cursor.y, size.0, size.1);
        let mut start_param = 0.0;
        let mut start_angle = 0.0;
        if let Handle::Axis(i) = handle {
            match mode {
                GizmoMode::Translate | GizmoMode::Scale => {
                    start_param = line_param(&ray, frame.origin, frame.axes[i]).unwrap_or(0.0);
                }
                GizmoMode::Rotate => {
                    if let Some(p) = ray_plane(&ray, frame.origin, frame.axes[i]) {
                        start_angle = plane_angle(&frame, i, p);
                    }
                }
            }
        }
        let last = match mode {
            GizmoMode::Translate => DragDelta::Translate(Vec3::ZERO),
            GizmoMode::Rotate => DragDelta::Rotate {
                axis: match handle {
                    Handle::Axis(i) => frame.axes[i],
                    Handle::Uniform => Vec3::Z,
                },
                angle_deg: 0.0,
            },
            GizmoMode::Scale => DragDelta::Scale(Vec3::ONE),
        };
        GizmoDrag {
            mode,
            handle,
            frame,
            start_param,
            start_angle,
            start_cursor: cursor,
            last,
        }
    }

    /// The delta for the pointer at `cursor`. `snap` rounds to
    /// `snap_step` world units / [`ROTATE_SNAP_DEG`] / [`SCALE_SNAP`].
    /// When the pointer ray degenerates (edge-on), the previous delta is kept.
    pub fn update(
        &mut self,
        cam: &Camera,
        size: (u32, u32),
        cursor: Vec2,
        snap: bool,
        snap_step: f32,
    ) -> DragDelta {
        let ray = cam.ray_through_pixel(cursor.x, cursor.y, size.0, size.1);
        let delta = match (self.mode, self.handle) {
            (GizmoMode::Translate, Handle::Axis(i)) => {
                line_param(&ray, self.frame.origin, self.frame.axes[i]).map(|t| {
                    let mut d = t - self.start_param;
                    if snap && snap_step > 0.0 {
                        d = (d / snap_step).round() * snap_step;
                    }
                    DragDelta::Translate(self.frame.axes[i] * d)
                })
            }
            (GizmoMode::Rotate, Handle::Axis(i)) => {
                ray_plane(&ray, self.frame.origin, self.frame.axes[i]).map(|p| {
                    let mut a =
                        wrap_pi(plane_angle(&self.frame, i, p) - self.start_angle).to_degrees();
                    if snap {
                        a = (a / ROTATE_SNAP_DEG).round() * ROTATE_SNAP_DEG;
                    }
                    DragDelta::Rotate {
                        axis: self.frame.axes[i],
                        angle_deg: a,
                    }
                })
            }
            (GizmoMode::Scale, Handle::Axis(i)) => {
                line_param(&ray, self.frame.origin, self.frame.axes[i])
                    .filter(|_| self.start_param.abs() > 1e-4)
                    .map(|t| {
                        let mut f = (t / self.start_param).max(0.01);
                        if snap {
                            f = ((f / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP);
                        }
                        let mut s = Vec3::ONE;
                        s[i] = f;
                        DragDelta::Scale(s)
                    })
            }
            (GizmoMode::Scale, Handle::Uniform) => {
                let mut f = ((self.start_cursor.y - cursor.y) / 150.0).exp();
                if snap {
                    f = ((f / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP);
                }
                Some(DragDelta::Scale(Vec3::splat(f)))
            }
            _ => None,
        };
        if let Some(d) = delta {
            self.last = d;
        }
        self.last
    }

    /// The most recent delta.
    pub fn delta(&self) -> DragDelta {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge3d_render::OrbitCamera;

    const SIZE: (u32, u32) = (800, 600);

    fn cam() -> Camera {
        OrbitCamera::new(Vec3::ZERO, 8.0, -60.0, 25.0).to_camera()
    }

    fn screen(cam: &Camera, p: Vec3) -> Vec2 {
        project(cam, p, SIZE).unwrap()
    }

    #[test]
    fn translate_axes_are_hit_on_their_shaft() {
        let cam = cam();
        let frame = GizmoFrame::world(Vec3::ZERO);
        let len = axis_length(&cam, &frame, SIZE);
        for i in 0..3 {
            let p = screen(&cam, frame.axes[i] * len * 0.5);
            assert_eq!(
                hit_test(GizmoMode::Translate, &frame, &cam, SIZE, p),
                Some(Handle::Axis(i)),
                "axis {i}"
            );
        }
        // far away from the gizmo: nothing
        assert_eq!(
            hit_test(
                GizmoMode::Translate,
                &frame,
                &cam,
                SIZE,
                Vec2::new(5.0, 5.0)
            ),
            None
        );
    }

    #[test]
    fn scale_has_uniform_center_and_axis_boxes() {
        let cam = cam();
        let frame = GizmoFrame::world(Vec3::ZERO);
        let c = screen(&cam, Vec3::ZERO);
        assert_eq!(
            hit_test(GizmoMode::Scale, &frame, &cam, SIZE, c),
            Some(Handle::Uniform)
        );
        let len = axis_length(&cam, &frame, SIZE);
        let tip = screen(&cam, Vec3::Z * len * 0.9);
        assert_eq!(
            hit_test(GizmoMode::Scale, &frame, &cam, SIZE, tip),
            Some(Handle::Axis(2))
        );
    }

    #[test]
    fn rotate_rings_are_hit() {
        let cam = cam();
        let frame = GizmoFrame::world(Vec3::ZERO);
        let len = axis_length(&cam, &frame, SIZE) * 0.85;
        // The Z ring (horizontal) point nearest the camera is always facing it.
        let toward = (cam.eye - frame.origin).truncate().normalize();
        let p = screen(&cam, Vec3::new(toward.x, toward.y, 0.0) * len);
        assert_eq!(
            hit_test(GizmoMode::Rotate, &frame, &cam, SIZE, p),
            Some(Handle::Axis(2))
        );
    }

    #[test]
    fn translate_drag_follows_the_pointer_along_the_axis() {
        let cam = cam();
        let origin = Vec3::new(1.0, 2.0, 0.5);
        let frame = GizmoFrame::world(origin);
        let len = axis_length(&cam, &frame, SIZE);
        let start = screen(&cam, origin + Vec3::X * len * 0.5);
        let mut drag = GizmoDrag::begin(
            GizmoMode::Translate,
            Handle::Axis(0),
            frame,
            &cam,
            SIZE,
            start,
        );
        // Move the pointer to where the point origin + 1.5 X projects.
        let target = screen(&cam, origin + Vec3::X * (len * 0.5 + 1.5));
        match drag.update(&cam, SIZE, target, false, 0.25) {
            DragDelta::Translate(d) => {
                assert!((d - Vec3::new(1.5, 0.0, 0.0)).length() < 1e-3, "{d:?}");
            }
            other => panic!("unexpected {other:?}"),
        }
        // Movement perpendicular to the axis in 3D is ignored.
        let off = screen(&cam, origin + Vec3::X * (len * 0.5 + 1.5) + Vec3::Y * 0.0);
        let d2 = drag.update(&cam, SIZE, off, false, 0.25);
        assert_eq!(d2, drag.delta());
        // Snapping rounds to the step.
        let snapped = screen(&cam, origin + Vec3::X * (len * 0.5 + 1.4));
        match drag.update(&cam, SIZE, snapped, true, 0.5) {
            DragDelta::Translate(d) => assert!((d.x - 1.5).abs() < 1e-3),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rotate_drag_measures_the_angle_in_the_ring_plane() {
        let cam = OrbitCamera::new(Vec3::ZERO, 8.0, -90.0, 89.0).to_camera(); // top-down
        let frame = GizmoFrame::world(Vec3::ZERO);
        let r = 2.0;
        let start = screen(&cam, Vec3::new(r, 0.0, 0.0));
        let mut drag =
            GizmoDrag::begin(GizmoMode::Rotate, Handle::Axis(2), frame, &cam, SIZE, start);
        let end = screen(&cam, Vec3::new(0.0, r, 0.0));
        match drag.update(&cam, SIZE, end, false, 0.25) {
            DragDelta::Rotate { axis, angle_deg } => {
                assert_eq!(axis, Vec3::Z);
                assert!((angle_deg - 90.0).abs() < 0.5, "{angle_deg}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scale_drag_is_the_ratio_of_axis_parameters() {
        let cam = cam();
        let frame = GizmoFrame::world(Vec3::ZERO);
        let len = axis_length(&cam, &frame, SIZE);
        let start = screen(&cam, Vec3::X * len * 0.9);
        let mut drag =
            GizmoDrag::begin(GizmoMode::Scale, Handle::Axis(0), frame, &cam, SIZE, start);
        let end = screen(&cam, Vec3::X * len * 1.8);
        match drag.update(&cam, SIZE, end, false, 0.25) {
            DragDelta::Scale(s) => {
                assert!((s - Vec3::new(2.0, 1.0, 1.0)).length() < 1e-2, "{s:?}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let mut uni = GizmoDrag::begin(
            GizmoMode::Scale,
            Handle::Uniform,
            frame,
            &cam,
            SIZE,
            Vec2::new(100.0, 300.0),
        );
        match uni.update(&cam, SIZE, Vec2::new(100.0, 150.0), false, 0.25) {
            DragDelta::Scale(s) => assert!(s.x > 2.0 && s.x == s.y && s.y == s.z),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn oriented_frame_normalizes_axes() {
        let f = GizmoFrame::oriented(Vec3::ZERO, [Vec3::X * 3.0, Vec3::Y * 0.5, Vec3::ZERO]);
        assert_eq!(f.axes, [Vec3::X, Vec3::Y, Vec3::Z]);
    }
}

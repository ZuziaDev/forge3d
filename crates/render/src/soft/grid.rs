//! Analytic ground grid (z = 0) for the software renderer. The WGSL grid
//! shader implements the same function.

use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::camera::{Camera, Projection};
use crate::options::style;
use crate::shading::smoothstep;

/// Per-frame grid parameters.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GridParams {
    pub cell: f32,
    pub fade_start: f32,
    pub fade_end: f32,
    /// Fade center: the eye (perspective) or the target (orthographic).
    pub center: Vec3,
    pub ortho: bool,
    /// Multiply screen-space derivatives by this (supersampling factor), so
    /// lines keep their width in output pixels.
    pub px_scale: f32,
}

impl GridParams {
    pub(crate) fn new(camera: &Camera, px_scale: f32) -> GridParams {
        let scale = match camera.projection {
            Projection::Perspective { .. } => camera.distance(),
            Projection::Orthographic { height } => height,
        };
        GridParams {
            cell: camera.grid_cell_size(),
            fade_start: scale * style::GRID_FADE_START,
            fade_end: scale * style::GRID_FADE_END,
            center: if camera.is_orthographic() {
                camera.target
            } else {
                camera.eye
            },
            ortho: camera.is_orthographic(),
            px_scale,
        }
    }
}

/// Coverage of 1-px-wide lines at integer multiples of 1 for coordinate `g`
/// with screen derivative `fw` (both in cell units).
fn line_cov(g: f32, fw: f32) -> f32 {
    let fw = fw.max(1e-6);
    let d = ((g - 0.5).rem_euclid(1.0) - 0.5).abs() / fw;
    (1.0 - d).clamp(0.0, 1.0)
}

fn over(dst: [f32; 4], rgb: [f32; 4], a: f32) -> [f32; 4] {
    let a = (rgb[3] * a).clamp(0.0, 1.0);
    [
        rgb[0] * a + dst[0] * (1.0 - a),
        rgb[1] * a + dst[1] * (1.0 - a),
        rgb[2] * a + dst[2] * (1.0 - a),
        a + dst[3] * (1.0 - a),
    ]
}

/// Premultiplied grid color at ground point (`u`, `v`) with screen-space
/// derivatives `fu`, `fv` (sum of |d/dx| and |d/dy|, world units per pixel).
pub(crate) fn grid_color(p: &GridParams, u: f32, v: f32, fu: f32, fv: f32) -> [f32; 4] {
    let (fu, fv) = (fu * p.px_scale, fv * p.px_scale);
    let mut c = [0.0f32; 4];
    let minor_cell = p.cell;
    let major_cell = p.cell * style::GRID_MAJOR_EVERY;
    let lod = |fw: f32| 1.0 - smoothstep(0.15, 0.6, fw);
    let mfw = (fu.max(fv)) / minor_cell;
    let minor = line_cov(u / minor_cell, fu / minor_cell)
        .max(line_cov(v / minor_cell, fv / minor_cell))
        * lod(mfw);
    c = over(c, style::GRID_MINOR, minor);
    let jfw = (fu.max(fv)) / major_cell;
    let major = line_cov(u / major_cell, fu / major_cell)
        .max(line_cov(v / major_cell, fv / major_cell))
        * lod(jfw);
    c = over(c, style::GRID_MAJOR, major);
    let ax = (1.5 - v.abs() / fv.max(1e-9)).clamp(0.0, 1.0);
    c = over(c, style::AXIS_X, ax);
    let ay = (1.5 - u.abs() / fu.max(1e-9)).clamp(0.0, 1.0);
    c = over(c, style::AXIS_Y, ay);
    let dist = if p.ortho {
        Vec2::new(u - p.center.x, v - p.center.y).length()
    } else {
        Vec3::new(u, v, 0.0).distance(p.center)
    };
    let fade = 1.0 - smoothstep(p.fade_start, p.fade_end, dist);
    c.map(|x| x * fade)
}

/// Screen → ground-plane homography for one frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GroundMap {
    /// (ndc x, ndc y, 1) → homogeneous (u, v, 1).
    inv: Mat3,
    /// Rows of the view-projection giving clip z and w for (u, v, 1).
    zrow: Vec3,
    wrow: Vec3,
    width: f32,
    height: f32,
}

impl GroundMap {
    /// `None` when the plane is seen exactly edge-on.
    pub(crate) fn new(view_proj: &Mat4, width: u32, height: u32) -> Option<GroundMap> {
        let m = view_proj;
        let row = |r: usize| Vec3::new(m.col(0)[r], m.col(1)[r], m.col(3)[r]);
        let fwd = Mat3::from_cols(row(0), row(1), row(3)).transpose();
        // fwd maps (u, v, 1) → (clip x, clip y, clip w).
        let det = fwd.determinant();
        if !det.is_finite() || det.abs() < 1e-20 {
            return None;
        }
        Some(GroundMap {
            inv: fwd.inverse(),
            zrow: row(2),
            wrow: row(3),
            width: width as f32,
            height: height as f32,
        })
    }

    /// Ground point (u, v) under pixel position (x, y) and its NDC depth;
    /// `None` when the plane is not visible there.
    pub(crate) fn sample(&self, x: f32, y: f32) -> Option<(f32, f32, f32)> {
        let nx = x / self.width * 2.0 - 1.0;
        let ny = 1.0 - y / self.height * 2.0;
        let q = self.inv * Vec3::new(nx, ny, 1.0);
        if q.z.abs() < 1e-20 {
            return None;
        }
        let uv1 = Vec3::new(q.x / q.z, q.y / q.z, 1.0);
        let w = self.wrow.dot(uv1);
        if w <= 1e-12 {
            return None;
        }
        let z = self.zrow.dot(uv1) / w;
        if !(0.0..=1.0).contains(&z) || !uv1.x.is_finite() || !uv1.y.is_finite() {
            return None;
        }
        Some((uv1.x, uv1.y, z))
    }

    /// Ground grid premultiplied color and depth at pixel center (x, y).
    pub(crate) fn shade(&self, p: &GridParams, x: f32, y: f32) -> Option<([f32; 4], f32)> {
        let (u, v, z) = self.sample(x, y)?;
        let dx = self.sample(x + 1.0, y);
        let dy = self.sample(x, y + 1.0);
        let big = p.cell * 1e3;
        let (du_x, dv_x) = dx.map_or((big, big), |(a, b, _)| ((a - u).abs(), (b - v).abs()));
        let (du_y, dv_y) = dy.map_or((big, big), |(a, b, _)| ((a - u).abs(), (b - v).abs()));
        let c = grid_color(p, u, v, du_x + du_y, dv_x + dv_y);
        (c[3] > 1e-4).then_some((c, z))
    }
}

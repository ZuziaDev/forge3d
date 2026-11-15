//! CPU software rasterizer: renders [`SceneDrawData`] to an [`RgbaImage`]
//! without any GPU.
//!
//! Pipeline: near-plane clipping in clip space → binning into horizontal
//! bands → parallel (rayon) per-band visibility rasterization (z-buffer +
//! triangle id, edge functions in `f64`) → deferred PBR shading of the
//! visible sample only → analytic ground grid → wireframe / selected
//! elements / vertex points (depth-biased, anti-aliased lines) → selection
//! outline → box-filter downsampling of the supersampled buffer → sRGB.

mod grid;
mod overlay;

use forge3d_core::scene::Scene;
use forge3d_core::selection::Selection;
use glam::{Mat4, Vec3, Vec4, Vec4Swizzles};
use rayon::prelude::*;

use crate::camera::Camera;
use crate::error::RenderResult;
use crate::extract::SceneDrawData;
use crate::image::RgbaImage;
use crate::options::{style, RenderOptions};
use crate::shading::{shade, srgb_to_linear, LightRig, SrgbLut, Surface};

pub use crate::pick::pick;

/// Rows per parallel work item.
const BAND: usize = 16;

/// A screen-space triangle ready for rasterization.
#[derive(Debug, Clone, Copy)]
struct RTri {
    sx: [f32; 3],
    sy: [f32; 3],
    z: [f32; 3],
    iw: [f32; 3],
    world: [Vec3; 3],
    normal: [Vec3; 3],
    obj: u32,
    /// Bit 0: face highlighted (edit mode), bits 1..: silhouette mask value.
    flags: u8,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

#[derive(Clone, Copy)]
struct CVert {
    clip: Vec4,
    world: Vec3,
    normal: Vec3,
}

fn lerp_cv(a: &CVert, b: &CVert, t: f32) -> CVert {
    CVert {
        clip: a.clip.lerp(b.clip, t),
        world: a.world.lerp(b.world, t),
        normal: a.normal.lerp(b.normal, t),
    }
}

/// Frame-wide raster parameters.
pub(crate) struct Frame {
    pub w: usize,
    pub h: usize,
    pub view_proj: Mat4,
    pub camera: Camera,
    pub ss: f32,
}

impl Frame {
    /// Clip → pixel coordinates (x, y, ndc z, 1/w).
    pub(crate) fn to_screen(&self, c: Vec4) -> (f32, f32, f32, f32) {
        let iw = 1.0 / c.w;
        let n = c.xyz() * iw;
        (
            (n.x * 0.5 + 0.5) * self.w as f32,
            (0.5 - n.y * 0.5) * self.h as f32,
            n.z,
            iw,
        )
    }
}

fn setup_triangle(frame: &Frame, v: [CVert; 3], obj: u32, flags: u8, out: &mut Vec<RTri>) {
    // Trivial rejects: all behind near, all beyond far, all outside one side.
    if v.iter().all(|p| p.clip.z < 0.0) || v.iter().all(|p| p.clip.z > p.clip.w) {
        return;
    }
    for axis in 0..2 {
        if v.iter().all(|p| p.clip[axis] > p.clip.w) || v.iter().all(|p| p.clip[axis] < -p.clip.w) {
            return;
        }
    }
    // Clip against z >= 0 (Sutherland–Hodgman).
    let mut poly: [CVert; 4] = [v[0]; 4];
    let mut n = 0;
    for i in 0..3 {
        let a = &v[i];
        let b = &v[(i + 1) % 3];
        let (da, db) = (a.clip.z, b.clip.z);
        if da >= 0.0 {
            poly[n] = *a;
            n += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            poly[n] = lerp_cv(a, b, t);
            n += 1;
        }
    }
    for k in 1..n.saturating_sub(1) {
        let tri = [poly[0], poly[k], poly[k + 1]];
        let mut r = RTri {
            sx: [0.0; 3],
            sy: [0.0; 3],
            z: [0.0; 3],
            iw: [0.0; 3],
            world: [tri[0].world, tri[1].world, tri[2].world],
            normal: [tri[0].normal, tri[1].normal, tri[2].normal],
            obj,
            flags,
            min_x: 0.0,
            max_x: 0.0,
            min_y: 0.0,
            max_y: 0.0,
        };
        for (j, cv) in tri.iter().enumerate() {
            let (x, y, z, iw) = frame.to_screen(cv.clip);
            r.sx[j] = x;
            r.sy[j] = y;
            r.z[j] = z;
            r.iw[j] = iw;
        }
        let area = (r.sx[1] - r.sx[0]) as f64 * (r.sy[2] - r.sy[0]) as f64
            - (r.sy[1] - r.sy[0]) as f64 * (r.sx[2] - r.sx[0]) as f64;
        if area.is_nan() || area.abs() <= 1e-12 {
            continue;
        }
        r.min_x = r.sx.iter().copied().fold(f32::INFINITY, f32::min).max(0.0);
        r.max_x =
            r.sx.iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                .min(frame.w as f32);
        r.min_y = r.sy.iter().copied().fold(f32::INFINITY, f32::min).max(0.0);
        r.max_y =
            r.sy.iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                .min(frame.h as f32);
        if r.min_x >= r.max_x || r.min_y >= r.max_y {
            continue;
        }
        out.push(r);
    }
}

/// Edge-function barycentrics of pixel center (px, py); `None` when outside.
#[inline]
fn barycentric(t: &RTri, px: f64, py: f64, inv_area: f64) -> Option<[f64; 3]> {
    let (x0, y0) = (t.sx[0] as f64, t.sy[0] as f64);
    let (x1, y1) = (t.sx[1] as f64, t.sy[1] as f64);
    let (x2, y2) = (t.sx[2] as f64, t.sy[2] as f64);
    let w0 = ((x2 - x1) * (py - y1) - (y2 - y1) * (px - x1)) * inv_area;
    let w1 = ((x0 - x2) * (py - y2) - (y0 - y2) * (px - x2)) * inv_area;
    let w2 = 1.0 - w0 - w1;
    (w0 >= 0.0 && w1 >= 0.0 && w2 >= -1e-12).then_some([w0, w1, w2.max(0.0)])
}

#[inline]
fn inv_area(t: &RTri) -> f64 {
    let a = (t.sx[1] - t.sx[0]) as f64 * (t.sy[2] - t.sy[0]) as f64
        - (t.sy[1] - t.sy[0]) as f64 * (t.sx[2] - t.sx[0]) as f64;
    // Numerators flip sign with the winding too, so inside points get
    // positive weights for either orientation.
    1.0 / a
}

/// Full-resolution (supersampled) frame buffers.
pub(crate) struct Buffers {
    /// Premultiplied linear RGBA.
    pub color: Vec<[f32; 4]>,
    /// NDC depth (`INFINITY` = empty).
    pub depth: Vec<f32>,
    /// 1-based object index (0 = none).
    pub obj: Vec<u32>,
    /// Selection silhouette mask (0 none, 1 selected, 2 active).
    pub mask: Vec<u8>,
}

fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

fn rgb(c: [f32; 4]) -> Vec3 {
    Vec3::new(c[0], c[1], c[2])
}

/// Render pre-extracted draw data.
pub fn render(
    draw: &SceneDrawData,
    camera: &Camera,
    options: &RenderOptions,
) -> RenderResult<RgbaImage> {
    options.validate()?;
    camera.validate()?;
    let ss = options.supersample as usize;
    let (w, h) = (options.width as usize * ss, options.height as usize * ss);
    let mut camera = *camera;
    if let Some(b) = &draw.bounds {
        camera.fit_clip_to_bounds(b);
    }
    let frame = Frame {
        w,
        h,
        view_proj: camera.view_projection(options.aspect()),
        camera,
        ss: ss as f32,
    };
    let lights = LightRig::default().to_world(&camera);
    let show_sel = options.view.show_selection;

    // ---- triangle setup ----
    let mut tris: Vec<RTri> = Vec::with_capacity(draw.triangle_count());
    for (oi, obj) in draw.objects.iter().enumerate() {
        let g = &obj.geometry;
        let hl = &obj.highlight;
        let mask = if show_sel && hl.selected {
            if hl.active {
                2u8
            } else {
                1u8
            }
        } else {
            0
        };
        let faces = hl.edit.as_ref().filter(|_| show_sel).map(|e| &e.faces);
        let part: Vec<RTri> = (0..g.triangle_count())
            .into_par_iter()
            .fold(Vec::new, |mut acc, t| {
                let face_hl = faces.is_some_and(|f| f.contains(&g.tri_faces[t]));
                let flags = (face_hl as u8) | (mask << 1);
                let v = [0, 1, 2].map(|k| {
                    let p = g.positions[3 * t + k];
                    CVert {
                        clip: frame.view_proj * p.extend(1.0),
                        world: p,
                        normal: g.normals[3 * t + k],
                    }
                });
                setup_triangle(&frame, v, oi as u32, flags, &mut acc);
                acc
            })
            .reduce(Vec::new, |mut a, mut b| {
                a.append(&mut b);
                a
            });
        tris.extend(part);
    }

    // ---- binning into bands ----
    let nbands = h.div_ceil(BAND);
    let mut bins: Vec<Vec<u32>> = vec![Vec::new(); nbands];
    for (i, t) in tris.iter().enumerate() {
        let b0 = ((t.min_y - 0.5).max(0.0) as usize / BAND).min(nbands - 1);
        let b1 = (((t.max_y + 0.5) as usize) / BAND).min(nbands - 1);
        for bin in &mut bins[b0..=b1] {
            bin.push(i as u32);
        }
    }

    // ---- visibility + shading per band ----
    let bg = options.view.background;
    let bg_lin = [
        srgb_to_linear(bg[0]) * bg[3],
        srgb_to_linear(bg[1]) * bg[3],
        srgb_to_linear(bg[2]) * bg[3],
        bg[3],
    ];
    let surfaces: Vec<Surface> = draw
        .objects
        .iter()
        .map(|o| Surface::from(&o.material))
        .collect();
    let mut buf = Buffers {
        color: vec![bg_lin; w * h],
        depth: vec![f32::INFINITY; w * h],
        obj: vec![0; w * h],
        mask: vec![0; w * h],
    };
    let grid_params = grid::GridParams::new(&camera, frame.ss);
    let ground = if options.view.grid {
        grid::GroundMap::new(&frame.view_proj, w as u32, h as u32)
    } else {
        None
    };
    let tint = style::OBJECT_TINT;
    let face_tint = style::FACE_SELECTED;
    let eye = camera.eye;
    let ortho = camera.is_orthographic();
    let fwd = camera.forward();

    buf.color
        .par_chunks_mut(w * BAND)
        .zip(buf.depth.par_chunks_mut(w * BAND))
        .zip(buf.obj.par_chunks_mut(w * BAND))
        .zip(buf.mask.par_chunks_mut(w * BAND))
        .zip(bins.par_iter())
        .enumerate()
        .for_each(|(band, ((((color, depth), objb), mask), bin))| {
            let y0 = band * BAND;
            let rows = color.len() / w;
            let mut tri_id = vec![u32::MAX; rows * w];
            for &ti in bin {
                let t = &tris[ti as usize];
                let ia = inv_area(t);
                let ys = ((t.min_y - 0.5).floor().max(y0 as f32) as usize).max(y0);
                let ye = ((t.max_y + 0.5).ceil() as usize).min(y0 + rows);
                let xs = (t.min_x - 0.5).floor().max(0.0) as usize;
                let xe = ((t.max_x + 0.5).ceil() as usize).min(w);
                let m = t.flags >> 1;
                for y in ys..ye {
                    let py = y as f64 + 0.5;
                    let row = (y - y0) * w;
                    for x in xs..xe {
                        let Some(b) = barycentric(t, x as f64 + 0.5, py, ia) else {
                            continue;
                        };
                        let z = (b[0] * t.z[0] as f64 + b[1] * t.z[1] as f64 + b[2] * t.z[2] as f64)
                            as f32;
                        if !(0.0..=1.0).contains(&z) {
                            continue;
                        }
                        let i = row + x;
                        if m > mask[i] {
                            mask[i] = m;
                        }
                        if z < depth[i] {
                            depth[i] = z;
                            tri_id[i] = ti;
                        }
                    }
                }
            }
            // Deferred shading.
            for r in 0..rows {
                let y = y0 + r;
                for x in 0..w {
                    let i = r * w + x;
                    let ti = tri_id[i];
                    if ti != u32::MAX {
                        let t = &tris[ti as usize];
                        let b = barycentric(t, x as f64 + 0.5, y as f64 + 0.5, inv_area(t))
                            .unwrap_or([1.0 / 3.0; 3]);
                        let pw = [
                            b[0] as f32 * t.iw[0],
                            b[1] as f32 * t.iw[1],
                            b[2] as f32 * t.iw[2],
                        ];
                        let s = (pw[0] + pw[1] + pw[2]).max(1e-30);
                        let pw = pw.map(|p| p / s);
                        let pos = t.world[0] * pw[0] + t.world[1] * pw[1] + t.world[2] * pw[2];
                        let mut n =
                            (t.normal[0] * pw[0] + t.normal[1] * pw[1] + t.normal[2] * pw[2])
                                .normalize_or(Vec3::Z);
                        let v = if ortho {
                            -fwd
                        } else {
                            (eye - pos).normalize_or(-fwd)
                        };
                        if n.dot(v) < 0.0 {
                            n = -n;
                        }
                        let o = &draw.objects[t.obj as usize];
                        let mut c = shade(&surfaces[t.obj as usize], n, v, &lights);
                        if show_sel && o.highlight.selected {
                            c = mix3(c, rgb(tint), tint[3]);
                        }
                        if t.flags & 1 != 0 {
                            c = mix3(c, rgb(face_tint), face_tint[3]);
                        }
                        color[i] = [c.x, c.y, c.z, 1.0];
                        objb[i] = t.obj + 1;
                    }
                    if let Some(g) = &ground {
                        if let Some((gc, gz)) =
                            g.shade(&grid_params, x as f32 + 0.5, y as f32 + 0.5)
                        {
                            if gz < depth[i] {
                                let a = gc[3];
                                let d = &mut color[i];
                                for k in 0..3 {
                                    d[k] = gc[k] + d[k] * (1.0 - a);
                                }
                                d[3] = a + d[3] * (1.0 - a);
                            }
                        }
                    }
                }
            }
        });

    // ---- overlays ----
    overlay::draw_overlays(&frame, draw, &options.view, &mut buf);
    if show_sel {
        overlay::draw_outline(&frame, &mut buf);
    }

    Ok(resolve(&buf, options.width, options.height, ss))
}

/// Box-filter the supersampled premultiplied buffer into an sRGB RGBA8 image.
fn resolve(buf: &Buffers, out_w: u32, out_h: u32, ss: usize) -> RgbaImage {
    let lut = SrgbLut::new();
    let (ow, oh) = (out_w as usize, out_h as usize);
    let w = ow * ss;
    let mut pixels = vec![0u8; ow * oh * 4];
    let inv = 1.0 / (ss * ss) as f32;
    pixels
        .par_chunks_mut(ow * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..ow {
                let mut acc = [0.0f32; 4];
                for sy in 0..ss {
                    let base = (y * ss + sy) * w + x * ss;
                    for c in &buf.color[base..base + ss] {
                        for k in 0..4 {
                            acc[k] += c[k];
                        }
                    }
                }
                let a = acc[3] * inv;
                let un = if a > 1e-6 { inv / a } else { 0.0 };
                let px = &mut row[x * 4..x * 4 + 4];
                px[0] = lut.encode(acc[0] * un);
                px[1] = lut.encode(acc[1] * un);
                px[2] = lut.encode(acc[2] * un);
                px[3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
    RgbaImage {
        width: out_w,
        height: out_h,
        pixels,
    }
}

/// Extract and render a scene.
pub fn render_scene(
    scene: &Scene,
    selection: Option<&Selection>,
    camera: &Camera,
    options: &RenderOptions,
) -> RenderResult<RgbaImage> {
    let draw = SceneDrawData::extract(scene, selection, options.view.smooth_shading);
    render(&draw, camera, options)
}

//! Software overlays: wireframe, selected edges, vertex points and the
//! selection outline.

use glam::{Vec2, Vec3, Vec4};
use rayon::prelude::*;

use super::{Buffers, Frame};
use crate::extract::SceneDrawData;
use crate::options::{style, ViewOptions};
use forge3d_core::selection::SelectMode;

#[inline]
fn blend(dst: &mut [f32; 4], c: [f32; 4], cov: f32) {
    let a = (c[3] * cov).clamp(0.0, 1.0);
    for k in 0..3 {
        dst[k] = c[k] * a + dst[k] * (1.0 - a);
    }
    dst[3] = a + dst[3] * (1.0 - a);
}

/// Whether an overlay sample at NDC depth `z` passes the biased depth test
/// against the stored surface depth.
#[inline]
fn visible(frame: &Frame, z: f32, stored: f32) -> bool {
    if !stored.is_finite() {
        return true;
    }
    let cam = &frame.camera;
    let dl = cam.linear_depth(z);
    let ds = cam.linear_depth(stored);
    let range = cam.far - cam.near;
    dl * (1.0 - style::LINE_DEPTH_BIAS * 1.5) <= ds + range * 1e-6
}

/// Clip a segment against z >= 0 in clip space.
fn clip_segment(mut a: Vec4, mut b: Vec4) -> Option<(Vec4, Vec4)> {
    if a.z < 0.0 && b.z < 0.0 {
        return None;
    }
    if a.z < 0.0 {
        a = a.lerp(b, a.z / (a.z - b.z));
    } else if b.z < 0.0 {
        b = b.lerp(a, b.z / (b.z - a.z));
    }
    Some((a, b))
}

/// Draw an anti-aliased line of `width` output pixels between world points.
fn draw_line(frame: &Frame, buf: &mut Buffers, p0: Vec3, p1: Vec3, color: [f32; 4], width: f32) {
    let Some((c0, c1)) = clip_segment(
        frame.view_proj * p0.extend(1.0),
        frame.view_proj * p1.extend(1.0),
    ) else {
        return;
    };
    let (x0, y0, z0, _) = frame.to_screen(c0);
    let (x1, y1, z1, _) = frame.to_screen(c1);
    let (a, b) = (Vec2::new(x0, y0), Vec2::new(x1, y1));
    if !(a.is_finite() && b.is_finite()) {
        return;
    }
    let hw = width * frame.ss * 0.5;
    let reach = hw + 1.0;
    let (w, h) = (frame.w as f32, frame.h as f32);
    let d = b - a;
    let len2 = d.length_squared();
    let steep = d.y.abs() > d.x.abs();
    // Iterate along the major axis; across it cover the line's footprint.
    let (ma, mb, na, nb) = if steep {
        (a.y, b.y, a.x, b.x)
    } else {
        (a.x, b.x, a.y, b.y)
    };
    let (ma, mb, na, nb) = if ma <= mb {
        (ma, mb, na, nb)
    } else {
        (mb, ma, nb, na)
    };
    let major_lim = if steep { h } else { w };
    let minor_lim = if steep { w } else { h };
    let start = (ma - reach).floor().max(0.0) as i64;
    let end = ((mb + reach).ceil().min(major_lim)) as i64;
    let slope = if (mb - ma).abs() > 1e-9 {
        (nb - na) / (mb - ma)
    } else {
        0.0
    };
    let across = reach * (1.0 + slope * slope).sqrt() + 1.0;
    for i in start..end {
        let mc = i as f32 + 0.5;
        let nc = na + (mc.clamp(ma, mb) - ma) * slope;
        let n0 = (nc - across).floor().max(0.0) as i64;
        let n1 = ((nc + across).ceil().min(minor_lim)) as i64;
        for j in n0..n1 {
            let (px, py) = if steep { (j, i) } else { (i, j) };
            let p = Vec2::new(px as f32 + 0.5, py as f32 + 0.5);
            let t = if len2 > 0.0 {
                ((p - a).dot(d) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let dist = p.distance(a + d * t);
            let cov = (hw + 0.5 - dist).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let idx = py as usize * frame.w + px as usize;
            let z = z0 + (z1 - z0) * t;
            if visible(frame, z, buf.depth[idx]) {
                blend(&mut buf.color[idx], color, cov);
            }
        }
    }
}

/// Draw a square point of `size` output pixels.
fn draw_point(frame: &Frame, buf: &mut Buffers, p: Vec3, color: [f32; 4], size: f32) {
    let c = frame.view_proj * p.extend(1.0);
    if c.z < 0.0 || c.z > c.w || c.w <= 0.0 {
        return;
    }
    let (x, y, z, _) = frame.to_screen(c);
    let half = size * frame.ss * 0.5;
    let x0 = (x - half).round().max(0.0) as i64;
    let x1 = ((x + half).round() as i64).min(frame.w as i64);
    let y0 = (y - half).round().max(0.0) as i64;
    let y1 = ((y + half).round() as i64).min(frame.h as i64);
    let (cx, cy) = (x as i64, y as i64);
    if cx < 0 || cy < 0 || cx >= frame.w as i64 || cy >= frame.h as i64 {
        return;
    }
    // Depth-test once at the center so points are either shown or hidden whole.
    let center_depth = buf.depth[cy as usize * frame.w + cx as usize];
    if !visible(frame, z, center_depth) {
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            blend(
                &mut buf.color[py as usize * frame.w + px as usize],
                color,
                1.0,
            );
        }
    }
}

/// Wireframe, selected edges and vertex points.
pub(crate) fn draw_overlays(
    frame: &Frame,
    draw: &SceneDrawData,
    view: &ViewOptions,
    buf: &mut Buffers,
) {
    for obj in &draw.objects {
        let g = &obj.geometry;
        let edit = obj.highlight.edit.as_ref().filter(|_| view.show_selection);
        if view.wireframe || edit.is_some() {
            let color = if edit.is_some() {
                style::WIRE_EDIT
            } else {
                style::WIRE
            };
            for (ei, &[a, b]) in g.edges.iter().enumerate() {
                if edit.is_some_and(|e| e.edges.contains(&(ei as u32))) {
                    continue;
                }
                draw_line(
                    frame,
                    buf,
                    g.vertices[a as usize],
                    g.vertices[b as usize],
                    color,
                    style::WIRE_WIDTH,
                );
            }
        }
        let Some(e) = edit else { continue };
        for &ei in &e.edges {
            if let Some(&[a, b]) = g.edges.get(ei as usize) {
                draw_line(
                    frame,
                    buf,
                    g.vertices[a as usize],
                    g.vertices[b as usize],
                    style::ELEMENT_SELECTED,
                    style::SELECTED_EDGE_WIDTH,
                );
            }
        }
        if e.mode == SelectMode::Vertex {
            for (vi, &p) in g.vertices.iter().enumerate() {
                if !e.vertices.contains(&(vi as u32)) {
                    draw_point(frame, buf, p, style::VERTEX, style::VERTEX_SIZE);
                }
            }
            for &vi in &e.vertices {
                if let Some(&p) = g.vertices.get(vi as usize) {
                    draw_point(
                        frame,
                        buf,
                        p,
                        style::ELEMENT_SELECTED,
                        style::SELECTED_VERTEX_SIZE,
                    );
                }
            }
        }
    }
}

/// Outline around the silhouettes of selected objects (from the mask).
pub(crate) fn draw_outline(frame: &Frame, buf: &mut Buffers) {
    if !buf.mask.iter().any(|&m| m > 0) {
        return;
    }
    let (w, h) = (frame.w as i64, frame.h as i64);
    let r = (style::OUTLINE_WIDTH * frame.ss).max(1.0);
    let ri = r.ceil() as i64;
    let mask = &buf.mask;
    let row_has: Vec<bool> = mask
        .chunks(frame.w)
        .map(|r| r.iter().any(|&m| m > 0))
        .collect();
    buf.color
        .par_chunks_mut(frame.w)
        .enumerate()
        .for_each(|(y, row)| {
            let y = y as i64;
            let near_mask = (y - ri).max(0)..=(y + ri).min(h - 1);
            if !near_mask.into_iter().any(|yy| row_has[yy as usize]) {
                return;
            }
            for x in 0..w {
                if mask[(y * w + x) as usize] != 0 {
                    continue;
                }
                let mut best = 0u8;
                let mut best_d = f32::INFINITY;
                for dy in -ri..=ri {
                    let yy = y + dy;
                    if yy < 0 || yy >= h {
                        continue;
                    }
                    for dx in -ri..=ri {
                        let xx = x + dx;
                        if xx < 0 || xx >= w {
                            continue;
                        }
                        let m = mask[(yy * w + xx) as usize];
                        if m == 0 {
                            continue;
                        }
                        let d = ((dx * dx + dy * dy) as f32).sqrt();
                        if d > r + 0.5 {
                            continue;
                        }
                        if m > best {
                            best = m;
                            best_d = d;
                        } else if m == best && d < best_d {
                            best_d = d;
                        }
                    }
                }
                if best > 0 {
                    let color = if best >= 2 {
                        style::OUTLINE_ACTIVE
                    } else {
                        style::OUTLINE_SELECTED
                    };
                    let cov = (r + 0.5 - best_d).clamp(0.0, 1.0);
                    blend(&mut row[x as usize], color, cov);
                }
            }
        });
}

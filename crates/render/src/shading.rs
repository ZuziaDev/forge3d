//! The shading model shared by the software rasterizer and the WGSL shaders:
//! PBR metallic-roughness (GGX / Smith-Schlick / Schlick Cook-Torrance) with
//! one view-relative key light, a fill light, a sky/ground hemisphere ambient
//! and a cheap environment specular term. All values are linear RGB.

use std::f32::consts::PI;

use forge3d_core::scene::Material;
use glam::Vec3;

use crate::camera::Camera;

/// Light setup in view space (directions point *towards* the light, camera
/// looks down −Z, +Y up, +X right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightRig {
    pub key_dir_view: Vec3,
    pub key_color: Vec3,
    pub fill_dir_view: Vec3,
    pub fill_color: Vec3,
    pub sky: Vec3,
    pub ground: Vec3,
    /// Strength of the environment specular term.
    pub env_specular: f32,
}

impl Default for LightRig {
    fn default() -> Self {
        LightRig {
            key_dir_view: Vec3::new(-0.45, 0.65, 0.62).normalize(),
            key_color: Vec3::splat(2.9),
            fill_dir_view: Vec3::new(0.7, -0.15, 0.7).normalize(),
            fill_color: Vec3::splat(0.75),
            sky: Vec3::new(0.30, 0.32, 0.36),
            ground: Vec3::new(0.12, 0.11, 0.10),
            env_specular: 1.0,
        }
    }
}

/// A light rig resolved to world space for a camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldLights {
    pub key_dir: Vec3,
    pub key_color: Vec3,
    pub fill_dir: Vec3,
    pub fill_color: Vec3,
    pub sky: Vec3,
    pub ground: Vec3,
    pub env_specular: f32,
}

impl LightRig {
    /// Rotate the view-space light directions into world space for `camera`.
    pub fn to_world(&self, camera: &Camera) -> WorldLights {
        let (r, u, f) = camera.basis();
        let to_world = |d: Vec3| (r * d.x + u * d.y - f * d.z).normalize_or(Vec3::Z);
        WorldLights {
            key_dir: to_world(self.key_dir_view),
            key_color: self.key_color,
            fill_dir: to_world(self.fill_dir_view),
            fill_color: self.fill_color,
            sky: self.sky,
            ground: self.ground,
            env_specular: self.env_specular,
        }
    }
}

/// Surface parameters for [`shade`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub base_color: Vec3,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Vec3,
}

impl From<&Material> for Surface {
    fn from(m: &Material) -> Self {
        Surface {
            base_color: Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2])
                .clamp(Vec3::ZERO, Vec3::ONE),
            metallic: m.metallic.clamp(0.0, 1.0),
            roughness: m.roughness.clamp(0.0, 1.0),
            emissive: Vec3::from(m.emissive).max(Vec3::ZERO),
        }
    }
}

fn direct(s: &Surface, f0: Vec3, diffuse: Vec3, n: Vec3, v: Vec3, l: Vec3, c: Vec3) -> Vec3 {
    let ndl = n.dot(l);
    if ndl <= 0.0 {
        return Vec3::ZERO;
    }
    let rough = s.roughness.clamp(0.04, 1.0);
    let a = rough * rough;
    let a2 = a * a;
    let h = (v + l).normalize_or(n);
    let ndh = n.dot(h).max(0.0);
    let ndv = n.dot(v).max(1e-4);
    let vdh = v.dot(h).max(0.0);
    let dd = ndh * ndh * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * dd * dd);
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let g = (ndv / (ndv * (1.0 - k) + k)) * (ndl / (ndl * (1.0 - k) + k));
    let fresnel = f0 + (Vec3::ONE - f0) * (1.0 - vdh).powi(5);
    let spec = fresnel * (d * g / (4.0 * ndv * ndl + 1e-4));
    let kd = Vec3::ONE - fresnel;
    (kd * diffuse / PI + spec) * c * ndl
}

/// Shade a surface point with unit normal `n` (facing the viewer) and unit
/// direction to the viewer `v`. Returns linear RGB (not clamped).
pub fn shade(s: &Surface, n: Vec3, v: Vec3, lights: &WorldLights) -> Vec3 {
    let f0 = Vec3::splat(0.04).lerp(s.base_color, s.metallic);
    let diffuse = s.base_color * (1.0 - s.metallic);
    let mut color = direct(s, f0, diffuse, n, v, lights.key_dir, lights.key_color)
        + direct(s, f0, diffuse, n, v, lights.fill_dir, lights.fill_color);
    // Hemisphere ambient.
    let hemi = lights.ground.lerp(lights.sky, n.z * 0.5 + 0.5);
    color += diffuse * hemi;
    // Environment specular (roughness-aware Schlick, reflected sky/ground).
    let ndv = n.dot(v).max(0.0);
    let f_amb = f0 + (Vec3::splat(1.0 - s.roughness).max(f0) - f0) * (1.0 - ndv).powi(5);
    let r = (n * (2.0 * n.dot(v)) - v).normalize_or(n);
    let t = smoothstep(-0.2, 0.2, r.z);
    let env = lights.ground.lerp(lights.sky, t) * lights.env_specular;
    color += f_amb * env * (1.0 - 0.5 * s.roughness);
    color + s.emissive
}

/// Hermite smoothstep.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear → sRGB transfer function (input clamped to 0..=1).
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → linear transfer function.
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Lookup table: 8-bit sRGB code for 4096 linear steps (fast encode).
pub(crate) struct SrgbLut([u8; 4097]);

impl SrgbLut {
    pub(crate) fn new() -> Self {
        let mut t = [0u8; 4097];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (linear_to_srgb(i as f32 / 4096.0) * 255.0 + 0.5) as u8;
        }
        SrgbLut(t)
    }
    #[inline]
    pub(crate) fn encode(&self, c: f32) -> u8 {
        let i = (c.clamp(0.0, 1.0) * 4096.0 + 0.5) as usize;
        self.0[i.min(4096)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            assert!((srgb_to_linear(linear_to_srgb(x)) - x).abs() < 1e-4);
        }
        let lut = SrgbLut::new();
        assert_eq!(lut.encode(0.0), 0);
        assert_eq!(lut.encode(1.0), 255);
        assert_eq!(lut.encode(0.214), 128);
    }

    #[test]
    fn lit_face_brighter_than_unlit() {
        let cam = Camera::default();
        let lights = LightRig::default().to_world(&cam);
        let s = Surface::from(&Material::default());
        let v = -cam.forward();
        let towards = shade(&s, lights.key_dir, v, &lights);
        let away = shade(&s, -lights.key_dir, v, &lights);
        assert!(towards.x > away.x);
        assert!(towards.is_finite());
    }
}

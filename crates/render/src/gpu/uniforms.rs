//! Rust mirrors of the WGSL uniform structs and vertex formats
//! (`shaders/common.wgsl`, `mesh.wgsl`, `lines.wgsl`).

use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Mat4, Vec3};

use crate::camera::{Camera, Projection};
use crate::extract::normal_matrix;
use crate::options::style;
use crate::shading::LightRig;

/// `struct Frame` in common.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub viewport: [f32; 4],
    pub key_dir: [f32; 4],
    pub key_color: [f32; 4],
    pub fill_dir: [f32; 4],
    pub fill_color: [f32; 4],
    pub sky: [f32; 4],
    pub ground: [f32; 4],
    pub grid: [f32; 4],
    pub grid_center: [f32; 4],
    pub grid_minor: [f32; 4],
    pub grid_major: [f32; 4],
    pub axis_x: [f32; 4],
    pub axis_y: [f32; 4],
    pub face_sel: [f32; 4],
    pub obj_tint: [f32; 4],
    pub outline_sel: [f32; 4],
    pub outline_active: [f32; 4],
    pub params: [f32; 4],
}

fn v4(v: Vec3, w: f32) -> [f32; 4] {
    [v.x, v.y, v.z, w]
}

impl FrameUniform {
    /// Frame constants for a camera and viewport. `px_scale` scales pixel
    /// sizes (line widths, outline, grid derivatives) for supersampled
    /// rendering; `encode_srgb` is set for non-sRGB render targets.
    pub(crate) fn new(
        camera: &Camera,
        width: u32,
        height: u32,
        px_scale: f32,
        encode_srgb: bool,
    ) -> FrameUniform {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let vp = camera.view_projection(w / h);
        let lights = LightRig::default().to_world(camera);
        let scale = match camera.projection {
            Projection::Perspective { .. } => camera.distance(),
            Projection::Orthographic { height } => height,
        };
        let ortho = camera.is_orthographic();
        let center = if ortho { camera.target } else { camera.eye };
        FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            eye: v4(camera.eye, if ortho { 1.0 } else { 0.0 }),
            forward: v4(camera.forward(), style::LINE_DEPTH_BIAS),
            viewport: [w, h, 1.0 / w, 1.0 / h],
            key_dir: v4(lights.key_dir, 0.0),
            key_color: v4(lights.key_color, 0.0),
            fill_dir: v4(lights.fill_dir, 0.0),
            fill_color: v4(lights.fill_color, 0.0),
            sky: v4(lights.sky, lights.env_specular),
            ground: v4(lights.ground, 0.0),
            grid: [
                camera.grid_cell_size(),
                style::GRID_MAJOR_EVERY,
                scale * style::GRID_FADE_START,
                scale * style::GRID_FADE_END,
            ],
            grid_center: v4(center, px_scale),
            grid_minor: style::GRID_MINOR,
            grid_major: style::GRID_MAJOR,
            axis_x: style::AXIS_X,
            axis_y: style::AXIS_Y,
            face_sel: style::FACE_SELECTED,
            obj_tint: style::OBJECT_TINT,
            outline_sel: style::OUTLINE_SELECTED,
            outline_active: style::OUTLINE_ACTIVE,
            params: [
                style::OUTLINE_WIDTH * px_scale,
                if encode_srgb { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        }
    }
}

/// `struct Object` in common.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ObjectUniform {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub pbr: [f32; 4],
    pub emissive: [f32; 4],
    pub wire: [f32; 4],
    pub ids: [u32; 4],
}

impl ObjectUniform {
    pub(crate) fn new(
        world: &Affine3A,
        material: &forge3d_core::scene::Material,
        pick_id: u32,
        mask: u8,
        tint: bool,
        edit: bool,
    ) -> ObjectUniform {
        let nm = normal_matrix(world);
        let wire = if edit { style::WIRE_EDIT } else { style::WIRE };
        ObjectUniform {
            model: Mat4::from(*world).to_cols_array_2d(),
            normal_mat: Mat4::from_mat3(nm).to_cols_array_2d(),
            base_color: material.base_color,
            pbr: [
                material.metallic,
                material.roughness,
                if tint { style::OBJECT_TINT[3] } else { 0.0 },
                0.0,
            ],
            emissive: [
                material.emissive[0],
                material.emissive[1],
                material.emissive[2],
                0.0,
            ],
            wire,
            ids: [pick_id, mask as u32, 0, 0],
        }
    }
}

/// Mesh vertex: local position, normal, source face.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub face: u32,
}

pub(crate) const MESH_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];

/// Line / point instance (local space).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct LineInstance {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [u8; 4],
    /// width px, kind (0 line / 1 point), use the object's wire color, unused
    pub params: [f32; 4],
}

pub(crate) const LINE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32x4];

/// Linear RGBA float color → unorm bytes (the shader reads them back as linear).
pub(crate) fn color_bytes(c: [f32; 4]) -> [u8; 4] {
    c.map(|x| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

impl LineInstance {
    pub(crate) fn line(a: Vec3, b: Vec3, color: [f32; 4], width: f32, object_wire: bool) -> Self {
        LineInstance {
            a: a.to_array(),
            b: b.to_array(),
            color: color_bytes(color),
            params: [width, 0.0, if object_wire { 1.0 } else { 0.0 }, 0.0],
        }
    }
    pub(crate) fn point(p: Vec3, color: [f32; 4], size: f32) -> Self {
        LineInstance {
            a: p.to_array(),
            b: p.to_array(),
            color: color_bytes(color),
            params: [size, 1.0, 0.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_sizes_match_wgsl() {
        // 2 mat4 + 20 vec4
        assert_eq!(std::mem::size_of::<FrameUniform>(), 2 * 64 + 20 * 16);
        // 2 mat4 + 5 vec4
        assert_eq!(std::mem::size_of::<ObjectUniform>(), 2 * 64 + 5 * 16);
        assert_eq!(std::mem::size_of::<MeshVertex>(), 28);
        assert_eq!(std::mem::size_of::<LineInstance>(), 44);
    }
}

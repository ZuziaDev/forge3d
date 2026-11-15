// Triangle passes: shaded surface, selected-face highlight, ID buffer and
// selection silhouette mask.

@group(1) @binding(0) var<uniform> obj: Object;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
};

fn transform_vertex(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = (obj.model * vec4<f32>(v.pos, 1.0)).xyz;
    out.world = world;
    out.normal = (obj.normal_mat * vec4<f32>(v.normal, 0.0)).xyz;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.face = v.face;
    return out;
}

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    return transform_vertex(v);
}

// Same as vs_main but depth-biased towards the camera (face highlight overlay).
@vertex
fn vs_overlay(v: VertexIn) -> VertexOut {
    var out = transform_vertex(v);
    out.clip = frame.view_proj * vec4<f32>(depth_biased(out.world), 1.0);
    return out;
}

@fragment
fn fs_shade(in: VertexOut) -> @location(0) vec4<f32> {
    var n = in.normal;
    if length(n) < 1e-12 {
        n = vec3<f32>(0.0, 0.0, 1.0);
    }
    n = normalize(n);
    let v = view_dir(in.world);
    if dot(n, v) < 0.0 {
        n = -n;
    }
    var c = shade(clamp(obj.base_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)),
                  clamp(obj.pbr.x, 0.0, 1.0), clamp(obj.pbr.y, 0.0, 1.0),
                  max(obj.emissive.rgb, vec3<f32>(0.0)), n, v);
    c = mix(c, frame.obj_tint.rgb, obj.pbr.z);
    return output_color(c, 1.0);
}

@fragment
fn fs_highlight(in: VertexOut) -> @location(0) vec4<f32> {
    return output_color(frame.face_sel.rgb, frame.face_sel.a);
}

@fragment
fn fs_id(in: VertexOut) -> @location(0) vec4<u32> {
    return vec4<u32>(obj.ids.x, in.face, bitcast<u32>(in.clip.z), 1u);
}

@fragment
fn fs_mask(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(obj.ids.y) * 0.5, 0.0, 0.0, 1.0);
}

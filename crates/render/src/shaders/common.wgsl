// Shared declarations of the forge3d viewport shaders. Kept in sync with
// `gpu::uniforms` (Rust layout) and `shading.rs` / `soft::grid` (CPU model).

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz: eye, w: 1 = orthographic
    eye: vec4<f32>,
    // xyz: unit view direction, w: overlay depth bias fraction
    forward: vec4<f32>,
    // width, height, 1/width, 1/height (pixels)
    viewport: vec4<f32>,
    key_dir: vec4<f32>,
    key_color: vec4<f32>,
    fill_dir: vec4<f32>,
    fill_color: vec4<f32>,
    // rgb: sky ambient, w: environment specular strength
    sky: vec4<f32>,
    ground: vec4<f32>,
    // cell size, major every, fade start, fade end
    grid: vec4<f32>,
    // xyz: fade center, w: derivative scale (supersampling factor)
    grid_center: vec4<f32>,
    grid_minor: vec4<f32>,
    grid_major: vec4<f32>,
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    face_sel: vec4<f32>,
    obj_tint: vec4<f32>,
    outline_sel: vec4<f32>,
    outline_active: vec4<f32>,
    // x: outline width (px), y: 1 = encode sRGB in the shader, zw: unused
    params: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    base_color: vec4<f32>,
    // metallic, roughness, selection tint amount, unused
    pbr: vec4<f32>,
    emissive: vec4<f32>,
    wire: vec4<f32>,
    // pick id (1-based), silhouette mask (0/1/2), unused, unused
    ids: vec4<u32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

const PI: f32 = 3.14159265358979;

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// Final color for the target: shaders work in linear space; non-sRGB
// targets get the transfer function applied here.
fn output_color(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    var c = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if frame.params.y > 0.5 {
        c = linear_to_srgb(c);
    }
    return vec4<f32>(c, clamp(a, 0.0, 1.0));
}

fn direct_light(base: vec3<f32>, metallic: f32, roughness: f32, f0: vec3<f32>, diffuse: vec3<f32>,
                n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let ndl = dot(n, l);
    if ndl <= 0.0 {
        return vec3<f32>(0.0);
    }
    let rough = clamp(roughness, 0.04, 1.0);
    let a = rough * rough;
    let a2 = a * a;
    var h = v + l;
    if length(h) < 1e-6 {
        h = n;
    }
    h = normalize(h);
    let ndh = max(dot(n, h), 0.0);
    let ndv = max(dot(n, v), 1e-4);
    let vdh = max(dot(v, h), 0.0);
    let dd = ndh * ndh * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * dd * dd);
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let g = (ndv / (ndv * (1.0 - k) + k)) * (ndl / (ndl * (1.0 - k) + k));
    let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vdh, 5.0);
    let spec = fresnel * (d * g / (4.0 * ndv * ndl + 1e-4));
    let kd = vec3<f32>(1.0) - fresnel;
    return (kd * diffuse / PI + spec) * c * ndl;
}

// Same model as `shading::shade` on the CPU.
fn shade(base: vec3<f32>, metallic: f32, roughness: f32, emissive: vec3<f32>,
         n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let diffuse = base * (1.0 - metallic);
    var color = direct_light(base, metallic, roughness, f0, diffuse, n, v, frame.key_dir.xyz, frame.key_color.xyz)
        + direct_light(base, metallic, roughness, f0, diffuse, n, v, frame.fill_dir.xyz, frame.fill_color.xyz);
    let hemi = mix(frame.ground.xyz, frame.sky.xyz, n.z * 0.5 + 0.5);
    color += diffuse * hemi;
    let ndv = max(dot(n, v), 0.0);
    let f_amb = f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * pow(1.0 - ndv, 5.0);
    var r = n * (2.0 * dot(n, v)) - v;
    if length(r) < 1e-6 {
        r = n;
    }
    r = normalize(r);
    let t = smoothstep(-0.2, 0.2, r.z);
    let env = mix(frame.ground.xyz, frame.sky.xyz, t) * frame.sky.w;
    color += f_amb * env * (1.0 - 0.5 * roughness);
    return color + emissive;
}

// Direction from a world point towards the viewer.
fn view_dir(world: vec3<f32>) -> vec3<f32> {
    if frame.eye.w > 0.5 {
        return -frame.forward.xyz;
    }
    let d = frame.eye.xyz - world;
    if length(d) < 1e-12 {
        return -frame.forward.xyz;
    }
    return normalize(d);
}

// Pull an overlay point towards the camera by a fraction of its view depth
// (keeps its screen position) so it wins the depth test against its faces.
fn depth_biased(world: vec3<f32>) -> vec3<f32> {
    let bias = frame.forward.w;
    if frame.eye.w > 0.5 {
        let depth = dot(world - frame.eye.xyz, frame.forward.xyz);
        return world - frame.forward.xyz * (depth * bias);
    }
    return world + (frame.eye.xyz - world) * bias;
}

struct FullscreenOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FullscreenOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    let p = uv * 2.0 - vec2<f32>(1.0);
    var out: FullscreenOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// Screen-space expanded, anti-aliased lines and square points, drawn as
// instanced quads (6 vertices per instance).

@group(1) @binding(0) var<uniform> obj: Object;

struct LineIn {
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
    // width / size in px, kind (0 line, 1 point), 1 = use the object's wire color, unused
    @location(3) params: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) dist: f32,
    @location(2) half_width: f32,
};

fn corner(vi: u32) -> vec2<f32> {
    // (end 0/1, side -1/+1) of the two triangles of a quad.
    switch vi % 6u {
        case 0u: { return vec2<f32>(0.0, -1.0); }
        case 1u: { return vec2<f32>(1.0, -1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        case 3u: { return vec2<f32>(0.0, -1.0); }
        case 4u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

fn to_clip(p: vec3<f32>) -> vec4<f32> {
    let world = (obj.model * vec4<f32>(p, 1.0)).xyz;
    return frame.view_proj * vec4<f32>(depth_biased(world), 1.0);
}

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var out: LineOut;
    out.color = select(v.color, obj.wire, v.params.z > 0.5);
    let k = corner(v.vi);
    let px_to_ndc = 2.0 * frame.viewport.zw;
    let half_w = v.params.x * frame.grid_center.w * 0.5;
    if v.params.y > 0.5 {
        // Square point.
        let c = to_clip(v.a);
        let off = vec2<f32>(k.x * 2.0 - 1.0, k.y) * half_w;
        out.clip = vec4<f32>(c.xy + off * px_to_ndc * c.w, c.z, c.w);
        out.dist = 0.0;
        out.half_width = 1e6;
        return out;
    }
    var ca = to_clip(v.a);
    var cb = to_clip(v.b);
    if ca.z < 0.0 && cb.z < 0.0 {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.dist = 0.0;
        out.half_width = 0.0;
        return out;
    }
    if ca.z < 0.0 {
        ca = mix(ca, cb, ca.z / (ca.z - cb.z));
    } else if cb.z < 0.0 {
        cb = mix(cb, ca, cb.z / (cb.z - ca.z));
    }
    let sa = ca.xy / ca.w * frame.viewport.xy * 0.5;
    let sb = cb.xy / cb.w * frame.viewport.xy * 0.5;
    var dir = sb - sa;
    if length(dir) < 1e-6 {
        dir = vec2<f32>(1.0, 0.0);
    }
    dir = normalize(dir);
    let nrm = vec2<f32>(-dir.y, dir.x);
    let reach = half_w + 1.0;
    let c = select(ca, cb, k.x > 0.5);
    let along = select(-1.0, 1.0, k.x > 0.5);
    let off = nrm * (k.y * reach) + dir * (along * reach);
    out.clip = vec4<f32>(c.xy + off * px_to_ndc * c.w, c.z, c.w);
    out.dist = k.y * reach;
    out.half_width = half_w;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    let cov = clamp(in.half_width + 0.5 - abs(in.dist), 0.0, 1.0);
    if cov <= 0.0 {
        discard;
    }
    return output_color(in.color.rgb, in.color.a * cov);
}

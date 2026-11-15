// Fullscreen analytic ground grid on z = 0 (same function as soft::grid).

fn line_cov(g: f32, fw: f32) -> f32 {
    let d = abs(fract(g - 0.5) - 0.5) / max(fw, 1e-6);
    return clamp(1.0 - d, 0.0, 1.0);
}

fn over(dst: vec4<f32>, c: vec4<f32>, cov: f32) -> vec4<f32> {
    let a = clamp(c.a * cov, 0.0, 1.0);
    return vec4<f32>(c.rgb * a + dst.rgb * (1.0 - a), a + dst.a * (1.0 - a));
}

fn lod(fw: f32) -> f32 {
    return 1.0 - smoothstep(0.15, 0.6, fw);
}

// Premultiplied grid color.
fn grid_color(u: f32, v: f32, fu_in: f32, fv_in: f32) -> vec4<f32> {
    let fu = fu_in * frame.grid_center.w;
    let fv = fv_in * frame.grid_center.w;
    let minor_cell = frame.grid.x;
    let major_cell = frame.grid.x * frame.grid.y;
    var c = vec4<f32>(0.0);
    let minor = max(line_cov(u / minor_cell, fu / minor_cell), line_cov(v / minor_cell, fv / minor_cell))
        * lod(max(fu, fv) / minor_cell);
    c = over(c, frame.grid_minor, minor);
    let major = max(line_cov(u / major_cell, fu / major_cell), line_cov(v / major_cell, fv / major_cell))
        * lod(max(fu, fv) / major_cell);
    c = over(c, frame.grid_major, major);
    c = over(c, frame.axis_x, clamp(1.5 - abs(v) / max(fv, 1e-9), 0.0, 1.0));
    c = over(c, frame.axis_y, clamp(1.5 - abs(u) / max(fu, 1e-9), 0.0, 1.0));
    var dist: f32;
    if frame.eye.w > 0.5 {
        dist = length(vec2<f32>(u, v) - frame.grid_center.xy);
    } else {
        dist = length(vec3<f32>(u, v, 0.0) - frame.grid_center.xyz);
    }
    let fade = 1.0 - smoothstep(frame.grid.z, frame.grid.w, dist);
    return c * fade;
}

struct GridOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_grid(in: FullscreenOut) -> GridOut {
    let p0 = frame.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let p1 = frame.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let a = p0.xyz / p0.w;
    let b = p1.xyz / p1.w;
    let d = b - a;
    var t = -1.0;
    if abs(d.z) > 1e-12 {
        t = -a.z / d.z;
    }
    let hit = a + d * t;
    // Derivatives are taken in uniform control flow, before any discard.
    let fu = fwidth(hit.x);
    let fv = fwidth(hit.y);
    let c = grid_color(hit.x, hit.y, fu, fv);
    let clip = frame.view_proj * vec4<f32>(hit, 1.0);
    let z = clip.z / clip.w;
    if t <= 0.0 || t > 1.0 || c.a < 1e-4 || z < 0.0 || z > 1.0 {
        discard;
    }
    var out: GridOut;
    out.color = output_color(c.rgb / c.a, c.a);
    out.depth = z;
    return out;
}

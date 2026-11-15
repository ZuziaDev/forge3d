// Selection outline: fullscreen pass over the silhouette mask (0, 0.5 =
// selected, 1 = active), drawn just outside the silhouettes.

@group(1) @binding(0) var mask_tex: texture_2d<f32>;

@fragment
fn fs_outline(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let dims = vec2<i32>(textureDimensions(mask_tex));
    let center = textureLoad(mask_tex, p, 0).r;
    if center > 0.01 {
        discard;
    }
    let r = max(frame.params.x, 1.0);
    let ri = i32(ceil(r));
    var best = 0.0;
    var best_d = 1e9;
    for (var dy = -ri; dy <= ri; dy++) {
        for (var dx = -ri; dx <= ri; dx++) {
            let q = p + vec2<i32>(dx, dy);
            if q.x < 0 || q.y < 0 || q.x >= dims.x || q.y >= dims.y {
                continue;
            }
            let m = textureLoad(mask_tex, q, 0).r;
            if m <= 0.01 {
                continue;
            }
            let d = length(vec2<f32>(f32(dx), f32(dy)));
            if d > r + 0.5 {
                continue;
            }
            if m > best + 0.01 {
                best = m;
                best_d = d;
            } else if abs(m - best) <= 0.01 && d < best_d {
                best_d = d;
            }
        }
    }
    if best <= 0.01 {
        discard;
    }
    var c = frame.outline_sel;
    if best > 0.75 {
        c = frame.outline_active;
    }
    let cov = clamp(r + 0.5 - best_d, 0.0, 1.0);
    return output_color(c.rgb, c.a * cov);
}

//! WGSL sources of the GPU renderer. Always compiled (even without the `gpu`
//! feature) so the shaders can be validated with naga in tests.

/// Shared declarations (frame/object uniforms, shading, sRGB) prepended to every module.
pub const COMMON: &str = include_str!("shaders/common.wgsl");
const MESH: &str = include_str!("shaders/mesh.wgsl");
const LINES: &str = include_str!("shaders/lines.wgsl");
const GRID: &str = include_str!("shaders/grid.wgsl");
const OUTLINE: &str = include_str!("shaders/outline.wgsl");

fn compose(body: &str) -> String {
    let mut s = String::with_capacity(COMMON.len() + body.len() + 1);
    s.push_str(COMMON);
    s.push('\n');
    s.push_str(body);
    s
}

/// Triangle shading, highlight, ID and mask passes.
pub fn mesh() -> String {
    compose(MESH)
}
/// Screen-space expanded lines and points (wireframe, edges, vertices).
pub fn lines() -> String {
    compose(LINES)
}
/// Fullscreen ground grid.
pub fn grid() -> String {
    compose(GRID)
}
/// Fullscreen selection outline from the silhouette mask.
pub fn outline() -> String {
    compose(OUTLINE)
}

/// All composed shader modules as `(name, source)`.
pub fn all() -> Vec<(&'static str, String)> {
    vec![
        ("mesh", mesh()),
        ("lines", lines()),
        ("grid", grid()),
        ("outline", outline()),
    ]
}

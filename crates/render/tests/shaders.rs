//! Parse and validate every WGSL module with naga (no GPU needed).

use naga::valid::{Capabilities, ValidationFlags, Validator};

#[test]
fn all_wgsl_modules_parse_and_validate() {
    let expected: &[(&str, &[&str])] = &[
        (
            "mesh",
            &[
                "vs_main",
                "vs_overlay",
                "fs_shade",
                "fs_highlight",
                "fs_id",
                "fs_mask",
            ],
        ),
        ("lines", &["vs_line", "fs_line"]),
        ("grid", &["vs_fullscreen", "fs_grid"]),
        ("outline", &["vs_fullscreen", "fs_outline"]),
    ];
    let all = forge3d_render::shaders::all();
    assert_eq!(all.len(), expected.len());
    for (name, src) in all {
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{name}.wgsl parse error:\n{}", e.emit_to_string(&src)));
        Validator::new(ValidationFlags::all(), Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| {
                panic!("{name}.wgsl validation error:\n{}", e.emit_to_string(&src))
            });
        let entries: Vec<&str> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        let (_, want) = expected.iter().find(|(n, _)| *n == name).unwrap();
        for w in *want {
            assert!(
                entries.contains(w),
                "{name}: missing entry point {w} in {entries:?}"
            );
        }
    }
}

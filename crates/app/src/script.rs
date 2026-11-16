//! Startup scripts: one [`CommandSpec`] JSON object per line.
//!
//! Lines starting with `#` and blank lines are ignored. A line may start
//! with `ai:` or `cli:` to attribute the command to that origin (it then
//! shows up as "AI: ..." / "CLI: ..." in the history); the default origin is
//! the GUI. The pseudo command `select Name1, Name2` selects objects by name
//! (the last one becomes active). Used by `--script` and `--demo` to build
//! scenes for screenshots.

use forge3d_core::command::Origin;
use forge3d_core::prelude::*;

use crate::error::AppError;

/// Execute every command of `text` on `doc`. Returns the number executed.
pub fn run_script(doc: &mut Document, text: &str) -> Result<usize, AppError> {
    let mut n = 0;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(names) = line.strip_prefix("select ") {
            let ids = names
                .split(',')
                .map(|n| {
                    let n = n.trim();
                    doc.scene
                        .find_by_name(n)
                        .ok_or_else(|| AppError::Script(format!("line {}: no object named '{n}'", i + 1)))
                })
                .collect::<Result<Vec<_>, _>>()?;
            doc.execute_spec(
                CommandSpec::SelectObjects {
                    objects: ids,
                    action: SelectAction::Set,
                },
                Origin::Gui,
            )
            .map_err(|e| AppError::Script(format!("line {}: {e}", i + 1)))?;
            n += 1;
            continue;
        }
        let (origin, json) = if let Some(rest) = line.strip_prefix("ai:") {
            (Origin::Ai, rest)
        } else if let Some(rest) = line.strip_prefix("cli:") {
            (Origin::Cli, rest)
        } else {
            (Origin::Gui, line)
        };
        let spec = CommandSpec::from_json(json.trim())
            .map_err(|e| AppError::Script(format!("line {}: {e}", i + 1)))?;
        doc.execute_spec(spec, origin)
            .map_err(|e| AppError::Script(format!("line {}: {e}", i + 1)))?;
        n += 1;
    }
    Ok(n)
}

/// A small showcase scene.
pub const DEMO_SCRIPT: &str = r##"
# Floor
{"op":"create_primitive","primitive":{"type":"plane","size":10,"subdivisions":1},"name":"Floor","material":{"name":"Concrete","base_color":[0.30,0.31,0.33,1.0],"metallic":0.0,"roughness":0.9}}
# Objects
{"op":"create_primitive","primitive":{"type":"cube","size":1.8},"name":"Crate","translation":[-2.6,0.6,0.9],"rotation_euler_deg":[0,0,20],"material":{"name":"Wood","base_color":[0.48,0.25,0.09,1.0],"metallic":0.0,"roughness":0.75}}
{"op":"create_primitive","primitive":{"type":"uv_sphere","radius":1.0,"segments":40,"rings":20},"name":"Ball","translation":[0.6,-1.4,1.0],"material":{"name":"Gold","base_color":[1.0,0.72,0.2,1.0],"metallic":1.0,"roughness":0.28}}
{"op":"create_primitive","primitive":{"type":"torus","major_radius":0.9,"minor_radius":0.28,"major_segments":48,"minor_segments":16},"name":"Ring","translation":[3.0,0.9,0.3],"rotation_euler_deg":[25,0,0],"material":{"name":"Teal","base_color":[0.05,0.55,0.55,1.0],"metallic":0.1,"roughness":0.4}}
{"op":"create_primitive","primitive":{"type":"cone","radius_bottom":0.8,"radius_top":0.0,"depth":1.8,"segments":32},"name":"Cone","translation":[0.2,2.4,0.9],"material":{"name":"Red","base_color":[0.75,0.08,0.06,1.0],"metallic":0.0,"roughness":0.45}}
{"op":"create_primitive","primitive":{"type":"cylinder","radius":0.45,"depth":1.4,"segments":32},"name":"Post","translation":[-1.0,-2.6,0.7],"material":{"name":"Steel","base_color":[0.6,0.62,0.66,1.0],"metallic":1.0,"roughness":0.5}}
select Crate
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_script_builds_a_scene() {
        let mut doc = Document::new();
        let n = run_script(&mut doc, DEMO_SCRIPT).unwrap();
        assert!(n >= 7);
        assert_eq!(doc.scene.len(), 6);
        let crate_id = doc.scene.find_by_name("Crate").unwrap();
        assert_eq!(doc.selection.active(), Some(crate_id));
        assert!(run_script(&mut doc, "select Nothing").is_err());
    }

    #[test]
    fn origins_and_errors() {
        let mut doc = Document::new();
        run_script(
            &mut doc,
            "# comment\n\nai: {\"op\":\"create_primitive\",\"primitive\":{\"type\":\"cube\"}}\n{\"op\":\"create_primitive\",\"primitive\":{\"type\":\"cube\"}}",
        )
        .unwrap();
        let h = doc.history_list();
        assert!(h[0].label.starts_with("AI: "));
        assert!(!h[1].label.starts_with("AI: "));
        let err = run_script(&mut doc, "{\"op\":\"nope\"}").unwrap_err();
        assert_eq!(err.code(), "script_error");
        assert!(err.to_string().contains("line 1"));
        let err = run_script(&mut doc, "{\"op\":\"extrude\",\"faces\":[999]}").unwrap_err();
        assert!(matches!(err, AppError::Script(_)));
    }
}

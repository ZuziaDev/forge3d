//! Modeling guidance for AI clients: the MCP server `instructions`, the
//! long-form `forge3d://guide` resource and the MCP prompts.

use serde_json::{Map, Value};

/// Instructions sent to the client at initialization: a concise playbook.
pub const INSTRUCTIONS: &str = "forge3d is a Blender-style polygon modeler. You edit ONE shared document that the user may watch live; every change is undoable and labelled \"AI: ...\" in the history.\n\
\n\
WORKFLOW\n\
1. scene_get first (ids, names, transforms, bounds, modifiers, selection).\n\
2. Block out with object_create (cube, plane, uv_sphere, ico_sphere, cylinder, cone, torus) + object_transform; name every part.\n\
3. Refine: mesh_select (by direction/box/indices/edge loop) -> mesh_edit (extrude, inset, bevel, loop_cut, dissolve, bridge_edge_loops, fill_holes, solidify, smooth ...).\n\
4. Prefer non-destructive modifier_edit: mirror for symmetry (model one half), subdivision for smooth organic shapes, bevel (segments 3) for rounded hard edges, solidify for walls, array for repeats. Apply only when later edits must see the result.\n\
5. object_shading smooth (auto_smooth_angle_deg 30-45 for hard-surface; omit for organic) and material_set for color.\n\
6. VERIFY: viewport_render from at least two views (e.g. camera {\"kind\":\"auto\",\"view\":\"front\"} and \"iso\"), plus mesh_measure for real sizes; fix and re-render.\n\
\n\
CONVENTIONS\n\
- Z is up, units are meters, XYZ Euler degrees. Primitives are centered on their origin: a cube of size 1 at translation [0,0,0.5] rests on the ground. Keep real-world scale (table top ~0.75 m, chair seat ~0.45 m, mug ~0.1 m).\n\
- Reference objects by id or unique name. Element indices CHANGE after every topology edit: use the selection returned by mesh_edit, or re-select by direction/box instead of reusing old indices.\n\
- mesh_edit and mesh_select work on the BASE mesh; rendering, export and mesh_measure use the evaluated mesh (modifiers applied).\n\
- Use batch for multi-step changes that belong together (one undo step, all-or-nothing).\n\
- Booleans need closed, overlapping meshes; boolean_op deletes the cutter unless keep_other.\n\
\n\
ERRORS\n\
Errors are {code, message, hint}: read the hint, fix the arguments and retry. invalid_selection/empty_selection: select elements first or pass indices. element_out_of_range: indices are stale, re-query. invalid_parameter: value out of range (the message names the limit). If an edit looks wrong, undo and try different parameters rather than piling on fixes. Read forge3d://guide for worked recipes (table, mug, rounded box, low-poly tree). File paths are relative to the server workspace.";

/// URI of the long-form guide resource.
pub const GUIDE_URI: &str = "forge3d://guide";

/// Long-form best-practice guide with worked recipes (Markdown).
pub const GUIDE: &str = r#"# forge3d modeling guide

## Mental model
* A **document** holds a scene tree of objects. Each object has a local transform, an optional mesh, a material, flat/smooth shading and a **modifier stack**.
* The **base mesh** is what `mesh_select` / `mesh_edit` change. The **evaluated mesh** (base + enabled modifiers, in order) is what you see in renders, what `mesh_measure` measures and what gets exported.
* Z is up, 1 unit = 1 meter. Rotations are XYZ Euler degrees. Every call is one undo step (`batch` groups several).

## The loop that produces good models
1. `scene_get` – know what exists.
2. Block out every part with primitives at real-world size. Name parts (`"Leg.FL"`, `"Seat"`), group them under an empty (`object_create` without `primitive`) via `parent`.
3. Shape: select elements, then edit. Selection by direction is robust: `{"by":{"type":"direction","direction":[0,0,1]}}` = faces facing up.
4. Add detail with inset → extrude, loop cuts for control, bevels for highlights.
5. Use modifiers for anything you may want to change later.
6. Look: `viewport_render` with `camera:{"kind":"auto","view":"front"}`, `"right"`, `"top"` and `"iso"`; turn on `wireframe` to check topology. Check proportions with `mesh_measure`.
7. Fix what you see, render again. Finish with `object_shading` and materials.

## Tool map
* Scene: `scene_get`, `mesh_measure`, `mesh_get` (positions, faces, edges, validation).
* Objects: `object_create`, `object_transform`, `object_duplicate` (copies with offset), `object_rename`, `object_parent` (hierarchy), `object_delete`, `object_join`, `material_set`, `object_shading`.
* Mesh: `mesh_select`, `mesh_edit`, `boolean_op`, `modifier_edit`.
* Look: `viewport_render`. Files: `file_import`, `file_export` (.glb/.gltf/.obj/.stl), `project_save` / `project_load` (.f3d).
* History: `undo`, `redo`, `history_list`, and `batch` for atomic multi-step edits.

## Indices change!
After extrude/bevel/loop_cut/dissolve/etc. old face/edge/vertex indices are invalid. Each `mesh_edit` returns the resulting selection (e.g. extruded caps, bevel strips) – chain the next edit on it by omitting the index list, or select again by direction/box.

## Modifiers (modifier_edit)
| modifier | use for | key params |
|---|---|---|
| mirror | symmetric models: model +X half, mirror X | `axis`, `merge` (weld the seam) |
| subdivision | smooth organic surfaces (Catmull–Clark) | `levels` 1–2 (each ×4 faces) |
| bevel | rounded hard-surface edges | `width`, `segments` 3, `angle_deg` 30 |
| solidify | wall thickness for shells (cups, vases, panels) | `thickness` |
| array | repeats (steps, fence posts, teeth) | `count`, `offset` [x,y,z], `merge` |
| triangulate | game/export needs triangles | – |

Order matters: mirror → subdivision → bevel. `set` toggles `enabled` for quick A/B renders; `apply` bakes into the base mesh (needed before element edits should see the modified shape, or before `object_join`).

## Shading
`object_shading {"smooth":true,"auto_smooth_angle_deg":35}` gives smooth curved areas while keeping corners crisp. Fully smooth (omit the angle) suits subdivided organic shapes. Flat shading is best for low-poly styles.

## Mesh operations cheat sheet
* `extrude` faces along normals (`distance`) – new caps selected.
* `inset` faces (`thickness`, `depth`; `individual` for per-face) – borders/panels.
* `bevel` edges (`width`, `segments` > 1 = rounded).
* `loop_cut` (`edge`, `cuts`) – add edge loops for support or extra resolution.
* `dissolve` vertices/edges/faces – remove detail while keeping the surface closed (unlike `delete`).
* `delete` + `fill_holes` / `bridge_edge_loops` – open and close topology; bridge needs two holes with equal vertex counts.
* `solidify` – destructive thickness; `smooth` – Laplacian relax; `poke` – spikes; `triangulate` / `tris_to_quads`.
* `separate` faces into a new object; `object_join` merges objects (then `merge` with `mode:"distance"` to weld touching parts).
* `boolean_op` union/difference/intersect for holes and cut-outs (closed meshes only).

## Recipes

### Table (1.2 × 0.7 m, 0.75 m high)
```json
{"tool":"batch","arguments":{"description":"Table","steps":[
 {"tool":"object_create","arguments":{"name":"Table"}},
 {"tool":"object_create","arguments":{"primitive":{"type":"cube","size":1},"name":"Top","parent":"Table","translation":[0,0,0.73],"scale":[1.2,0.7,0.04]}},
 {"tool":"object_create","arguments":{"primitive":{"type":"cylinder","radius":0.03,"depth":0.71,"segments":16},"name":"Leg.FL","parent":"Table","translation":[0.55,-0.3,0.355]}},
 {"tool":"object_create","arguments":{"primitive":{"type":"cylinder","radius":0.03,"depth":0.71,"segments":16},"name":"Leg.FR","parent":"Table","translation":[-0.55,-0.3,0.355]}},
 {"tool":"object_create","arguments":{"primitive":{"type":"cylinder","radius":0.03,"depth":0.71,"segments":16},"name":"Leg.BL","parent":"Table","translation":[0.55,0.3,0.355]}},
 {"tool":"object_create","arguments":{"primitive":{"type":"cylinder","radius":0.03,"depth":0.71,"segments":16},"name":"Leg.BR","parent":"Table","translation":[-0.55,0.3,0.355]}}]}}
```
Then `modifier_edit {"object":"Top","action":"add","modifier":{"type":"bevel","width":0.01,"segments":3}}`, `object_shading` smooth with 35°, a wood material, and render front + iso.
Alternative: model one leg and use a mirror modifier on X and Y of a joined leg object.

### Mug with handle (boolean)
1. Body: `cylinder` radius 0.045, depth 0.1, segments 32 at z = 0.05, name "Mug".
2. Hollow: select the top face (direction [0,0,1]) → `inset` thickness 0.005 → `extrude` distance −0.092 (into the body).
3. Handle: `torus` major_radius 0.03, minor_radius 0.007 rotated [90,0,0], at [0.05,0,0.05], name "Handle".
4. `boolean_op {"object":"Mug","other":"Handle","operation":"union"}`.
5. `modifier_edit` add subdivision levels 1 (or a bevel with segments 2), `object_shading` smooth, ceramic material (roughness 0.3). Render front and top to check the handle attaches.

### Rounded box
`object_create` cube → `modifier_edit` add `{"type":"bevel","width":0.08,"segments":4}` → `object_shading {"smooth":true,"auto_smooth_angle_deg":40}`. For a soft pillow look use subdivision levels 2 instead and fully smooth shading.

### Low-poly tree
1. Trunk: `cylinder` radius 0.12, depth 1.2, segments 8 at z = 0.6, brown material.
2. Crown: `ico_sphere` radius 0.7, subdivisions 1 at z = 1.6; `mesh_edit smooth` factor 0.3 for a softer silhouette, or select a few vertices and `transform` them outward for irregularity.
3. Optional second crown layer: `cone` radius_bottom 0.6, radius_top 0, depth 0.9 at z = 2.1.
4. Keep flat shading and saturated greens; render iso.

### Symmetric character / vehicle
Model only the +X half (delete faces with x < 0 via `mesh_select` box + `delete`), add `mirror` axis x with merge, and a `subdivision` level 1 after it. Edit the base half; both sides update.

## Recovering from mistakes
* `undo` (with `steps`) is always safe; `history_list` shows what happened.
* `mesh_get` with `validate` reports non-manifold edges/holes; `recalculate_normals` fixes inside-out faces; `merge` mode distance welds duplicates.
* A modifier that fails (e.g. bevel on non-manifold geometry) is skipped and reported in the object's `evaluated.warnings`.
"#;

/// An MCP prompt template.
pub struct PromptDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// `(name, description, required)`.
    pub arguments: &'static [(&'static str, &'static str, bool)],
}

/// The prompts offered to clients.
pub const PROMPTS: &[PromptDef] = &[
    PromptDef {
        name: "model_object",
        title: "Model an object",
        description: "Plan and build a 3D model of a described object with the forge3d tools, verifying it visually.",
        arguments: &[
            ("description", "What to model, e.g. \"a wooden chair with a slatted back\".", true),
            ("style", "Optional style: low-poly, realistic, stylized, hard-surface ...", false),
            ("size", "Optional overall size, e.g. \"0.9 m tall\".", false),
        ],
    },
    PromptDef {
        name: "review_model",
        title: "Review the model",
        description: "Inspect the current scene from several angles, check topology and proportions, and fix problems.",
        arguments: &[(
            "focus",
            "Optional object name or aspect to focus on (e.g. \"proportions\", \"Handle\").",
            false,
        )],
    },
];

fn arg<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Render prompt `name` with `args`. Errors name the missing argument or
/// unknown prompt.
pub fn render_prompt(name: &str, args: &Map<String, Value>) -> Result<String, String> {
    match name {
        "model_object" => {
            let what = arg(args, "description")
                .ok_or("prompt 'model_object' needs the `description` argument")?;
            let mut text = format!("Model the following in forge3d: {what}.\n");
            if let Some(s) = arg(args, "style") {
                text.push_str(&format!("Style: {s}.\n"));
            }
            if let Some(s) = arg(args, "size") {
                text.push_str(&format!("Overall size: {s}.\n"));
            }
            text.push_str(
                "\nProceed like an experienced Blender artist:\n\
1. Call scene_get. Read forge3d://guide if you have not yet.\n\
2. Write a short plan: the parts, their real-world dimensions in meters (Z up), and which primitive, edits and modifiers build each part.\n\
3. Block out all parts with object_create (named, grouped under an empty parent), using batch for groups of related steps.\n\
4. Refine shapes with mesh_select + mesh_edit; use modifiers (mirror for symmetry, subdivision for smooth forms, bevel with segments 3 for rounded edges, solidify for walls, array for repeats).\n\
5. Apply object_shading and materials.\n\
6. Verify with viewport_render from at least two views (front and iso, plus top/right where useful) and mesh_measure; compare against the plan and fix any problem you see before finishing.\n\
7. Finish with a brief summary of the parts and how to tweak them (e.g. modifier parameters).",
            );
            Ok(text)
        }
        "review_model" => {
            let mut text = String::from(
                "Review the current forge3d model critically.\n\
1. scene_get, then mesh_measure for real-world dimensions.\n\
2. viewport_render from front, right, top and iso (wireframe on for one of them).\n\
3. For each mesh check: closed/manifold (mesh_get validate), inverted normals, intersecting or floating parts, proportions versus real objects, shading (flat vs smooth, hard edges), materials, modifier warnings in `evaluated.warnings`.\n\
4. List the problems found, fix them one by one (undo when a fix goes wrong), and re-render to confirm.\n",
            );
            if let Some(f) = arg(args, "focus") {
                text.push_str(&format!("Focus especially on: {f}.\n"));
            }
            Ok(text)
        }
        other => Err(format!(
            "unknown prompt '{other}'; available: {}",
            PROMPTS
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompts_render_and_validate_arguments() {
        let args = json!({"description": "a mug", "style": "low-poly"});
        let t = render_prompt("model_object", args.as_object().unwrap()).unwrap();
        assert!(t.contains("a mug") && t.contains("low-poly") && t.contains("viewport_render"));
        assert!(render_prompt("model_object", &Map::new())
            .unwrap_err()
            .contains("description"));
        let r = render_prompt("review_model", &Map::new()).unwrap();
        assert!(r.contains("mesh_measure"));
        assert!(render_prompt("nope", &Map::new())
            .unwrap_err()
            .contains("model_object"));
    }

    #[test]
    fn guidance_mentions_every_modeling_tool() {
        for t in crate::tools::tool_names() {
            if matches!(
                t,
                "undo" | "redo" | "history_list" | "project_load" | "file_import"
            ) {
                continue;
            }
            assert!(
                GUIDE.contains(t) || INSTRUCTIONS.contains(t),
                "guide lacks {t}"
            );
        }
        assert!(INSTRUCTIONS.contains("Z is up") && INSTRUCTIONS.len() < 3500);
    }
}

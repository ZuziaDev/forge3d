//! Planning of mutating tools: a tool call is turned into one undoable
//! [`Command`] against the *current* document (object references are
//! resolved here), plus a post-processing step that builds the tool-specific
//! part of the result once the command has been applied.

use forge3d_core::command::{ChangeSummary, Command};
use forge3d_core::commands::AddNode;
use forge3d_core::document::Document;
use forge3d_core::error::{CoreError, CoreResult};
use forge3d_core::math::{trs, quat_from_euler_deg, Space};
use forge3d_core::mesh::ElementKind;
use forge3d_core::scene::Node;
use forge3d_core::selection::{SelectAction, SelectMode};
use forge3d_core::spec::CommandSpec;
use glam::Vec3;
use serde_json::{json, Value};
use uuid::Uuid;

use super::params::*;
use super::Ctx;
use crate::error::{ApiError, ApiResult};
use crate::info::{object_info, resolve_opt, resolve_ref, resolve_refs, round3};

/// Builds the tool-specific result fields after the command was applied.
pub(crate) type PostFn = Box<dyn FnOnce(&Document, &ChangeSummary) -> ApiResult<Value> + Send>;

/// A planned mutation.
pub(crate) struct Plan {
    pub command: Box<dyn Command>,
    pub post: Option<PostFn>,
}

impl Plan {
    fn new(command: Box<dyn Command>) -> Plan {
        Plan {
            command,
            post: None,
        }
    }

    fn spec(spec: CommandSpec) -> ApiResult<Plan> {
        Ok(Plan::new(spec.into_command()?))
    }

    fn with_post(
        mut self,
        f: impl FnOnce(&Document, &ChangeSummary) -> ApiResult<Value> + Send + 'static,
    ) -> Plan {
        self.post = Some(Box::new(f));
        self
    }
}

/// Several commands applied in order as one undo step (with rollback on
/// failure); its description joins the step descriptions.
pub(crate) struct Sequence {
    commands: Vec<Box<dyn Command>>,
    label: Option<String>,
}

impl Sequence {
    pub(crate) fn new(commands: Vec<Box<dyn Command>>, label: Option<String>) -> Sequence {
        Sequence { commands, label }
    }
}

impl Command for Sequence {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let before = doc.selection.clone();
        let mut summary = ChangeSummary::new();
        for i in 0..self.commands.len() {
            match self.commands[i].apply(doc) {
                Ok(s) => {
                    doc.selection.prune(&doc.scene);
                    summary.merge(s);
                }
                Err(e) => {
                    for j in (0..i).rev() {
                        self.commands[j].undo(doc).map_err(|u| {
                            CoreError::CommandState(format!("rollback of step {j} failed: {u}"))
                        })?;
                    }
                    doc.selection = before;
                    return Err(e);
                }
            }
        }
        Ok(summary)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        for c in self.commands.iter_mut().rev() {
            c.undo(doc)?;
        }
        Ok(())
    }

    fn describe(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        self.commands
            .iter()
            .map(|c| c.describe())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn finite3(name: &str, v: [f32; 3]) -> ApiResult<Vec3> {
    let v = Vec3::from_array(v);
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ApiError::invalid_arguments(format!("`{name}` must contain finite numbers")))
    }
}

fn objects_post(ids_from: fn(&ChangeSummary) -> Vec<Uuid>) -> impl FnOnce(&Document, &ChangeSummary) -> ApiResult<Value> + Send {
    move |doc, changes| {
        let infos = ids_from(changes)
            .into_iter()
            .filter(|id| doc.scene.contains(*id))
            .map(|id| object_info(doc, id))
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(json!({ "objects": infos }))
    }
}

fn created_ids(c: &ChangeSummary) -> Vec<Uuid> {
    c.created.iter().map(|o| o.id).collect()
}

fn modified_ids(c: &ChangeSummary) -> Vec<Uuid> {
    c.modified.iter().map(|o| o.id).collect()
}

// ---------------------------------------------------------------------------

pub(crate) fn object_create(doc: &Document, p: ObjectCreateParams) -> ApiResult<Plan> {
    let parent = resolve_opt(&doc.scene, &p.parent)?;
    let plan = match p.primitive {
        Some(primitive) => Plan::spec(CommandSpec::CreatePrimitive {
            primitive,
            name: p.name,
            parent,
            translation: p.translation,
            rotation_euler_deg: p.rotation_euler_deg,
            scale: p.scale,
            material: p.material,
        })?,
        None => {
            if p.material.is_some() {
                return Err(ApiError::invalid_arguments(
                    "an empty group node (no `primitive`) cannot carry a material",
                )
                .with_hint("Give a `primitive` (e.g. {\"type\":\"cube\"}) or drop `material`; assign materials to the mesh objects inside the group."));
            }
            let t = finite3("translation", p.translation.unwrap_or([0.0; 3]))?;
            let r = finite3("rotation_euler_deg", p.rotation_euler_deg.unwrap_or([0.0; 3]))?;
            let s = finite3("scale", p.scale.unwrap_or([1.0; 3]))?;
            if s.abs().min_element() < 1e-8 {
                return Err(CoreError::param("scale", "components must be non-zero").into());
            }
            let node = Node::new(p.name.unwrap_or_else(|| "Empty".into()))
                .with_transform(trs(t, quat_from_euler_deg(r.to_array()), s));
            Plan::new(Box::new(AddNode::new(node, parent, true, "empty")))
        }
    };
    Ok(plan.with_post(|doc, changes| {
        let id = changes
            .created
            .first()
            .map(|o| o.id)
            .ok_or_else(|| ApiError::internal("object_create created nothing"))?;
        Ok(json!({ "object": object_info(doc, id)? }))
    }))
}

pub(crate) fn object_transform(doc: &Document, p: ObjectTransformParams) -> ApiResult<Plan> {
    let objects = resolve_refs(&doc.scene, &p.objects)?;
    if p.translation.is_none()
        && p.rotation_euler_deg.is_none()
        && p.scale.is_none()
        && p.rotation_axis.is_none()
    {
        return Err(ApiError::invalid_arguments(
            "nothing to do: give at least one of `translation`, `rotation_euler_deg`, `rotation_axis`+`rotation_angle_deg` or `scale`",
        ));
    }
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    match p.mode {
        TransformMode::Absolute => {
            if p.rotation_axis.is_some() || p.rotation_angle_deg.is_some() || p.pivot.is_some() {
                return Err(ApiError::invalid_arguments(
                    "`rotation_axis`, `rotation_angle_deg` and `pivot` are only valid with mode `relative`",
                ));
            }
            for (n, v) in [
                ("translation", p.translation),
                ("rotation_euler_deg", p.rotation_euler_deg),
                ("scale", p.scale),
            ] {
                if let Some(v) = v {
                    finite3(n, v)?;
                }
            }
            if let Some(s) = p.scale {
                if Vec3::from_array(s).abs().min_element() < 1e-8 {
                    return Err(CoreError::param("scale", "components must be non-zero").into());
                }
            }
            cmds.push(
                CommandSpec::SetTransform {
                    objects: objects.clone(),
                    translation: p.translation,
                    rotation_euler_deg: p.rotation_euler_deg,
                    scale: p.scale,
                }
                .into_command()?,
            );
        }
        TransformMode::Relative => {
            if let Some(t) = p.translation {
                finite3("translation", t)?;
                cmds.push(
                    CommandSpec::Translate {
                        objects: objects.clone(),
                        offset: t,
                        space: p.space,
                    }
                    .into_command()?,
                );
            }
            if p.pivot.is_some() && p.space == Space::Local && p.rotation_euler_deg.is_some() {
                return Err(ApiError::invalid_arguments(
                    "`pivot` is a world-space point and requires `space: world`",
                ));
            }
            if let Some(r) = p.rotation_euler_deg {
                finite3("rotation_euler_deg", r)?;
                for (axis, angle) in [([1.0, 0.0, 0.0], r[0]), ([0.0, 1.0, 0.0], r[1]), ([0.0, 0.0, 1.0], r[2])] {
                    if angle != 0.0 {
                        cmds.push(
                            CommandSpec::Rotate {
                                objects: objects.clone(),
                                axis,
                                angle_deg: angle,
                                pivot: p.pivot,
                                space: p.space,
                            }
                            .into_command()?,
                        );
                    }
                }
            }
            match (p.rotation_axis, p.rotation_angle_deg) {
                (Some(axis), Some(angle)) => {
                    let a = finite3("rotation_axis", axis)?;
                    if a.length_squared() < 1e-12 {
                        return Err(ApiError::invalid_arguments("`rotation_axis` must be non-zero"));
                    }
                    cmds.push(
                        CommandSpec::Rotate {
                            objects: objects.clone(),
                            axis,
                            angle_deg: angle,
                            pivot: p.pivot,
                            space: p.space,
                        }
                        .into_command()?,
                    );
                }
                (None, None) => {}
                _ => {
                    return Err(ApiError::invalid_arguments(
                        "`rotation_axis` and `rotation_angle_deg` must be given together",
                    ))
                }
            }
            if let Some(s) = p.scale {
                finite3("scale", s)?;
                cmds.push(
                    CommandSpec::Scale {
                        objects: objects.clone(),
                        factor: s,
                        pivot: p.pivot,
                    }
                    .into_command()?,
                );
            }
        }
    }
    let command: Box<dyn Command> = if cmds.len() == 1 {
        cmds.pop().expect("one command")
    } else {
        Box::new(Sequence::new(cmds, None))
    };
    Ok(Plan::new(command).with_post(objects_post(modified_ids)))
}

pub(crate) fn object_delete(doc: &Document, p: ObjectDeleteParams) -> ApiResult<Plan> {
    if p.objects.is_empty() {
        return Err(ApiError::invalid_arguments("`objects` must list at least one object"));
    }
    let objects = resolve_refs(&doc.scene, &Some(p.objects))?;
    Plan::spec(CommandSpec::DeleteObjects {
        objects,
        keep_children: p.keep_children,
    })
}

pub(crate) fn object_duplicate(doc: &Document, p: ObjectDuplicateParams) -> ApiResult<Plan> {
    let objects = resolve_refs(&doc.scene, &p.objects)?;
    finite3("offset", p.offset)?;
    Ok(Plan::spec(CommandSpec::DuplicateObjects {
        objects,
        offset: p.offset,
    })?
    .with_post(objects_post(created_ids)))
}

pub(crate) fn object_rename(doc: &Document, p: ObjectRenameParams) -> ApiResult<Plan> {
    let id = resolve_ref(&doc.scene, &p.object)?;
    if p.name.trim().is_empty() {
        return Err(ApiError::invalid_arguments("`name` must not be empty"));
    }
    Ok(Plan::spec(CommandSpec::RenameObject {
        object: Some(id),
        name: p.name,
    })?
    .with_post(move |doc, _| Ok(json!({ "object": object_info(doc, id)? }))))
}

pub(crate) fn object_parent(doc: &Document, p: ObjectParentParams) -> ApiResult<Plan> {
    if p.objects.is_empty() {
        return Err(ApiError::invalid_arguments("`objects` must list at least one object"));
    }
    let objects = resolve_refs(&doc.scene, &Some(p.objects))?.unwrap_or_default();
    let parent = resolve_opt(&doc.scene, &p.parent)?;
    let mut cmds = Vec::new();
    for id in &objects {
        cmds.push(
            CommandSpec::Reparent {
                object: Some(*id),
                parent,
                keep_world_transform: p.keep_world_transform,
            }
            .into_command()?,
        );
    }
    let command: Box<dyn Command> = if cmds.len() == 1 {
        cmds.pop().expect("one command")
    } else {
        Box::new(Sequence::new(cmds, None))
    };
    Ok(Plan::new(command).with_post(move |doc, _| {
        let infos = objects
            .iter()
            .map(|id| object_info(doc, *id))
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(json!({ "objects": infos }))
    }))
}

/// Mesh counts and element selection of `id` after an edit.
fn mesh_state(doc: &Document, id: Uuid, max_indices: usize) -> ApiResult<Value> {
    let mesh = doc.scene.mesh(id)?;
    let sel = doc.selection.elements(id);
    let cap = |s: Option<&std::collections::BTreeSet<u32>>| -> Value {
        let list: Vec<u32> = s.map(|s| s.iter().take(max_indices).copied().collect()).unwrap_or_default();
        Value::from(list)
    };
    Ok(json!({
        "object": id,
        "object_name": doc.scene.node(id)?.name,
        "mesh": {
            "vertex_count": mesh.vertex_count(),
            "edge_count": mesh.edge_count(),
            "face_count": mesh.face_count(),
        },
        "selection": {
            "mode": doc.selection.mode,
            "counts": {
                "vertices": sel.map_or(0, |s| s.vertices.len()),
                "edges": sel.map_or(0, |s| s.edges.len()),
                "faces": sel.map_or(0, |s| s.faces.len()),
            },
            "vertices": cap(sel.map(|s| &s.vertices)),
            "edges": cap(sel.map(|s| &s.edges)),
            "faces": cap(sel.map(|s| &s.faces)),
            "max_indices": max_indices,
        }
    }))
}

fn active_mesh_object(doc: &Document, r: &Option<String>) -> ApiResult<Uuid> {
    match resolve_opt(&doc.scene, r)? {
        Some(id) => Ok(id),
        None => doc
            .selection
            .active()
            .or(doc.selection.objects().last().copied())
            .ok_or_else(|| {
                ApiError::new(
                    "empty_selection",
                    "no `object` given and there is no active object",
                    "Pass `object` (id or unique name; see scene_get) or select an object first.",
                )
            }),
    }
}

pub(crate) fn mesh_edit(doc: &Document, p: MeshEditParams) -> ApiResult<Plan> {
    let id = active_mesh_object(doc, &p.object)?;
    let mesh = doc.scene.mesh(id)?;
    let object = Some(id);
    let spec = match p.operation {
        MeshOperation::Extrude {
            faces,
            distance,
            offset,
        } => CommandSpec::Extrude {
            object,
            faces,
            distance,
            offset,
        },
        MeshOperation::Inset {
            faces,
            thickness,
            depth,
            individual,
        } => CommandSpec::Inset {
            object,
            faces,
            thickness,
            depth,
            individual,
        },
        MeshOperation::Bevel { edges, width } => CommandSpec::Bevel {
            object,
            edges,
            width,
        },
        MeshOperation::LoopCut { edge, cuts, factor } => CommandSpec::LoopCut {
            object,
            edge,
            cuts,
            factor,
        },
        MeshOperation::Subdivide { levels } => {
            if !(1..=4).contains(&levels) {
                return Err(CoreError::param("levels", "must be in 1..=4").into());
            }
            let estimate = (mesh.face_count().max(1) as u64) * 4u64.pow(levels);
            if estimate > 2_000_000 {
                return Err(ApiError::new(
                    "mesh_too_large",
                    format!("subdividing {} faces {levels} times would create ~{estimate} faces", mesh.face_count()),
                    "Use fewer `levels` (each level multiplies the face count by about 4) or subdivide a simpler mesh.",
                ));
            }
            CommandSpec::Subdivide { object, levels }
        }
        MeshOperation::Merge {
            vertices,
            mode,
            threshold,
        } => CommandSpec::MergeVertices {
            object,
            vertices,
            mode,
            threshold,
        },
        MeshOperation::Delete { kind, indices } => CommandSpec::DeleteElements {
            object,
            kind,
            indices,
        },
        MeshOperation::Mirror {
            axis,
            merge,
            merge_threshold,
        } => CommandSpec::Mirror {
            object,
            axis,
            merge,
            merge_threshold,
        },
        MeshOperation::RecalculateNormals { inside } => {
            CommandSpec::RecalculateNormals { object, inside }
        }
        MeshOperation::FlipNormals { faces } => CommandSpec::FlipNormals { object, faces },
        MeshOperation::Transform {
            kind,
            indices,
            translation,
            rotation_euler_deg,
            scale,
            pivot,
        } => {
            if translation.is_none() && rotation_euler_deg.is_none() && scale.is_none() {
                return Err(ApiError::invalid_arguments(
                    "transform needs at least one of `translation`, `rotation_euler_deg`, `scale`",
                ));
            }
            CommandSpec::TransformElements {
                object,
                kind,
                indices,
                translation,
                rotation_euler_deg,
                scale,
                pivot,
            }
        }
        MeshOperation::SetMeshData { mesh } => CommandSpec::SetMeshData { object, mesh },
    };
    Ok(Plan::spec(spec)?.with_post(move |doc, _| mesh_state(doc, id, 200)))
}

fn mode_for(kind: ElementKind) -> SelectMode {
    match kind {
        ElementKind::Vertex => SelectMode::Vertex,
        ElementKind::Edge => SelectMode::Edge,
        ElementKind::Face => SelectMode::Face,
    }
}

pub(crate) fn mesh_select(doc: &Document, p: MeshSelectParams) -> ApiResult<Plan> {
    let max_indices = p.max_indices.min(5000);
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    let Some(by) = p.by else {
        let mode = p.mode.or(p.kind.map(mode_for)).ok_or_else(|| {
            ApiError::invalid_arguments("give `by` (what to select) and/or `mode`")
        })?;
        let plan = Plan::spec(CommandSpec::SetSelectMode { mode })?;
        return Ok(plan.with_post(move |doc, _| {
            Ok(json!({ "selection": crate::info::selection_info(doc) }))
        }));
    };
    if let SelectBy::Objects { objects } = by {
        let ids = resolve_refs(&doc.scene, &Some(objects))?.unwrap_or_default();
        if doc.selection.mode != SelectMode::Object {
            cmds.push(CommandSpec::SetSelectMode { mode: SelectMode::Object }.into_command()?);
        }
        cmds.push(
            CommandSpec::SelectObjects {
                objects: ids,
                action: p.action,
            }
            .into_command()?,
        );
        return Ok(Plan::new(Box::new(Sequence::new(cmds, None))).with_post(|doc, _| {
            Ok(json!({ "selection": crate::info::selection_info(doc) }))
        }));
    }
    let id = active_mesh_object(doc, &p.object)?;
    doc.scene.mesh(id)?;
    let kind = match &by {
        SelectBy::EdgeLoop { .. } => ElementKind::Edge,
        _ => p
            .kind
            .or(p.mode.and_then(|m| m.element_kind()))
            .or(doc.selection.mode.element_kind())
            .unwrap_or(ElementKind::Face),
    };
    if let SelectBy::EdgeLoop { .. } = by {
        if p.kind.is_some_and(|k| k != ElementKind::Edge) {
            return Err(ApiError::invalid_arguments("edge_loop always selects edges; use kind `edge` or omit it"));
        }
    }
    let target_mode = p.mode.unwrap_or(mode_for(kind));
    if doc.selection.mode != target_mode {
        cmds.push(CommandSpec::SetSelectMode { mode: target_mode }.into_command()?);
    }
    // The object becomes selected + active so later mesh_edit calls without
    // `object` target it.
    if doc.selection.active() != Some(id) {
        cmds.push(
            CommandSpec::SelectObjects {
                objects: vec![id],
                action: SelectAction::Set,
            }
            .into_command()?,
        );
    }
    let object = Some(id);
    let k = Some(kind);
    let action = p.action;
    let spec = match by {
        SelectBy::Indices { indices } => CommandSpec::SelectElements {
            object,
            kind: k,
            indices,
            action,
        },
        SelectBy::All => CommandSpec::SelectAll { object, kind: k },
        SelectBy::None => CommandSpec::SelectNone { object },
        SelectBy::Invert => CommandSpec::SelectInvert { object, kind: k },
        SelectBy::Direction {
            direction,
            max_angle_deg,
            space,
        } => CommandSpec::SelectByDirection {
            object,
            kind: k,
            direction,
            max_angle_deg,
            space,
            action,
        },
        SelectBy::Normal {
            reference,
            max_angle_deg,
        } => CommandSpec::SelectByNormal {
            object,
            kind: k,
            reference,
            max_angle_deg,
            action,
        },
        SelectBy::Box { min, max, space } => CommandSpec::SelectInBox {
            object,
            kind: k,
            min,
            max,
            space,
            action,
        },
        SelectBy::EdgeLoop { edge, ring } => CommandSpec::SelectEdgeLoop {
            object,
            edge,
            ring,
            action,
        },
        SelectBy::Objects { .. } => unreachable!("handled above"),
    };
    cmds.push(spec.into_command()?);
    let command: Box<dyn Command> = if cmds.len() == 1 {
        cmds.pop().expect("one command")
    } else {
        Box::new(Sequence::new(cmds, None))
    };
    Ok(Plan::new(command).with_post(move |doc, _| {
        let mut v = mesh_state(doc, id, max_indices)?;
        let sel = doc.selection.elements(id);
        let set = sel.map(|s| s.set(kind).clone()).unwrap_or_default();
        v["kind"] = json!(kind);
        v["selected_count"] = json!(set.len());
        v["indices"] = json!(set.iter().take(max_indices).collect::<Vec<_>>());
        v["truncated"] = json!(set.len() > max_indices);
        Ok(v)
    }))
}

pub(crate) fn boolean_op(doc: &Document, p: BooleanOpParams) -> ApiResult<Plan> {
    let a = resolve_ref(&doc.scene, &p.object)?;
    let b = resolve_ref(&doc.scene, &p.other)?;
    if a == b {
        return Err(ApiError::invalid_arguments("`object` and `other` must be different objects"));
    }
    Ok(Plan::spec(CommandSpec::Boolean {
        object: Some(a),
        other: b,
        operation: p.operation,
        keep_other: p.keep_other,
    })?
    .with_post(move |doc, _| Ok(json!({ "object": object_info(doc, a)? }))))
}

pub(crate) fn material_set(doc: &Document, p: MaterialSetParams) -> ApiResult<Plan> {
    let objects = resolve_refs(&doc.scene, &p.objects)?;
    let material = match (p.material, p.clear) {
        (Some(_), true) => {
            return Err(ApiError::invalid_arguments("give either `material` or `clear: true`, not both"))
        }
        (None, false) => {
            return Err(ApiError::invalid_arguments(
                "give a `material` (e.g. {\"base_color\":[0.8,0.1,0.1,1]}) or `clear: true`",
            ))
        }
        (m, _) => m,
    };
    Ok(Plan::spec(CommandSpec::SetMaterial { objects, material })?
        .with_post(objects_post(modified_ids)))
}

pub(crate) fn file_import(doc: &Document, p: FileImportParams, ctx: &Ctx) -> ApiResult<Plan> {
    let parent = resolve_opt(&doc.scene, &p.parent)?;
    let path = ctx.workspace.resolve(&p.path, crate::workspace::PathAccess::Read)?;
    let opts = forge3d_io::ImportOptions {
        weld: p.weld,
        obj_up_axis: match p.obj_up_axis {
            Some(UpAxisParam::Z) => forge3d_io::UpAxis::Z,
            _ => forge3d_io::UpAxis::Y,
        },
        ..Default::default()
    };
    let (command, warnings) = forge3d_io::import_file_command(&path, &opts, parent)?;
    let shown = ctx.workspace.display_path(&path);
    Ok(Plan::new(command).with_post(move |doc, changes| {
        let mut v = objects_post(created_ids)(doc, changes)?;
        v["path"] = json!(shown);
        v["warnings"] = json!(warnings);
        Ok(v)
    }))
}

/// Round helper re-exported for tool modules.
pub(crate) fn r3(v: Vec3) -> [f32; 3] {
    round3(v.to_array())
}

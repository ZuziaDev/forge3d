//! Concrete [`Command`] implementations. Most callers construct these through
//! [`crate::spec::CommandSpec`]; they are public for direct use as well.
//!
//! Every command resolves its targets (explicit ids or the current
//! selection) on its first `apply` and caches the result (generated ids,
//! before/after snapshots), so redo reproduces exactly the same state.

use glam::{Affine3A, Quat, Vec3};
use uuid::Uuid;

use crate::command::{ChangeSummary, Command, ObjectSummary};
use crate::document::Document;
use crate::error::{CoreError, CoreResult};
use crate::math::{Aabb, Space};
use crate::mesh::{EdgeId, ElementKind, FaceId, Mesh, VertexId};
use crate::ops::{self, BooleanOp, OpResult};
use crate::scene::{Material, Node, RemovedSubtree};
use crate::selection::{self as sel, ElementSelection, SelectAction, SelectMode, Selection};

// ---------------------------------------------------------------------------
// target resolution helpers
// ---------------------------------------------------------------------------

/// Explicit ids or the current object selection; validates existence.
pub fn resolve_objects(doc: &Document, ids: Option<&[Uuid]>) -> CoreResult<Vec<Uuid>> {
    let list: Vec<Uuid> = match ids {
        Some(v) => v.to_vec(),
        None => doc.selection.objects().to_vec(),
    };
    if list.is_empty() {
        return Err(CoreError::EmptySelection("no objects selected".into()));
    }
    let mut out = Vec::with_capacity(list.len());
    for id in list {
        doc.scene.node(id)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Explicit id or the active object (falling back to the last selected one).
pub fn resolve_object(doc: &Document, id: Option<Uuid>) -> CoreResult<Uuid> {
    let id = id
        .or(doc.selection.active())
        .or(doc.selection.objects().last().copied())
        .ok_or_else(|| {
            CoreError::EmptySelection("no active object; pass `object` or select one".into())
        })?;
    doc.scene.node(id)?;
    Ok(id)
}

/// Element kind: explicit, else from the selection mode.
pub fn resolve_kind(doc: &Document, kind: Option<ElementKind>) -> CoreResult<ElementKind> {
    kind.or(doc.selection.mode.element_kind()).ok_or_else(|| {
        CoreError::InvalidSelection(
            "selection mode is `object`; pass `kind` or switch to vertex/edge/face mode".into(),
        )
    })
}

fn name_of(doc: &Document, id: Uuid) -> String {
    doc.scene
        .get(id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn names(doc: &Document, ids: &[Uuid]) -> String {
    let n: Vec<String> = ids.iter().map(|&i| format!("'{}'", name_of(doc, i))).collect();
    n.join(", ")
}

fn not_applied() -> CoreError {
    CoreError::CommandState("undo called before apply".into())
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn parent_world(doc: &Document, id: Uuid) -> CoreResult<Affine3A> {
    match doc.scene.node(id)?.parent() {
        Some(p) => doc.scene.world_transform(p),
        None => Ok(Affine3A::IDENTITY),
    }
}

// ---------------------------------------------------------------------------
// AddNode
// ---------------------------------------------------------------------------

/// Insert a prepared node (primitive, empty or imported mesh). The node's
/// name is used as the base of a unique name.
pub struct AddNode {
    node: Option<Node>,
    parent: Option<Uuid>,
    select: bool,
    what: String,
    created: Option<Uuid>,
    removed: Option<RemovedSubtree>,
    desc: String,
}

impl AddNode {
    pub fn new(node: Node, parent: Option<Uuid>, select: bool, what: impl Into<String>) -> AddNode {
        let what = what.into();
        AddNode {
            desc: format!("Create {what} '{}'", node.name),
            node: Some(node),
            parent,
            select,
            what,
            created: None,
            removed: None,
        }
    }

    /// Id of the created node (after the first apply).
    pub fn created_id(&self) -> Option<Uuid> {
        self.created
    }
}

impl Command for AddNode {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let id = if let Some(sub) = self.removed.take() {
            let id = sub.nodes[0].id();
            if let Err(e) = doc.scene.restore_subtree(sub.clone()) {
                self.removed = Some(sub);
                return Err(e);
            }
            id
        } else {
            let mut node = self
                .node
                .take()
                .ok_or_else(|| CoreError::CommandState("node already inserted".into()))?;
            let base = node.name.clone();
            node.name = doc.scene.unique_name(&base);
            match doc.scene.add_node(node.clone(), self.parent) {
                Ok(id) => id,
                Err(e) => {
                    node.name = base;
                    self.node = Some(node);
                    return Err(e);
                }
            }
        };
        self.created = Some(id);
        self.desc = format!("Create {} '{}'", self.what, name_of(doc, id));
        if self.select {
            doc.selection.set_objects([id]);
        }
        ChangeSummary::new().created(&doc.scene, id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let id = self.created.ok_or_else(not_applied)?;
        self.removed = Some(doc.scene.remove_subtree(id)?);
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

// ---------------------------------------------------------------------------
// Object transforms
// ---------------------------------------------------------------------------

/// Kind of object transform.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformOp {
    /// Replace components of the local transform (absolute).
    Set {
        translation: Option<Vec3>,
        rotation: Option<Quat>,
        scale: Option<Vec3>,
    },
    /// Replace the whole local transform.
    Matrix(Affine3A),
    /// Move by an offset (world axes or the object's local axes).
    Translate { offset: Vec3, space: Space },
    /// Rotate around an axis. World space: about `pivot` (default: the
    /// object's world origin). Local space: about the object's origin, axis
    /// in local coordinates.
    Rotate {
        axis: Vec3,
        angle_rad: f32,
        pivot: Option<Vec3>,
        space: Space,
    },
    /// Scale along the object's local axes, or about a world `pivot`.
    Scale { factor: Vec3, pivot: Option<Vec3> },
}

impl TransformOp {
    fn validate(&self) -> CoreResult<()> {
        let finite3 = |name: &str, v: Vec3| {
            if v.is_finite() {
                Ok(())
            } else {
                Err(CoreError::param(name, "must be finite"))
            }
        };
        match self {
            TransformOp::Set {
                translation,
                rotation,
                scale,
            } => {
                if let Some(t) = translation {
                    finite3("translation", *t)?;
                }
                if let Some(r) = rotation {
                    if !r.is_finite() {
                        return Err(CoreError::param("rotation", "must be finite"));
                    }
                }
                if let Some(s) = scale {
                    finite3("scale", *s)?;
                    if s.abs().min_element() < 1e-8 {
                        return Err(CoreError::param("scale", "components must be non-zero"));
                    }
                }
            }
            TransformOp::Matrix(m) => {
                if !m.is_finite() || m.matrix3.determinant().abs() < 1e-12 {
                    return Err(CoreError::param("matrix", "must be finite and invertible"));
                }
            }
            TransformOp::Translate { offset, .. } => finite3("offset", *offset)?,
            TransformOp::Rotate {
                axis,
                angle_rad,
                pivot,
                ..
            } => {
                finite3("axis", *axis)?;
                if axis.length_squared() < 1e-12 {
                    return Err(CoreError::param("axis", "must be non-zero"));
                }
                if !angle_rad.is_finite() {
                    return Err(CoreError::param("angle_deg", "must be finite"));
                }
                if let Some(p) = pivot {
                    finite3("pivot", *p)?;
                }
            }
            TransformOp::Scale { factor, pivot } => {
                finite3("factor", *factor)?;
                if factor.abs().min_element() < 1e-8 {
                    return Err(CoreError::param("factor", "components must be non-zero"));
                }
                if let Some(p) = pivot {
                    finite3("pivot", *p)?;
                }
            }
        }
        Ok(())
    }

    fn verb(&self) -> &'static str {
        match self {
            TransformOp::Set { .. } | TransformOp::Matrix(_) => "Set transform of",
            TransformOp::Translate { .. } => "Move",
            TransformOp::Rotate { .. } => "Rotate",
            TransformOp::Scale { .. } => "Scale",
        }
    }
}

/// Apply a transform to objects.
pub struct TransformObjects {
    targets: Option<Vec<Uuid>>,
    op: TransformOp,
    resolved: Vec<(Uuid, Affine3A, Affine3A)>,
    desc: String,
}

impl TransformObjects {
    pub fn new(targets: Option<Vec<Uuid>>, op: TransformOp) -> CoreResult<TransformObjects> {
        op.validate()?;
        Ok(TransformObjects {
            desc: format!("{} objects", op.verb()),
            targets,
            op,
            resolved: Vec::new(),
        })
    }

    fn compute(&self, doc: &Document, id: Uuid) -> CoreResult<Affine3A> {
        let local = doc.scene.node(id)?.transform;
        let world = doc.scene.world_transform(id)?;
        let pw = parent_world(doc, id)?;
        let from_world = |w: Affine3A| pw.inverse() * w;
        Ok(match &self.op {
            TransformOp::Set {
                translation,
                rotation,
                scale,
            } => {
                let (s, r, t) = local.to_scale_rotation_translation();
                Affine3A::from_scale_rotation_translation(
                    scale.unwrap_or(s),
                    rotation.unwrap_or(r),
                    translation.unwrap_or(t),
                )
            }
            TransformOp::Matrix(m) => *m,
            TransformOp::Translate { offset, space } => match space {
                Space::World => from_world(Affine3A::from_translation(*offset) * world),
                Space::Local => local * Affine3A::from_translation(*offset),
            },
            TransformOp::Rotate {
                axis,
                angle_rad,
                pivot,
                space,
            } => {
                let r = Affine3A::from_quat(Quat::from_axis_angle(axis.normalize(), *angle_rad));
                match space {
                    Space::World => {
                        let p = pivot.unwrap_or(Vec3::from(world.translation));
                        from_world(
                            Affine3A::from_translation(p) * r * Affine3A::from_translation(-p) * world,
                        )
                    }
                    Space::Local => local * r,
                }
            }
            TransformOp::Scale { factor, pivot } => {
                let s = Affine3A::from_scale(*factor);
                match pivot {
                    None => local * s,
                    Some(p) => from_world(
                        Affine3A::from_translation(*p) * s * Affine3A::from_translation(-*p) * world,
                    ),
                }
            }
        })
    }
}

impl Command for TransformObjects {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.resolved.is_empty() {
            let ids = resolve_objects(doc, self.targets.as_deref())?;
            // Skip objects whose ancestor is also transformed (world-space ops
            // would otherwise apply twice).
            let relative = !matches!(self.op, TransformOp::Set { .. } | TransformOp::Matrix(_));
            let ids: Vec<Uuid> = ids
                .iter()
                .copied()
                .filter(|&i| !relative || !ids.iter().any(|&a| a != i && doc.scene.is_ancestor(a, i)))
                .collect();
            let mut resolved = Vec::with_capacity(ids.len());
            for &id in &ids {
                let new = self.compute(doc, id)?;
                if !new.is_finite() {
                    return Err(CoreError::param("transform", "produces a non-finite matrix"));
                }
                resolved.push((id, doc.scene.node(id)?.transform, new));
            }
            self.desc = format!("{} {}", self.op.verb(), names(doc, &ids));
            self.resolved = resolved;
        }
        for (id, _, _) in &self.resolved {
            doc.scene.node(*id)?;
        }
        let mut s = ChangeSummary::new();
        for (id, _, new) in &self.resolved {
            doc.scene.node_mut(*id)?.transform = *new;
            s = s.modified(&doc.scene, *id)?;
        }
        Ok(s)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        if self.resolved.is_empty() {
            return Err(not_applied());
        }
        for (id, old, _) in &self.resolved {
            doc.scene.node_mut(*id)?.transform = *old;
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

// ---------------------------------------------------------------------------
// Delete / duplicate / rename / reparent / material / visibility
// ---------------------------------------------------------------------------

struct DeletedEntry {
    sub: RemovedSubtree,
    /// (child, original local transform, original index) of children that
    /// were moved to the deleted node's parent.
    moved_children: Vec<(Uuid, Affine3A, usize)>,
}

/// Delete objects (with their descendants unless `keep_children`).
pub struct DeleteObjects {
    targets: Option<Vec<Uuid>>,
    keep_children: bool,
    resolved: Option<Vec<Uuid>>,
    done: Vec<DeletedEntry>,
    desc: String,
}

impl DeleteObjects {
    pub fn new(targets: Option<Vec<Uuid>>, keep_children: bool) -> DeleteObjects {
        DeleteObjects {
            targets,
            keep_children,
            resolved: None,
            done: Vec::new(),
            desc: "Delete objects".into(),
        }
    }
}

impl Command for DeleteObjects {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let ids = match &self.resolved {
            Some(r) => r.clone(),
            None => {
                let ids = resolve_objects(doc, self.targets.as_deref())?;
                let ids: Vec<Uuid> = if self.keep_children {
                    ids
                } else {
                    ids.iter()
                        .copied()
                        .filter(|&i| !ids.iter().any(|&a| a != i && doc.scene.is_ancestor(a, i)))
                        .collect()
                };
                self.desc = format!("Delete {}", names(doc, &ids));
                self.resolved = Some(ids.clone());
                ids
            }
        };
        for &id in &ids {
            doc.scene.node(id)?;
        }
        let mut summary = ChangeSummary::new();
        self.done.clear();
        for &id in &ids {
            let mut gone = vec![ObjectSummary::of(&doc.scene, id)?];
            let mut moved = Vec::new();
            if self.keep_children {
                let parent = doc.scene.node(id)?.parent();
                let children: Vec<Uuid> = doc.scene.node(id)?.children().to_vec();
                for (idx, c) in children.into_iter().enumerate() {
                    let t = doc.scene.node(c)?.transform;
                    doc.scene.set_parent(c, parent, true, None)?;
                    moved.push((c, t, idx));
                    summary = summary.modified(&doc.scene, c)?;
                }
            } else {
                for d in doc.scene.descendants(id) {
                    gone.push(ObjectSummary::of(&doc.scene, d)?);
                }
            }
            let sub = doc.scene.remove_subtree(id)?;
            for g in gone {
                summary = summary.deleted(g);
            }
            self.done.push(DeletedEntry {
                sub,
                moved_children: moved,
            });
        }
        Ok(summary)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        if self.resolved.is_none() {
            return Err(not_applied());
        }
        while let Some(entry) = self.done.pop() {
            let id = entry.sub.nodes[0].id();
            doc.scene.restore_subtree(entry.sub)?;
            for (c, t, idx) in entry.moved_children {
                doc.scene.set_parent(c, Some(id), false, Some(idx))?;
                doc.scene.node_mut(c)?.transform = t;
            }
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Duplicate objects (deep copy of their subtrees, placed after the
/// originals and moved by a world-space offset). Duplicates get selected.
pub struct DuplicateObjects {
    targets: Option<Vec<Uuid>>,
    offset: Vec3,
    roots: Vec<Uuid>,
    removed: Vec<RemovedSubtree>,
    desc: String,
}

impl DuplicateObjects {
    pub fn new(targets: Option<Vec<Uuid>>, offset: Vec3) -> CoreResult<DuplicateObjects> {
        if !offset.is_finite() {
            return Err(CoreError::param("offset", "must be finite"));
        }
        Ok(DuplicateObjects {
            targets,
            offset,
            roots: Vec::new(),
            removed: Vec::new(),
            desc: "Duplicate objects".into(),
        })
    }

    /// Ids of the duplicated root nodes (after the first apply).
    pub fn created_ids(&self) -> &[Uuid] {
        &self.roots
    }

    fn first_apply(&mut self, doc: &mut Document) -> CoreResult<()> {
        let ids = resolve_objects(doc, self.targets.as_deref())?;
        let ids: Vec<Uuid> = ids
            .iter()
            .copied()
            .filter(|&i| !ids.iter().any(|&a| a != i && doc.scene.is_ancestor(a, i)))
            .collect();
        let mut roots = Vec::new();
        for &src in &ids {
            let mut order = vec![src];
            order.extend(doc.scene.descendants(src));
            let mut map: std::collections::HashMap<Uuid, Uuid> = Default::default();
            for (k, old) in order.iter().enumerate() {
                let orig = doc.scene.node(*old)?.clone();
                let mut copy = Node::with_id(Uuid::new_v4(), doc.scene.unique_name(&orig.name));
                copy.transform = orig.transform;
                copy.mesh = orig.mesh.clone();
                copy.material = orig.material.clone();
                copy.visible = orig.visible;
                map.insert(*old, copy.id());
                if k == 0 {
                    let parent = orig.parent();
                    let index = doc.scene.sibling_index(src)? + 1;
                    let pw = parent_world(doc, src)?;
                    let world = doc.scene.world_transform(src)?;
                    copy.transform =
                        pw.inverse() * Affine3A::from_translation(self.offset) * world;
                    let id = doc.scene.insert_node(copy, parent, Some(index))?;
                    roots.push(id);
                } else {
                    let parent = map[&orig.parent().expect("descendant has parent")];
                    doc.scene.add_node(copy, Some(parent))?;
                }
            }
        }
        self.desc = format!("Duplicate {}", names(doc, &ids));
        self.roots = roots;
        Ok(())
    }
}

impl Command for DuplicateObjects {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.roots.is_empty() {
            self.first_apply(doc)?;
        } else {
            while let Some(sub) = self.removed.pop() {
                doc.scene.restore_subtree(sub)?;
            }
        }
        doc.selection.set_objects(self.roots.iter().copied());
        let mut s = ChangeSummary::new();
        for &r in &self.roots {
            s = s.created(&doc.scene, r)?;
            for d in doc.scene.descendants(r) {
                s = s.created(&doc.scene, d)?;
            }
        }
        Ok(s)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        if self.roots.is_empty() {
            return Err(not_applied());
        }
        self.removed.clear();
        for &r in self.roots.iter().rev() {
            self.removed.push(doc.scene.remove_subtree(r)?);
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Rename an object (the name is made unique).
pub struct RenameObject {
    target: Option<Uuid>,
    name: String,
    state: Option<(Uuid, String, String)>,
}

impl RenameObject {
    pub fn new(target: Option<Uuid>, name: impl Into<String>) -> CoreResult<RenameObject> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(CoreError::param("name", "must not be empty"));
        }
        Ok(RenameObject {
            target,
            name,
            state: None,
        })
    }
}

impl Command for RenameObject {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.state.is_none() {
            let id = resolve_object(doc, self.target)?;
            let old = name_of(doc, id);
            let new = if old == self.name {
                old.clone()
            } else {
                doc.scene.unique_name(&self.name)
            };
            self.state = Some((id, old, new));
        }
        let (id, _, new) = self.state.clone().expect("resolved");
        doc.scene.node_mut(id)?.name = new;
        ChangeSummary::new().modified(&doc.scene, id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let (id, old, _) = self.state.clone().ok_or_else(not_applied)?;
        doc.scene.node_mut(id)?.name = old;
        Ok(())
    }

    fn describe(&self) -> String {
        match &self.state {
            Some((_, old, new)) => format!("Rename '{old}' to '{new}'"),
            None => format!("Rename object to '{}'", self.name),
        }
    }
}

/// Move an object under another parent (or to the top level).
pub struct Reparent {
    target: Option<Uuid>,
    parent: Option<Uuid>,
    keep_world: bool,
    state: Option<(Uuid, Option<Uuid>, usize, Affine3A)>,
    desc: String,
}

impl Reparent {
    pub fn new(target: Option<Uuid>, parent: Option<Uuid>, keep_world_transform: bool) -> Reparent {
        Reparent {
            target,
            parent,
            keep_world: keep_world_transform,
            state: None,
            desc: "Reparent object".into(),
        }
    }
}

impl Command for Reparent {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let id = match &self.state {
            Some(s) => s.0,
            None => resolve_object(doc, self.target)?,
        };
        let n = doc.scene.node(id)?;
        let state = (id, n.parent(), doc.scene.sibling_index(id)?, n.transform);
        doc.scene.set_parent(id, self.parent, self.keep_world, None)?;
        self.desc = match self.parent {
            Some(p) => format!("Parent '{}' to '{}'", name_of(doc, id), name_of(doc, p)),
            None => format!("Clear parent of '{}'", name_of(doc, id)),
        };
        self.state = Some(state);
        ChangeSummary::new().modified(&doc.scene, id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let (id, old_parent, idx, t) = self.state.ok_or_else(not_applied)?;
        doc.scene.set_parent(id, old_parent, false, Some(idx))?;
        doc.scene.node_mut(id)?.transform = t;
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Assign (or clear) the material of objects.
pub struct SetMaterial {
    targets: Option<Vec<Uuid>>,
    material: Option<Material>,
    old: Vec<(Uuid, Option<Material>)>,
    desc: String,
}

impl SetMaterial {
    pub fn new(targets: Option<Vec<Uuid>>, material: Option<Material>) -> CoreResult<SetMaterial> {
        if let Some(m) = &material {
            m.validate()?;
        }
        Ok(SetMaterial {
            targets,
            material,
            old: Vec::new(),
            desc: "Set material".into(),
        })
    }
}

impl Command for SetMaterial {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let ids: Vec<Uuid> = if self.old.is_empty() {
            let ids = resolve_objects(doc, self.targets.as_deref())?;
            self.desc = match &self.material {
                Some(m) => format!("Set material '{}' on {}", m.name, names(doc, &ids)),
                None => format!("Clear material of {}", names(doc, &ids)),
            };
            ids
        } else {
            self.old.iter().map(|(i, _)| *i).collect()
        };
        let mut old = Vec::with_capacity(ids.len());
        for &id in &ids {
            old.push((id, doc.scene.node(id)?.material.clone()));
        }
        let mut s = ChangeSummary::new();
        for &id in &ids {
            doc.scene.node_mut(id)?.material = self.material.clone();
            s = s.modified(&doc.scene, id)?;
        }
        self.old = old;
        Ok(s)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        if self.old.is_empty() {
            return Err(not_applied());
        }
        for (id, m) in &self.old {
            doc.scene.node_mut(*id)?.material = m.clone();
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Show or hide objects.
pub struct SetVisibility {
    targets: Option<Vec<Uuid>>,
    visible: bool,
    old: Vec<(Uuid, bool)>,
    desc: String,
}

impl SetVisibility {
    pub fn new(targets: Option<Vec<Uuid>>, visible: bool) -> SetVisibility {
        SetVisibility {
            targets,
            visible,
            old: Vec::new(),
            desc: "Set visibility".into(),
        }
    }
}

impl Command for SetVisibility {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let ids: Vec<Uuid> = if self.old.is_empty() {
            let ids = resolve_objects(doc, self.targets.as_deref())?;
            self.desc = format!(
                "{} {}",
                if self.visible { "Show" } else { "Hide" },
                names(doc, &ids)
            );
            ids
        } else {
            self.old.iter().map(|(i, _)| *i).collect()
        };
        let mut old = Vec::new();
        for &id in &ids {
            old.push((id, doc.scene.node(id)?.visible));
        }
        let mut s = ChangeSummary::new();
        for &id in &ids {
            doc.scene.node_mut(id)?.visible = self.visible;
            s = s.modified(&doc.scene, id)?;
        }
        self.old = old;
        Ok(s)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        if self.old.is_empty() {
            return Err(not_applied());
        }
        for (id, v) in &self.old {
            doc.scene.node_mut(*id)?.visible = *v;
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

// ---------------------------------------------------------------------------
// Mesh editing
// ---------------------------------------------------------------------------

/// Merge strategy for [`MeshOp::Merge`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergeKind {
    Center,
    Distance { threshold: f32 },
}

/// A mesh-level operation (parameters only; element lists `None` mean
/// "use the current element selection of the object").
#[derive(Debug, Clone, PartialEq)]
pub enum MeshOp {
    Extrude {
        faces: Option<Vec<u32>>,
        distance: f32,
        offset: Option<Vec3>,
    },
    Inset {
        faces: Option<Vec<u32>>,
        thickness: f32,
        depth: f32,
        individual: bool,
    },
    Bevel {
        edges: Option<Vec<u32>>,
        width: f32,
    },
    LoopCut {
        edge: u32,
        cuts: u32,
        factor: f32,
    },
    Subdivide {
        levels: u32,
    },
    Merge {
        vertices: Option<Vec<u32>>,
        kind: MergeKind,
    },
    Delete {
        kind: Option<ElementKind>,
        indices: Option<Vec<u32>>,
    },
    Mirror {
        axis: crate::math::Axis,
        merge: bool,
        threshold: f32,
    },
    RecalculateNormals {
        inside: bool,
    },
    FlipNormals {
        faces: Option<Vec<u32>>,
    },
    TransformElements {
        kind: Option<ElementKind>,
        indices: Option<Vec<u32>>,
        matrix_about_pivot: Affine3A,
        pivot: Option<Vec3>,
    },
    /// Replace the mesh entirely.
    Replace(Mesh),
}

impl MeshOp {
    fn name(&self) -> &'static str {
        match self {
            MeshOp::Extrude { .. } => "Extrude",
            MeshOp::Inset { .. } => "Inset",
            MeshOp::Bevel { .. } => "Bevel",
            MeshOp::LoopCut { .. } => "Loop cut",
            MeshOp::Subdivide { .. } => "Subdivide",
            MeshOp::Merge { .. } => "Merge vertices",
            MeshOp::Delete { .. } => "Delete",
            MeshOp::Mirror { .. } => "Mirror",
            MeshOp::RecalculateNormals { .. } => "Recalculate normals",
            MeshOp::FlipNormals { .. } => "Flip normals",
            MeshOp::TransformElements { .. } => "Transform elements",
            MeshOp::Replace(_) => "Set mesh",
        }
    }

    fn faces_or_selection(
        mesh: &Mesh,
        explicit: &Option<Vec<u32>>,
        sel: &ElementSelection,
    ) -> CoreResult<Vec<FaceId>> {
        let faces: Vec<FaceId> = match explicit {
            Some(v) => {
                sel::check_indices(mesh, ElementKind::Face, v)?;
                v.iter().map(|&i| FaceId(i)).collect()
            }
            None => sel.resolved_faces(mesh),
        };
        if faces.is_empty() {
            return Err(CoreError::EmptySelection(
                "no faces selected; pass `faces` or select faces first".into(),
            ));
        }
        Ok(faces)
    }

    /// Run the operation. Returns the result and a short detail string.
    fn run(&self, mesh: &Mesh, sel: &ElementSelection, mode: SelectMode) -> CoreResult<(OpResult, String)> {
        let empty = ElementSelection::default;
        match self {
            MeshOp::Extrude {
                faces,
                distance,
                offset,
            } => {
                let f = Self::faces_or_selection(mesh, faces, sel)?;
                let d = plural(f.len(), "face");
                Ok((ops::extrude_faces(mesh, &f, *distance, *offset)?, d))
            }
            MeshOp::Inset {
                faces,
                thickness,
                depth,
                individual,
            } => {
                let f = Self::faces_or_selection(mesh, faces, sel)?;
                let d = plural(f.len(), "face");
                Ok((ops::inset_faces(mesh, &f, *thickness, *depth, *individual)?, d))
            }
            MeshOp::Bevel { edges, width } => {
                let e: Vec<EdgeId> = match edges {
                    Some(v) => {
                        sel::check_indices(mesh, ElementKind::Edge, v)?;
                        v.iter().map(|&i| EdgeId(i)).collect()
                    }
                    None => sel.resolved_edges(mesh),
                };
                if e.is_empty() {
                    return Err(CoreError::EmptySelection(
                        "no edges selected; pass `edges` or select edges first".into(),
                    ));
                }
                let d = plural(e.len(), "edge");
                Ok((ops::bevel_edges(mesh, &e, *width)?, d))
            }
            MeshOp::LoopCut { edge, cuts, factor } => {
                let r = ops::loop_cut(mesh, EdgeId(*edge), *cuts, *factor)?;
                Ok((r, plural(*cuts as usize, "cut")))
            }
            MeshOp::Subdivide { levels } => {
                let m = ops::subdivide_catmull_clark(mesh, *levels)?;
                Ok((
                    OpResult {
                        mesh: m,
                        selection: empty(),
                    },
                    plural(*levels as usize, "level"),
                ))
            }
            MeshOp::Merge { vertices, kind } => {
                let verts: Option<Vec<VertexId>> = match vertices {
                    Some(v) => {
                        sel::check_indices(mesh, ElementKind::Vertex, v)?;
                        Some(v.iter().map(|&i| VertexId(i)).collect())
                    }
                    None => {
                        let r = sel.resolved_vertices(mesh);
                        (!r.is_empty()).then_some(r)
                    }
                };
                match kind {
                    MergeKind::Center => {
                        let v = verts.ok_or_else(|| {
                            CoreError::EmptySelection("no vertices selected to merge".into())
                        })?;
                        let d = format!("{} at center", plural(v.len(), "vertex"));
                        Ok((ops::merge_at_center(mesh, &v)?, d))
                    }
                    MergeKind::Distance { threshold } => {
                        let (r, removed) = ops::merge_by_distance(mesh, verts.as_deref(), *threshold)?;
                        Ok((r, format!("by distance ({removed} removed)")))
                    }
                }
            }
            MeshOp::Delete { kind, indices } => {
                let kind = kind.or(mode.element_kind()).or_else(|| {
                    if !sel.faces.is_empty() {
                        Some(ElementKind::Face)
                    } else if !sel.edges.is_empty() {
                        Some(ElementKind::Edge)
                    } else if !sel.vertices.is_empty() {
                        Some(ElementKind::Vertex)
                    } else {
                        None
                    }
                });
                let kind = kind.ok_or_else(|| {
                    CoreError::EmptySelection("nothing to delete; pass `kind` and `indices`".into())
                })?;
                let idx: Vec<u32> = match indices {
                    Some(v) => {
                        sel::check_indices(mesh, kind, v)?;
                        v.clone()
                    }
                    None => match kind {
                        ElementKind::Vertex => sel.resolved_vertices(mesh).into_iter().map(|v| v.0).collect(),
                        ElementKind::Edge => sel.resolved_edges(mesh).into_iter().map(|e| e.0).collect(),
                        ElementKind::Face => sel.resolved_faces(mesh).into_iter().map(|f| f.0).collect(),
                    },
                };
                let word = match kind {
                    ElementKind::Vertex => "vertex",
                    ElementKind::Edge => "edge",
                    ElementKind::Face => "face",
                };
                let d = plural(idx.len(), word);
                let m = ops::delete_elements(mesh, kind, &idx)?;
                Ok((
                    OpResult {
                        mesh: m,
                        selection: empty(),
                    },
                    d,
                ))
            }
            MeshOp::Mirror {
                axis,
                merge,
                threshold,
            } => {
                let m = ops::mirror(mesh, *axis, *merge, *threshold)?;
                Ok((
                    OpResult {
                        mesh: m,
                        selection: empty(),
                    },
                    format!("across {axis:?}"),
                ))
            }
            MeshOp::RecalculateNormals { inside } => {
                let m = ops::recalculate_normals(mesh, *inside)?;
                Ok((
                    OpResult {
                        mesh: m,
                        selection: sel.clone(),
                    },
                    if *inside { "inside".into() } else { "outside".into() },
                ))
            }
            MeshOp::FlipNormals { faces } => {
                let f: Vec<FaceId> = match faces {
                    Some(v) => {
                        sel::check_indices(mesh, ElementKind::Face, v)?;
                        v.iter().map(|&i| FaceId(i)).collect()
                    }
                    None => {
                        let r = sel.resolved_faces(mesh);
                        if r.is_empty() {
                            mesh.face_ids().collect()
                        } else {
                            r
                        }
                    }
                };
                let d = plural(f.len(), "face");
                let m = ops::flip_normals(mesh, &f)?;
                Ok((
                    OpResult {
                        mesh: m,
                        selection: sel.clone(),
                    },
                    d,
                ))
            }
            MeshOp::TransformElements {
                kind,
                indices,
                matrix_about_pivot,
                pivot,
            } => {
                let verts: Vec<VertexId> = match (kind, indices) {
                    (Some(k), Some(v)) => {
                        sel::check_indices(mesh, *k, v)?;
                        let mut tmp = ElementSelection::default();
                        tmp.apply(*k, v.iter().copied(), SelectAction::Set);
                        tmp.touched_vertices(mesh)
                    }
                    (None, Some(v)) => {
                        let k = mode.element_kind().unwrap_or(ElementKind::Vertex);
                        sel::check_indices(mesh, k, v)?;
                        let mut tmp = ElementSelection::default();
                        tmp.apply(k, v.iter().copied(), SelectAction::Set);
                        tmp.touched_vertices(mesh)
                    }
                    (_, None) => sel.touched_vertices(mesh),
                };
                if verts.is_empty() {
                    return Err(CoreError::EmptySelection(
                        "no elements selected to transform".into(),
                    ));
                }
                let p = pivot.unwrap_or_else(|| ops::vertices_center(mesh, &verts));
                let m = Affine3A::from_translation(p) * *matrix_about_pivot * Affine3A::from_translation(-p);
                let d = plural(verts.len(), "vertex");
                Ok((
                    OpResult {
                        mesh: ops::transform_vertices(mesh, &verts, &m)?,
                        selection: sel.clone(),
                    },
                    d,
                ))
            }
            MeshOp::Replace(m) => Ok((
                OpResult {
                    mesh: m.clone(),
                    selection: empty(),
                },
                format!("({} faces)", m.face_count()),
            )),
        }
    }
}

struct MeshEditState {
    id: Uuid,
    before: Mesh,
    after: Mesh,
    selection_after: ElementSelection,
}

/// Apply a [`MeshOp`] to one object's mesh. Undo restores a snapshot.
pub struct MeshEdit {
    target: Option<Uuid>,
    op: MeshOp,
    state: Option<MeshEditState>,
    desc: String,
}

impl MeshEdit {
    pub fn new(target: Option<Uuid>, op: MeshOp) -> MeshEdit {
        MeshEdit {
            desc: op.name().to_string(),
            target,
            op,
            state: None,
        }
    }
}

impl Command for MeshEdit {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.state.is_none() {
            let id = resolve_object(doc, self.target)?;
            let mesh = doc.scene.mesh(id)?;
            let empty = ElementSelection::default();
            let cur = doc.selection.elements(id).unwrap_or(&empty);
            let (res, detail) = self.op.run(mesh, cur, doc.selection.mode)?;
            self.desc = format!("{} {} on '{}'", self.op.name(), detail, name_of(doc, id));
            self.state = Some(MeshEditState {
                id,
                before: mesh.clone(),
                after: res.mesh,
                selection_after: res.selection,
            });
        }
        let st = self.state.as_ref().expect("resolved");
        *doc.scene.mesh_mut(st.id)? = st.after.clone();
        doc.selection.set_elements(st.id, st.selection_after.clone());
        ChangeSummary::new().modified(&doc.scene, st.id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let st = self.state.as_ref().ok_or_else(not_applied)?;
        *doc.scene.mesh_mut(st.id)? = st.before.clone();
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Boolean between two objects; the result replaces the target's mesh and
/// the other object is deleted unless `keep_other`.
pub struct BooleanObjects {
    target: Option<Uuid>,
    other: Uuid,
    op: BooleanOp,
    keep_other: bool,
    state: Option<(Uuid, Mesh, Mesh)>,
    removed: Option<RemovedSubtree>,
    desc: String,
}

impl BooleanObjects {
    pub fn new(target: Option<Uuid>, other: Uuid, op: BooleanOp, keep_other: bool) -> BooleanObjects {
        BooleanObjects {
            target,
            other,
            op,
            keep_other,
            state: None,
            removed: None,
            desc: format!("Boolean {op:?}"),
        }
    }
}

impl Command for BooleanObjects {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.state.is_none() {
            let id = resolve_object(doc, self.target)?;
            if id == self.other {
                return Err(CoreError::InvalidSelection(
                    "boolean needs two different objects".into(),
                ));
            }
            doc.scene.node(self.other)?;
            if !self.keep_other && doc.scene.is_ancestor(self.other, id) {
                return Err(CoreError::InvalidSelection(
                    "the other operand is an ancestor of the target; set keep_other or reparent first".into(),
                ));
            }
            let a = doc.scene.mesh(id)?;
            let b = doc.scene.mesh(self.other)?;
            let to_a = doc.scene.world_transform(id)?.inverse() * doc.scene.world_transform(self.other)?;
            let mut b_local = b.clone();
            b_local.transform(&to_a);
            let result = ops::boolean(a, &b_local, self.op)?;
            self.desc = format!(
                "Boolean {} '{}' with '{}'",
                match self.op {
                    BooleanOp::Union => "union",
                    BooleanOp::Difference => "difference",
                    BooleanOp::Intersect => "intersect",
                },
                name_of(doc, id),
                name_of(doc, self.other)
            );
            self.state = Some((id, a.clone(), result));
        }
        let (id, _, after) = self.state.as_ref().expect("resolved");
        let id = *id;
        let mut s = ChangeSummary::new();
        if !self.keep_other {
            let summary = ObjectSummary::of(&doc.scene, self.other)?;
            let mut gone = vec![summary];
            for d in doc.scene.descendants(self.other) {
                gone.push(ObjectSummary::of(&doc.scene, d)?);
            }
            self.removed = Some(doc.scene.remove_subtree(self.other)?);
            for g in gone {
                s = s.deleted(g);
            }
        }
        *doc.scene.mesh_mut(id)? = after.clone();
        doc.selection.clear_elements(id);
        doc.selection.set_objects([id]);
        s.modified(&doc.scene, id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let (id, before, _) = self.state.as_ref().ok_or_else(not_applied)?;
        *doc.scene.mesh_mut(*id)? = before.clone();
        if let Some(sub) = self.removed.take() {
            doc.scene.restore_subtree(sub)?;
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// A selection change (computed against the document when applied).
#[derive(Debug, Clone, PartialEq)]
pub enum SelectionChange {
    Mode(SelectMode),
    Objects {
        objects: Vec<Uuid>,
        action: SelectAction,
    },
    Active(Uuid),
    All {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
    },
    None {
        object: Option<Uuid>,
    },
    Invert {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
    },
    Elements {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
        indices: Vec<u32>,
        action: SelectAction,
    },
    ByDirection {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
        direction: Vec3,
        max_angle_deg: f32,
        space: Space,
        action: SelectAction,
    },
    ByNormal {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
        reference: u32,
        max_angle_deg: f32,
        action: SelectAction,
    },
    InBox {
        object: Option<Uuid>,
        kind: Option<ElementKind>,
        aabb: Aabb,
        space: Space,
        action: SelectAction,
    },
    EdgeLoop {
        object: Option<Uuid>,
        edge: u32,
        ring: bool,
        action: SelectAction,
    },
}

/// Change the selection (undoable).
pub struct SelectCommand {
    change: SelectionChange,
    before: Option<Selection>,
    desc: String,
}

impl SelectCommand {
    pub fn new(change: SelectionChange) -> SelectCommand {
        SelectCommand {
            change,
            before: None,
            desc: "Select".into(),
        }
    }

    fn compute(&self, doc: &Document) -> CoreResult<(Selection, String)> {
        let mut s = doc.selection.clone();
        let mesh_target = |object: Option<Uuid>| -> CoreResult<(Uuid, &Mesh)> {
            let id = resolve_object(doc, object)?;
            Ok((id, doc.scene.mesh(id)?))
        };
        let desc = match &self.change {
            SelectionChange::Mode(m) => {
                s.mode = *m;
                format!("Select mode {m:?}")
            }
            SelectionChange::Objects { objects, action } => {
                for &o in objects {
                    doc.scene.node(o)?;
                }
                s.apply_objects(objects, *action);
                format!("Select objects ({action:?}) {}", names(doc, objects))
            }
            SelectionChange::Active(id) => {
                doc.scene.node(*id)?;
                s.set_active(Some(*id));
                format!("Set active object '{}'", name_of(doc, *id))
            }
            SelectionChange::All { object, kind } => {
                if kind.is_none() && s.mode == SelectMode::Object && object.is_none() {
                    s.set_objects(doc.scene.traverse());
                    "Select all objects".to_string()
                } else {
                    let k = resolve_kind(doc, *kind)?;
                    let (id, mesh) = mesh_target(*object)?;
                    s.elements_mut(id)
                        .apply(k, sel::all_elements(mesh, k), SelectAction::Set);
                    format!("Select all {k:?} elements of '{}'", name_of(doc, id))
                }
            }
            SelectionChange::None { object } => match object {
                Some(id) => {
                    doc.scene.node(*id)?;
                    s.clear_elements(*id);
                    format!("Deselect elements of '{}'", name_of(doc, *id))
                }
                None => {
                    if s.mode == SelectMode::Object {
                        s.clear_objects();
                        "Deselect all objects".to_string()
                    } else {
                        let ids: Vec<Uuid> = s.objects_with_elements().map(|(i, _)| *i).collect();
                        for id in ids {
                            s.clear_elements(id);
                        }
                        "Deselect all elements".to_string()
                    }
                }
            },
            SelectionChange::Invert { object, kind } => {
                if kind.is_none() && s.mode == SelectMode::Object && object.is_none() {
                    let all = doc.scene.traverse();
                    let inv: Vec<Uuid> = all.into_iter().filter(|i| !s.is_selected(*i)).collect();
                    s.set_objects(inv);
                    "Invert object selection".to_string()
                } else {
                    let k = resolve_kind(doc, *kind)?;
                    let (id, mesh) = mesh_target(*object)?;
                    let cur = s.elements(id).map(|e| e.set(k).clone()).unwrap_or_default();
                    let inv = sel::inverted_elements(mesh, k, &cur);
                    s.elements_mut(id).apply(k, inv, SelectAction::Set);
                    format!("Invert {k:?} selection of '{}'", name_of(doc, id))
                }
            }
            SelectionChange::Elements {
                object,
                kind,
                indices,
                action,
            } => {
                let k = resolve_kind(doc, *kind)?;
                let (id, mesh) = mesh_target(*object)?;
                sel::check_indices(mesh, k, indices)?;
                s.elements_mut(id).apply(k, indices.iter().copied(), *action);
                format!(
                    "Select {} ({action:?}) on '{}'",
                    plural(indices.len(), &format!("{k:?}").to_lowercase()),
                    name_of(doc, id)
                )
            }
            SelectionChange::ByDirection {
                object,
                kind,
                direction,
                max_angle_deg,
                space,
                action,
            } => {
                let k = resolve_kind(doc, *kind)?;
                let (id, mesh) = mesh_target(*object)?;
                let nm = match space {
                    Space::World => Some(sel::normal_matrix(&doc.scene.world_transform(id)?)),
                    Space::Local => None,
                };
                let found = sel::elements_facing(mesh, k, *direction, *max_angle_deg, nm)?;
                let n = found.len();
                s.elements_mut(id).apply(k, found, *action);
                format!(
                    "Select {} facing {:?} within {}° on '{}'",
                    plural(n, &format!("{k:?}").to_lowercase()),
                    direction.to_array(),
                    max_angle_deg,
                    name_of(doc, id)
                )
            }
            SelectionChange::ByNormal {
                object,
                kind,
                reference,
                max_angle_deg,
                action,
            } => {
                let k = resolve_kind(doc, *kind)?;
                let (id, mesh) = mesh_target(*object)?;
                let found = sel::elements_similar_normal(mesh, k, *reference, *max_angle_deg)?;
                let n = found.len();
                s.elements_mut(id).apply(k, found, *action);
                format!(
                    "Select {} with normal similar to {k:?} {reference} on '{}'",
                    plural(n, &format!("{k:?}").to_lowercase()),
                    name_of(doc, id)
                )
            }
            SelectionChange::InBox {
                object,
                kind,
                aabb,
                space,
                action,
            } => {
                let k = resolve_kind(doc, *kind)?;
                let (id, mesh) = mesh_target(*object)?;
                let m = match space {
                    Space::World => doc.scene.world_transform(id)?,
                    Space::Local => Affine3A::IDENTITY,
                };
                let found = sel::elements_in_box(mesh, k, aabb, &m);
                let n = found.len();
                s.elements_mut(id).apply(k, found, *action);
                format!(
                    "Select {} in box on '{}'",
                    plural(n, &format!("{k:?}").to_lowercase()),
                    name_of(doc, id)
                )
            }
            SelectionChange::EdgeLoop {
                object,
                edge,
                ring,
                action,
            } => {
                let (id, mesh) = mesh_target(*object)?;
                let found = sel::edge_loop_or_ring(mesh, *edge, *ring)?;
                let n = found.len();
                s.elements_mut(id).apply(ElementKind::Edge, found, *action);
                format!(
                    "Select edge {} ({}) on '{}'",
                    if *ring { "ring" } else { "loop" },
                    plural(n, "edge"),
                    name_of(doc, id)
                )
            }
        };
        s.prune(&doc.scene);
        Ok((s, desc))
    }
}

impl Command for SelectCommand {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let (new, desc) = self.compute(doc)?;
        self.before = Some(std::mem::replace(&mut doc.selection, new));
        self.desc = desc;
        Ok(ChangeSummary::new().with_selection_changed())
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        doc.selection = self.before.take().ok_or_else(not_applied)?;
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

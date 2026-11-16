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
use crate::modifier::{Modifier, ModifierKind};
use crate::ops::{self, BooleanOp, OpResult};
use crate::scene::{Material, Node, RemovedSubtree, Scene};
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
    let n: Vec<String> = ids
        .iter()
        .map(|&i| format!("'{}'", name_of(doc, i)))
        .collect();
    n.join(", ")
}

fn not_applied() -> CoreError {
    CoreError::CommandState("undo called before apply".into())
}

fn kind_word(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Vertex => "vertex",
        ElementKind::Edge => "edge",
        ElementKind::Face => "face",
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else if word == "vertex" {
        format!("{n} vertices")
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
                            Affine3A::from_translation(p)
                                * r
                                * Affine3A::from_translation(-p)
                                * world,
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
                        Affine3A::from_translation(*p)
                            * s
                            * Affine3A::from_translation(-*p)
                            * world,
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
                .filter(|&i| {
                    !relative || !ids.iter().any(|&a| a != i && doc.scene.is_ancestor(a, i))
                })
                .collect();
            let mut resolved = Vec::with_capacity(ids.len());
            for &id in &ids {
                let new = self.compute(doc, id)?;
                if !new.is_finite() {
                    return Err(CoreError::param(
                        "transform",
                        "produces a non-finite matrix",
                    ));
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
                    copy.transform = pw.inverse() * Affine3A::from_translation(self.offset) * world;
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
        doc.scene
            .set_parent(id, self.parent, self.keep_world, None)?;
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
        segments: u32,
    },
    Solidify {
        thickness: f32,
    },
    BridgeEdgeLoops {
        edges: Option<Vec<u32>>,
    },
    FillHoles {
        edges: Option<Vec<u32>>,
        fan: bool,
    },
    Dissolve {
        kind: Option<ElementKind>,
        indices: Option<Vec<u32>>,
    },
    Triangulate {
        faces: Option<Vec<u32>>,
    },
    TrisToQuads {
        faces: Option<Vec<u32>>,
        max_angle_deg: f32,
    },
    Poke {
        faces: Option<Vec<u32>>,
        offset: f32,
    },
    Smooth {
        vertices: Option<Vec<u32>>,
        factor: f32,
        iterations: u32,
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
            MeshOp::Solidify { .. } => "Solidify",
            MeshOp::BridgeEdgeLoops { .. } => "Bridge edge loops",
            MeshOp::FillHoles { .. } => "Fill holes",
            MeshOp::Dissolve { .. } => "Dissolve",
            MeshOp::Triangulate { .. } => "Triangulate",
            MeshOp::TrisToQuads { .. } => "Triangles to quads",
            MeshOp::Poke { .. } => "Poke",
            MeshOp::Smooth { .. } => "Smooth vertices",
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

    /// Element kind (explicit, from the select mode or the non-empty part of
    /// the selection) and indices (explicit or selected).
    fn kind_and_indices(
        mesh: &Mesh,
        sel: &ElementSelection,
        mode: SelectMode,
        kind: Option<ElementKind>,
        indices: &Option<Vec<u32>>,
        verb: &str,
    ) -> CoreResult<(ElementKind, Vec<u32>)> {
        let inferred = if !sel.faces.is_empty() {
            Some(ElementKind::Face)
        } else if !sel.edges.is_empty() {
            Some(ElementKind::Edge)
        } else if !sel.vertices.is_empty() {
            Some(ElementKind::Vertex)
        } else {
            None
        };
        let kind = kind.or(mode.element_kind()).or(inferred);
        let kind = kind.ok_or_else(|| {
            CoreError::EmptySelection(format!("nothing to {verb}; pass `kind` and `indices`"))
        })?;
        let idx: Vec<u32> = match indices {
            Some(v) => {
                sel::check_indices(mesh, kind, v)?;
                v.clone()
            }
            None => match kind {
                ElementKind::Vertex => sel
                    .resolved_vertices(mesh)
                    .into_iter()
                    .map(|v| v.0)
                    .collect(),
                ElementKind::Edge => sel.resolved_edges(mesh).into_iter().map(|e| e.0).collect(),
                ElementKind::Face => sel.resolved_faces(mesh).into_iter().map(|f| f.0).collect(),
            },
        };
        Ok((kind, idx))
    }

    /// Explicit faces, else the selected faces, else all faces.
    fn faces_or_all(
        mesh: &Mesh,
        explicit: &Option<Vec<u32>>,
        sel: &ElementSelection,
    ) -> CoreResult<Vec<FaceId>> {
        if explicit.is_none() && sel.resolved_faces(mesh).is_empty() {
            return Ok(mesh.face_ids().collect());
        }
        Self::faces_or_selection(mesh, explicit, sel)
    }

    fn edges_or_selection(
        mesh: &Mesh,
        explicit: &Option<Vec<u32>>,
        sel: &ElementSelection,
    ) -> CoreResult<Vec<EdgeId>> {
        Ok(match explicit {
            Some(v) => {
                sel::check_indices(mesh, ElementKind::Edge, v)?;
                v.iter().map(|&i| EdgeId(i)).collect()
            }
            None => sel.resolved_edges(mesh),
        })
    }

    /// Run the operation. Returns the result and a short detail string.
    fn run(
        &self,
        mesh: &Mesh,
        sel: &ElementSelection,
        mode: SelectMode,
    ) -> CoreResult<(OpResult, String)> {
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
                Ok((
                    ops::inset_faces(mesh, &f, *thickness, *depth, *individual)?,
                    d,
                ))
            }
            MeshOp::Bevel {
                edges,
                width,
                segments,
            } => {
                let e = Self::edges_or_selection(mesh, edges, sel)?;
                if e.is_empty() {
                    return Err(CoreError::EmptySelection(
                        "no edges selected; pass `edges` or select edges first".into(),
                    ));
                }
                let d = format!(
                    "{} ({})",
                    plural(e.len(), "edge"),
                    plural(*segments as usize, "segment")
                );
                Ok((ops::bevel_edges(mesh, &e, *width, *segments)?, d))
            }
            MeshOp::Solidify { thickness } => Ok((
                OpResult {
                    mesh: ops::solidify(mesh, *thickness)?,
                    selection: empty(),
                },
                format!("({thickness})"),
            )),
            MeshOp::BridgeEdgeLoops { edges } => {
                let e = Self::edges_or_selection(mesh, edges, sel)?;
                let r = ops::bridge_edge_loops(mesh, &e)?;
                let d = plural(r.selection.faces.len(), "face");
                Ok((r, d))
            }
            MeshOp::FillHoles { edges, fan } => {
                let e = Self::edges_or_selection(mesh, edges, sel)?;
                let r = ops::fill_holes(mesh, (!e.is_empty()).then_some(&e[..]), *fan)?;
                let d = plural(r.selection.faces.len(), "face");
                Ok((r, d))
            }
            MeshOp::Dissolve { kind, indices } => {
                let (kind, idx) =
                    Self::kind_and_indices(mesh, sel, mode, *kind, indices, "dissolve")?;
                let d = plural(idx.len(), kind_word(kind));
                Ok((ops::dissolve(mesh, kind, &idx)?, d))
            }
            MeshOp::Triangulate { faces } => {
                let f = Self::faces_or_all(mesh, faces, sel)?;
                let d = plural(f.len(), "face");
                Ok((ops::triangulate_faces(mesh, &f)?, d))
            }
            MeshOp::TrisToQuads {
                faces,
                max_angle_deg,
            } => {
                let f = Self::faces_or_all(mesh, faces, sel)?;
                let d = plural(f.len(), "face");
                Ok((ops::tris_to_quads(mesh, &f, *max_angle_deg)?, d))
            }
            MeshOp::Poke { faces, offset } => {
                let f = Self::faces_or_selection(mesh, faces, sel)?;
                let d = plural(f.len(), "face");
                Ok((ops::poke_faces(mesh, &f, *offset)?, d))
            }
            MeshOp::Smooth {
                vertices,
                factor,
                iterations,
            } => {
                let v: Vec<VertexId> = match vertices {
                    Some(v) => {
                        sel::check_indices(mesh, ElementKind::Vertex, v)?;
                        v.iter().map(|&i| VertexId(i)).collect()
                    }
                    None => {
                        let t = sel.touched_vertices(mesh);
                        if t.is_empty() {
                            mesh.vertex_ids().collect()
                        } else {
                            t
                        }
                    }
                };
                let d = format!("{} x{iterations}", plural(v.len(), "vertex"));
                Ok((
                    OpResult {
                        mesh: ops::smooth_vertices(mesh, &v, *factor, *iterations)?,
                        selection: sel.clone(),
                    },
                    d,
                ))
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
                        let (r, removed) =
                            ops::merge_by_distance(mesh, verts.as_deref(), *threshold)?;
                        Ok((r, format!("by distance ({removed} removed)")))
                    }
                }
            }
            MeshOp::Delete { kind, indices } => {
                let (kind, idx) =
                    Self::kind_and_indices(mesh, sel, mode, *kind, indices, "delete")?;
                let d = plural(idx.len(), kind_word(kind));
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
                    if *inside {
                        "inside".into()
                    } else {
                        "outside".into()
                    },
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
                let m = Affine3A::from_translation(p)
                    * *matrix_about_pivot
                    * Affine3A::from_translation(-p);
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

/// Undo data of a mesh edit: full snapshots, or only the moved vertex
/// positions for edits that keep the topology (element transforms, smoothing).
enum MeshSnapshot {
    Full {
        before: Mesh,
        after: Mesh,
    },
    Positions {
        verts: Vec<VertexId>,
        before: Vec<Vec3>,
        after: Vec<Vec3>,
    },
}

struct MeshEditState {
    id: Uuid,
    snapshot: MeshSnapshot,
    selection_after: ElementSelection,
}

fn set_positions(mesh: &mut Mesh, verts: &[VertexId], pos: &[Vec3]) {
    for (&v, &p) in verts.iter().zip(pos) {
        mesh.set_position(v, p);
    }
    mesh.recalc_normals();
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
        if let Some(st) = self.state.as_ref() {
            let mesh = doc.scene.mesh_mut(st.id)?;
            match &st.snapshot {
                MeshSnapshot::Full { after, .. } => *mesh = after.clone(),
                MeshSnapshot::Positions { verts, after, .. } => set_positions(mesh, verts, after),
            }
            doc.selection
                .set_elements(st.id, st.selection_after.clone());
            return ChangeSummary::new().modified(&doc.scene, st.id);
        }
        let id = resolve_object(doc, self.target)?;
        let mesh = doc.scene.mesh(id)?;
        let empty = ElementSelection::default();
        let cur = doc.selection.elements(id).unwrap_or(&empty);
        let (res, detail) = self.op.run(mesh, cur, doc.selection.mode)?;
        self.desc = format!("{} {} on '{}'", self.op.name(), detail, name_of(doc, id));
        let positions_only = matches!(
            self.op,
            MeshOp::TransformElements { .. } | MeshOp::Smooth { .. }
        ) && res.mesh.vertex_count() == mesh.vertex_count();
        let snapshot = if positions_only {
            let verts: Vec<VertexId> = mesh
                .vertex_ids()
                .filter(|&v| mesh.position(v) != res.mesh.position(v))
                .collect();
            MeshSnapshot::Positions {
                before: verts.iter().map(|&v| mesh.position(v)).collect(),
                after: verts.iter().map(|&v| res.mesh.position(v)).collect(),
                verts,
            }
        } else {
            MeshSnapshot::Full {
                before: mesh.clone(),
                after: res.mesh.clone(),
            }
        };
        *doc.scene.mesh_mut(id)? = res.mesh;
        doc.selection.set_elements(id, res.selection.clone());
        self.state = Some(MeshEditState {
            id,
            snapshot,
            selection_after: res.selection,
        });
        ChangeSummary::new().modified(&doc.scene, id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let st = self.state.as_ref().ok_or_else(not_applied)?;
        let mesh = doc.scene.mesh_mut(st.id)?;
        match &st.snapshot {
            MeshSnapshot::Full { before, .. } => *mesh = before.clone(),
            MeshSnapshot::Positions { verts, before, .. } => set_positions(mesh, verts, before),
        }
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
    pub fn new(
        target: Option<Uuid>,
        other: Uuid,
        op: BooleanOp,
        keep_other: bool,
    ) -> BooleanObjects {
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
            let to_a =
                doc.scene.world_transform(id)?.inverse() * doc.scene.world_transform(self.other)?;
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
// Modifiers and shading
// ---------------------------------------------------------------------------

/// A change to a node's modifier stack.
#[derive(Debug, Clone, PartialEq)]
pub enum ModifierOp {
    /// Insert at `index` (default: end of the stack).
    Add {
        modifier: Modifier,
        index: Option<usize>,
    },
    Remove {
        index: usize,
    },
    /// Replace the type/parameters and/or the enabled flag.
    Set {
        index: usize,
        kind: Option<ModifierKind>,
        enabled: Option<bool>,
    },
    /// Move the modifier at `from` so it ends up at `to`.
    Move {
        from: usize,
        to: usize,
    },
    /// Bake one modifier (applied to the base mesh) or, with `None`, the
    /// whole enabled stack into the base mesh and remove it.
    Apply {
        index: Option<usize>,
    },
}

struct ModifierState {
    id: Uuid,
    before: Vec<Modifier>,
    after: Vec<Modifier>,
    mesh: Option<(Mesh, Mesh)>,
}

/// Edit one object's modifier stack (undo restores the stack and, for
/// apply, the base mesh).
pub struct ModifierEdit {
    target: Option<Uuid>,
    op: ModifierOp,
    state: Option<ModifierState>,
    desc: String,
}

fn check_modifier_index(len: usize, index: usize, name: &str) -> CoreResult<()> {
    if index < len {
        Ok(())
    } else if len == 0 {
        Err(CoreError::param(name, "the object has no modifiers"))
    } else {
        Err(CoreError::param(
            name,
            format!("modifier index {index} out of range (stack has {len})"),
        ))
    }
}

impl ModifierEdit {
    pub fn new(target: Option<Uuid>, op: ModifierOp) -> CoreResult<ModifierEdit> {
        match &op {
            ModifierOp::Add { modifier, .. } => modifier.kind.validate()?,
            ModifierOp::Set { kind: Some(k), .. } => k.validate()?,
            ModifierOp::Set {
                kind: None,
                enabled: None,
                ..
            } => {
                return Err(CoreError::param(
                    "modifier",
                    "pass `modifier` and/or `enabled` to change",
                ))
            }
            _ => {}
        }
        Ok(ModifierEdit {
            target,
            op,
            state: None,
            desc: "Modifier".into(),
        })
    }

    fn compute(&self, doc: &Document) -> CoreResult<ModifierState> {
        let id = resolve_object(doc, self.target)?;
        let node = doc.scene.node(id)?;
        let base = node.mesh.as_ref().ok_or(CoreError::NoMesh(id))?;
        let before = node.modifiers.clone();
        let mut after = before.clone();
        let mut mesh = None;
        match &self.op {
            ModifierOp::Add { modifier, index } => {
                let i = index.unwrap_or(after.len());
                if i > after.len() {
                    return Err(CoreError::param(
                        "index",
                        format!("must be <= {} (stack length)", after.len()),
                    ));
                }
                after.insert(i, modifier.clone());
            }
            ModifierOp::Remove { index } => {
                check_modifier_index(after.len(), *index, "index")?;
                after.remove(*index);
            }
            ModifierOp::Set {
                index,
                kind,
                enabled,
            } => {
                check_modifier_index(after.len(), *index, "index")?;
                if let Some(k) = kind {
                    after[*index].kind = k.clone();
                }
                if let Some(e) = enabled {
                    after[*index].enabled = *e;
                }
            }
            ModifierOp::Move { from, to } => {
                check_modifier_index(after.len(), *from, "from")?;
                check_modifier_index(after.len(), *to, "to")?;
                let m = after.remove(*from);
                after.insert(*to, m);
            }
            ModifierOp::Apply { index: Some(i) } => {
                check_modifier_index(after.len(), *i, "index")?;
                let baked = after[*i].kind.apply(base)?;
                after.remove(*i);
                mesh = Some((base.clone(), baked));
            }
            ModifierOp::Apply { index: None } => {
                if after.is_empty() {
                    return Err(CoreError::InvalidSelection(
                        "the object has no modifiers to apply".into(),
                    ));
                }
                let mut cur = base.clone();
                for m in after.iter().filter(|m| m.enabled) {
                    cur = m.kind.apply(&cur)?;
                }
                after.clear();
                mesh = Some((base.clone(), cur));
            }
        }
        Ok(ModifierState {
            id,
            before,
            after,
            mesh,
        })
    }

    fn describe_op(&self, doc: &Document, st: &ModifierState) -> String {
        let name = name_of(doc, st.id);
        let ty = |list: &[Modifier], i: usize| {
            list.get(i)
                .map_or("modifier", |m| m.kind.type_name())
                .to_string()
        };
        match &self.op {
            ModifierOp::Add { modifier, .. } => {
                format!("Add {} modifier to '{name}'", modifier.kind.type_name())
            }
            ModifierOp::Remove { index } => {
                format!("Remove {} modifier from '{name}'", ty(&st.before, *index))
            }
            ModifierOp::Set { index, .. } => {
                format!("Set {} modifier on '{name}'", ty(&st.after, *index))
            }
            ModifierOp::Move { from, to } => format!("Move modifier {from} to {to} on '{name}'"),
            ModifierOp::Apply { index: Some(i) } => {
                format!("Apply {} modifier on '{name}'", ty(&st.before, *i))
            }
            ModifierOp::Apply { index: None } => format!("Apply all modifiers on '{name}'"),
        }
    }
}

impl Command for ModifierEdit {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.state.is_none() {
            let st = self.compute(doc)?;
            self.desc = self.describe_op(doc, &st);
            self.state = Some(st);
        }
        let st = self.state.as_ref().expect("resolved");
        let node = doc.scene.node_mut(st.id)?;
        node.modifiers = st.after.clone();
        if let Some((_, after)) = &st.mesh {
            node.mesh = Some(after.clone());
            doc.selection.clear_elements(st.id);
        }
        ChangeSummary::new().modified(&doc.scene, st.id)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        let st = self.state.as_ref().ok_or_else(not_applied)?;
        let node = doc.scene.node_mut(st.id)?;
        node.modifiers = st.before.clone();
        if let Some((before, _)) = &st.mesh {
            node.mesh = Some(before.clone());
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Set flat/smooth shading (and the auto-smooth angle) on objects.
pub struct SetShading {
    targets: Option<Vec<Uuid>>,
    smooth: bool,
    angle: Option<f32>,
    state: Option<Vec<(Uuid, bool, Option<f32>)>>,
    desc: String,
}

impl SetShading {
    pub fn new(
        targets: Option<Vec<Uuid>>,
        smooth: bool,
        auto_smooth_angle_deg: Option<f32>,
    ) -> CoreResult<SetShading> {
        if let Some(a) = auto_smooth_angle_deg {
            if !(a.is_finite() && (0.0..=180.0).contains(&a)) {
                return Err(CoreError::param(
                    "auto_smooth_angle_deg",
                    "must be in 0..=180",
                ));
            }
        }
        Ok(SetShading {
            targets,
            smooth,
            angle: auto_smooth_angle_deg,
            state: None,
            desc: "Set shading".into(),
        })
    }
}

impl Command for SetShading {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        if self.state.is_none() {
            let ids = resolve_objects(doc, self.targets.as_deref())?;
            let mut st = Vec::with_capacity(ids.len());
            for id in &ids {
                let n = doc.scene.node(*id)?;
                st.push((*id, n.shade_smooth, n.auto_smooth_angle_deg));
            }
            self.desc = format!(
                "Shade {} {}",
                if self.smooth { "smooth" } else { "flat" },
                names(doc, &ids)
            );
            self.state = Some(st);
        }
        let mut s = ChangeSummary::new();
        for &(id, _, _) in self.state.as_ref().expect("resolved") {
            let n = doc.scene.node_mut(id)?;
            n.shade_smooth = self.smooth;
            n.auto_smooth_angle_deg = self.angle;
            s = s.modified(&doc.scene, id)?;
        }
        Ok(s)
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        for &(id, smooth, angle) in self.state.as_ref().ok_or_else(not_applied)? {
            let n = doc.scene.node_mut(id)?;
            n.shade_smooth = smooth;
            n.auto_smooth_angle_deg = angle;
        }
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

// ---------------------------------------------------------------------------
// Separate / join (whole-scene snapshots: rare, structural operations)
// ---------------------------------------------------------------------------

struct SceneSwap {
    before: Scene,
    after: Scene,
    summary: ChangeSummary,
}

/// Move faces of an object into a new object (same transform, material,
/// shading and modifiers, placed right after the source).
pub struct SeparateFaces {
    target: Option<Uuid>,
    faces: Option<Vec<u32>>,
    name: Option<String>,
    state: Option<SceneSwap>,
    desc: String,
}

impl SeparateFaces {
    pub fn new(
        target: Option<Uuid>,
        faces: Option<Vec<u32>>,
        name: Option<String>,
    ) -> SeparateFaces {
        SeparateFaces {
            target,
            faces,
            name,
            state: None,
            desc: "Separate".into(),
        }
    }

    fn compute(&mut self, doc: &mut Document) -> CoreResult<SceneSwap> {
        let id = resolve_object(doc, self.target)?;
        let mesh = doc.scene.mesh(id)?;
        let empty = ElementSelection::default();
        let cur = doc.selection.elements(id).unwrap_or(&empty);
        let faces = MeshOp::faces_or_selection(mesh, &self.faces, cur)?;
        if faces.len() == mesh.face_count() {
            return Err(CoreError::InvalidSelection(
                "cannot separate every face; leave at least one face (or duplicate the object)"
                    .into(),
            ));
        }
        let (rest, taken) = ops::split_faces(mesh, &faces)?;
        let before = doc.scene.clone();
        let src = doc.scene.node(id)?.clone();
        let base = self.name.clone().unwrap_or_else(|| src.name.clone());
        let mut node = Node::new(doc.scene.unique_name(&base))
            .with_mesh(taken)
            .with_transform(src.transform);
        node.material = src.material.clone();
        node.visible = src.visible;
        node.modifiers = src.modifiers.clone();
        node.shade_smooth = src.shade_smooth;
        node.auto_smooth_angle_deg = src.auto_smooth_angle_deg;
        let index = doc.scene.sibling_index(id)? + 1;
        let new_id = doc.scene.insert_node(node, src.parent, Some(index))?;
        *doc.scene.mesh_mut(id)? = rest;
        self.desc = format!(
            "Separate {} from '{}' into '{}'",
            plural(faces.len(), "face"),
            src.name,
            name_of(doc, new_id)
        );
        let summary = ChangeSummary::new()
            .created(&doc.scene, new_id)?
            .modified(&doc.scene, id)?;
        doc.selection.clear_elements(id);
        Ok(SceneSwap {
            before,
            after: doc.scene.clone(),
            summary,
        })
    }
}

impl Command for SeparateFaces {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        match &self.state {
            Some(st) => doc.scene = st.after.clone(),
            None => {
                let before = doc.scene.clone();
                match self.compute(doc) {
                    Ok(st) => self.state = Some(st),
                    Err(e) => {
                        doc.scene = before;
                        return Err(e);
                    }
                }
            }
        }
        Ok(self.state.as_ref().expect("resolved").summary.clone())
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        doc.scene = self.state.as_ref().ok_or_else(not_applied)?.before.clone();
        Ok(())
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}

/// Merge the meshes of several objects into one target (its local space).
/// Children of joined objects are re-parented to the target keeping their
/// world transform; the joined objects are deleted. Their modifiers are not
/// carried over (apply them first to keep their effect).
pub struct JoinObjects {
    targets: Option<Vec<Uuid>>,
    into: Option<Uuid>,
    state: Option<SceneSwap>,
    desc: String,
}

impl JoinObjects {
    pub fn new(targets: Option<Vec<Uuid>>, into: Option<Uuid>) -> JoinObjects {
        JoinObjects {
            targets,
            into,
            state: None,
            desc: "Join".into(),
        }
    }

    fn compute(&mut self, doc: &mut Document) -> CoreResult<SceneSwap> {
        let mut ids = resolve_objects(doc, self.targets.as_deref())?;
        let into = match self.into {
            Some(t) => {
                doc.scene.node(t)?;
                t
            }
            None => doc
                .selection
                .active()
                .filter(|a| ids.contains(a))
                .unwrap_or(ids[0]),
        };
        if !ids.contains(&into) {
            ids.insert(0, into);
        }
        ids.dedup();
        let others: Vec<Uuid> = ids.iter().copied().filter(|&i| i != into).collect();
        if others.is_empty() {
            return Err(CoreError::InvalidSelection(
                "join needs at least two objects".into(),
            ));
        }
        doc.scene.mesh(into)?;
        for &o in &others {
            doc.scene.mesh(o)?;
            if doc.scene.is_ancestor(o, into) {
                return Err(CoreError::InvalidSelection(format!(
                    "'{}' is an ancestor of the join target; reparent first",
                    name_of(doc, o)
                )));
            }
        }
        let before = doc.scene.clone();
        let inv = doc.scene.world_transform(into)?.inverse();
        let mut merged = doc.scene.mesh(into)?.clone();
        let mut gone = Vec::new();
        for &o in &others {
            let mut m = doc.scene.mesh(o)?.clone();
            m.transform(&(inv * doc.scene.world_transform(o)?));
            merged.append(&m);
            gone.push(ObjectSummary::of(&doc.scene, o)?);
        }
        for &o in &others {
            let children: Vec<Uuid> = doc.scene.node(o)?.children().to_vec();
            for c in children {
                let target = if others.contains(&c) {
                    None
                } else {
                    Some(into)
                };
                doc.scene.set_parent(c, target, true, None)?;
            }
        }
        for &o in &others {
            doc.scene.remove_subtree(o)?;
        }
        *doc.scene.mesh_mut(into)? = merged;
        self.desc = format!(
            "Join {} into '{}'",
            gone.iter()
                .map(|g| format!("'{}'", g.name))
                .collect::<Vec<_>>()
                .join(", "),
            name_of(doc, into)
        );
        let mut summary = ChangeSummary::new().modified(&doc.scene, into)?;
        for g in gone {
            summary = summary.deleted(g);
        }
        doc.selection.clear_elements(into);
        doc.selection.set_objects([into]);
        Ok(SceneSwap {
            before,
            after: doc.scene.clone(),
            summary,
        })
    }
}

impl Command for JoinObjects {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        match &self.state {
            Some(st) => doc.scene = st.after.clone(),
            None => {
                let before = doc.scene.clone();
                match self.compute(doc) {
                    Ok(st) => self.state = Some(st),
                    Err(e) => {
                        doc.scene = before;
                        return Err(e);
                    }
                }
            }
        }
        Ok(self.state.as_ref().expect("resolved").summary.clone())
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        doc.scene = self.state.as_ref().ok_or_else(not_applied)?.before.clone();
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
                s.elements_mut(id)
                    .apply(k, indices.iter().copied(), *action);
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

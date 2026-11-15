//! What the gizmo manipulates, and how a drag becomes a command.

use forge3d_core::math::euler_deg_from_quat;
use forge3d_core::prelude::*;
use glam::{Affine3A, Quat, Vec3};
use uuid::Uuid;

use crate::actions::top_level_selection;
use crate::gizmo::{DragDelta, GizmoFrame, GizmoMode};

/// The thing a gizmo drag edits.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetKind {
    /// Whole objects (object mode).
    Objects(Vec<Uuid>),
    /// Selected mesh elements of one object (edit modes).
    Elements {
        object: Uuid,
        kind: ElementKind,
        indices: Vec<u32>,
        /// Object-to-world transform when the target was computed.
        world: Affine3A,
    },
}

/// Gizmo placement plus the edited objects / elements.
#[derive(Debug, Clone, PartialEq)]
pub struct GizmoTarget {
    pub kind: TargetKind,
    /// World-space pivot (active object origin / selected elements' centroid).
    pub origin: Vec3,
    /// Axes of the edited thing's own frame (unit length, world space).
    pub local_axes: [Vec3; 3],
}

fn columns(m: &Affine3A) -> [Vec3; 3] {
    [m.x_axis.into(), m.y_axis.into(), m.z_axis.into()]
}

impl GizmoTarget {
    /// Compute the target for the document's current selection, or `None`
    /// when nothing transformable is selected.
    pub fn compute(doc: &Document) -> Option<GizmoTarget> {
        match doc.selection.mode.element_kind() {
            None => Self::objects(doc),
            Some(kind) => Self::elements(doc, kind),
        }
    }

    fn objects(doc: &Document) -> Option<GizmoTarget> {
        let ids = top_level_selection(doc);
        if ids.is_empty() {
            return None;
        }
        let lead = doc
            .selection
            .active()
            .filter(|a| ids.contains(a))
            .unwrap_or(ids[0]);
        let world = doc.scene.world_transform(lead).ok()?;
        let cols = columns(&world);
        let local_axes = GizmoFrame::oriented(world.translation.into(), cols).axes;
        Some(GizmoTarget {
            kind: TargetKind::Objects(ids),
            origin: world.translation.into(),
            local_axes,
        })
    }

    fn elements(doc: &Document, kind: ElementKind) -> Option<GizmoTarget> {
        let object = doc.selection.active()?;
        let sel = doc.selection.elements(object)?;
        let indices: Vec<u32> = sel.set(kind).iter().copied().collect();
        if indices.is_empty() {
            return None;
        }
        let mesh = doc.scene.get(object)?.mesh.as_ref()?;
        let world = doc.scene.world_transform(object).ok()?;
        // Only the elements of the current mode are transformed.
        let mut only = ElementSelection::default();
        only.set_mut(kind).extend(indices.iter().copied());
        let verts = only.touched_vertices(mesh);
        if verts.is_empty() {
            return None;
        }
        let sum: Vec3 = verts
            .iter()
            .map(|&v| world.transform_point3(mesh.position(v)))
            .sum();
        let origin = sum / verts.len() as f32;
        let local_axes = GizmoFrame::oriented(origin, columns(&world)).axes;
        Some(GizmoTarget {
            kind: TargetKind::Elements {
                object,
                kind,
                indices,
                world,
            },
            origin,
            local_axes,
        })
    }

    /// True when the target is a single object or an element selection, i.e.
    /// the gizmo can use the target's own axes.
    pub fn single(&self) -> bool {
        match &self.kind {
            TargetKind::Objects(ids) => ids.len() == 1,
            TargetKind::Elements { .. } => true,
        }
    }

    /// Gizmo frame for a tool: translate / rotate use the world axes,
    /// scale uses the target's own axes when there is a single frame.
    pub fn frame(&self, mode: GizmoMode) -> GizmoFrame {
        match mode {
            GizmoMode::Scale if self.single() => GizmoFrame {
                origin: self.origin,
                axes: self.local_axes,
            },
            _ => GizmoFrame::world(self.origin),
        }
    }

    /// Whether every referenced object still exists in `doc`.
    pub fn is_valid(&self, doc: &Document) -> bool {
        match &self.kind {
            TargetKind::Objects(ids) => ids.iter().all(|&i| doc.scene.contains(i)),
            TargetKind::Elements { object, .. } => doc
                .scene
                .get(*object)
                .is_some_and(|n| n.mesh.is_some()),
        }
    }

    /// The command that applies `delta` (measured from the drag start) to
    /// the target as it was when the drag began.
    pub fn spec(&self, delta: DragDelta) -> CommandSpec {
        match &self.kind {
            TargetKind::Objects(ids) => match delta {
                DragDelta::Translate(t) => CommandSpec::Translate {
                    objects: Some(ids.clone()),
                    offset: t.to_array(),
                    space: Space::World,
                },
                DragDelta::Rotate { axis, angle_deg } => CommandSpec::Rotate {
                    objects: Some(ids.clone()),
                    axis: axis.to_array(),
                    angle_deg,
                    pivot: Some(self.origin.to_array()),
                    space: Space::World,
                },
                DragDelta::Scale(f) => CommandSpec::Scale {
                    objects: Some(ids.clone()),
                    factor: f.to_array(),
                    // A single object scales along its own axes about its
                    // origin; several scale in world axes about the pivot.
                    pivot: (ids.len() > 1).then(|| self.origin.to_array()),
                },
            },
            TargetKind::Elements {
                object,
                kind,
                indices,
                world,
            } => {
                let inv = world.inverse();
                let pivot = inv.transform_point3(self.origin);
                let mut spec = CommandSpec::TransformElements {
                    object: Some(*object),
                    kind: Some(*kind),
                    indices: Some(indices.clone()),
                    translation: None,
                    rotation_euler_deg: None,
                    scale: None,
                    pivot: Some(pivot.to_array()),
                };
                if let CommandSpec::TransformElements {
                    translation,
                    rotation_euler_deg,
                    scale,
                    ..
                } = &mut spec
                {
                    match delta {
                        DragDelta::Translate(t) => {
                            let v: Vec3 = (inv.matrix3 * glam::Vec3A::from(t)).into();
                            *translation = Some(v.to_array());
                        }
                        DragDelta::Rotate { axis, angle_deg } => {
                            let local: Vec3 = (inv.matrix3 * glam::Vec3A::from(axis)).into();
                            let q = Quat::from_axis_angle(
                                local.try_normalize().unwrap_or(Vec3::Z),
                                angle_deg.to_radians(),
                            );
                            *rotation_euler_deg = Some(euler_deg_from_quat(q));
                        }
                        DragDelta::Scale(f) => *scale = Some(f.to_array()),
                    }
                }
                spec
            }
        }
    }
}

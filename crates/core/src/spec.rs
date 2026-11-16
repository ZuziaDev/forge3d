//! Serializable, data-driven description of every operation.
//!
//! [`CommandSpec`] is the single entry point shared by the GUI, CLI scripts
//! and the MCP/AI layer. It is internally tagged by `"op"` (snake_case), e.g.
//!
//! ```json
//! {"op": "create_primitive", "primitive": {"type": "cube", "size": 2.0}}
//! {"op": "extrude", "faces": [1], "distance": 0.5}
//! ```
//!
//! Element-level operations take an optional `object` (default: the active
//! object) and optional element index lists (default: the object's current
//! element selection). Unknown fields are rejected so typos surface as
//! `invalid_command` errors.

use glam::{Affine3A, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command::{Batch, Command};
use crate::commands::*;
use crate::error::{CoreError, CoreResult};
use crate::math::{quat_from_euler_deg, Aabb, Axis, Space};
use crate::mesh::{ElementKind, Mesh, MeshData};
use crate::modifier::{Modifier, ModifierKind};
use crate::ops::BooleanOp;
use crate::primitives::PrimitiveSpec;
use crate::scene::{Material, Node};
use crate::selection::{SelectAction, SelectMode};

fn d_true() -> bool {
    true
}
fn d_distance() -> f32 {
    1.0
}
fn d_thickness() -> f32 {
    0.1
}
fn d_width() -> f32 {
    0.1
}
fn d_one() -> u32 {
    1
}
fn d_half() -> f32 {
    0.5
}
fn d_merge_threshold() -> f32 {
    1e-4
}
fn d_quad_angle() -> f32 {
    40.0
}
fn d_angle() -> f32 {
    30.0
}

/// How [`CommandSpec::MergeVertices`] merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// Collapse the vertices into one at their average position.
    #[default]
    Center,
    /// Weld vertices closer than `threshold`.
    Distance,
}

/// Every operation of the application, as data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandSpec {
    // ----- object creation -----
    /// Create a primitive object. It becomes the selected, active object.
    CreatePrimitive {
        primitive: PrimitiveSpec,
        /// Object name (made unique); defaults to the primitive's name.
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<Uuid>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        /// XYZ Euler angles in degrees.
        #[serde(default)]
        rotation_euler_deg: Option<[f32; 3]>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
        #[serde(default)]
        material: Option<Material>,
    },
    /// Create an object from raw vertex/face data.
    CreateMesh {
        #[serde(default)]
        name: Option<String>,
        mesh: MeshData,
        #[serde(default)]
        parent: Option<Uuid>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        #[serde(default)]
        material: Option<Material>,
    },
    /// Create an empty (group) node without mesh.
    CreateEmpty {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<Uuid>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
    },

    // ----- object transforms -----
    /// Set components of the local transform (absolute). Omitted components keep their value.
    SetTransform {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        #[serde(default)]
        rotation_euler_deg: Option<[f32; 3]>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
    },
    /// Move objects by an offset (relative).
    Translate {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        offset: [f32; 3],
        #[serde(default)]
        space: Space,
    },
    /// Rotate objects (relative) around `axis` by `angle_deg`.
    Rotate {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        axis: [f32; 3],
        angle_deg: f32,
        /// World-space pivot (world space only); default: each object's origin.
        #[serde(default)]
        pivot: Option<[f32; 3]>,
        #[serde(default)]
        space: Space,
    },
    /// Scale objects (relative) along their local axes, or about a world pivot.
    Scale {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        factor: [f32; 3],
        #[serde(default)]
        pivot: Option<[f32; 3]>,
    },

    // ----- object management -----
    /// Delete objects (default: selected) with their children unless `keep_children`.
    DeleteObjects {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        #[serde(default)]
        keep_children: bool,
    },
    /// Duplicate objects (default: selected); the copies are selected.
    DuplicateObjects {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        /// World-space offset applied to each copy.
        #[serde(default)]
        offset: [f32; 3],
    },
    /// Rename an object (default: active). Names are made unique.
    RenameObject {
        #[serde(default)]
        object: Option<Uuid>,
        name: String,
    },
    /// Parent an object (default: active) to `parent`, or clear its parent when null.
    Reparent {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        parent: Option<Uuid>,
        #[serde(default = "d_true")]
        keep_world_transform: bool,
    },
    /// Assign a material (or clear it with null).
    SetMaterial {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        #[serde(default)]
        material: Option<Material>,
    },
    /// Show or hide objects.
    SetVisibility {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        visible: bool,
    },

    // ----- mesh operations (object default: active; elements default: selection) -----
    /// Region-extrude faces along their normals by `distance`, or by `offset` (local space).
    Extrude {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_distance")]
        distance: f32,
        #[serde(default)]
        offset: Option<[f32; 3]>,
    },
    /// Inset faces by `thickness`, optionally pushing them by `depth`.
    Inset {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_thickness")]
        thickness: f32,
        #[serde(default)]
        depth: f32,
        /// Inset each face separately instead of the selection as a region.
        #[serde(default)]
        individual: bool,
    },
    /// Bevel interior edges: `segments` = 1 is a flat chamfer, more give a
    /// rounded profile.
    Bevel {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        edges: Option<Vec<u32>>,
        #[serde(default = "d_width")]
        width: f32,
        #[serde(default = "d_one")]
        segments: u32,
    },
    /// Give the whole surface a thickness (inner shell + rim on open edges).
    Solidify {
        #[serde(default)]
        object: Option<Uuid>,
        /// Wall thickness, grown against the normals (negative = outward).
        #[serde(default = "d_thickness")]
        thickness: f32,
    },
    /// Connect two boundary loops (holes) touched by `edges` with quads.
    BridgeEdgeLoops {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        edges: Option<Vec<u32>>,
    },
    /// Close holes: loops touched by `edges` (default: selection; nothing selected = every hole).
    FillHoles {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        edges: Option<Vec<u32>>,
        /// Triangle fan around a new center vertex instead of one n-gon.
        #[serde(default)]
        triangle_fan: bool,
    },
    /// Remove vertices / edges / faces merging the faces around them into n-gons.
    Dissolve {
        #[serde(default)]
        object: Option<Uuid>,
        /// Default: from the selection mode.
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
    },
    /// Split faces into triangles (default: selected faces, or all when none selected).
    Triangulate {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
    },
    /// Join adjacent triangle pairs into quads (default: selected faces, or all).
    TrisToQuads {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default = "d_quad_angle")]
        max_angle_deg: f32,
    },
    /// Replace faces by triangle fans around their centers (pushed by `offset`).
    PokeFaces {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
        #[serde(default)]
        offset: f32,
    },
    /// Laplacian smoothing of vertices (default: selection, or all when none selected).
    SmoothVertices {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        vertices: Option<Vec<u32>>,
        #[serde(default = "d_half")]
        factor: f32,
        #[serde(default = "d_one")]
        iterations: u32,
    },
    /// Move faces (default: selected) into a new object.
    Separate {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
        /// Name of the new object (default: the source name, made unique).
        #[serde(default)]
        name: Option<String>,
    },
    /// Merge objects' meshes into `into` (default: active) and delete the others.
    JoinObjects {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        #[serde(default)]
        into: Option<Uuid>,
    },

    // ----- modifiers and shading -----
    /// Append (or insert at `index`) a non-destructive modifier.
    AddModifier {
        #[serde(default)]
        object: Option<Uuid>,
        modifier: ModifierKind,
        #[serde(default = "d_true")]
        enabled: bool,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Remove the modifier at `index`.
    RemoveModifier {
        #[serde(default)]
        object: Option<Uuid>,
        index: usize,
    },
    /// Replace the parameters (`modifier`) and/or `enabled` of the modifier at `index`.
    SetModifier {
        #[serde(default)]
        object: Option<Uuid>,
        index: usize,
        #[serde(default)]
        modifier: Option<ModifierKind>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    /// Reorder the stack: move the modifier at `from` to position `to`.
    MoveModifier {
        #[serde(default)]
        object: Option<Uuid>,
        from: usize,
        to: usize,
    },
    /// Bake the modifier at `index` into the base mesh (omit `index` to apply the whole stack).
    ApplyModifier {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Flat or smooth shading; with smooth, edges sharper than `auto_smooth_angle_deg` stay hard.
    SetShading {
        #[serde(default)]
        objects: Option<Vec<Uuid>>,
        smooth: bool,
        #[serde(default)]
        auto_smooth_angle_deg: Option<f32>,
    },
    /// Insert `cuts` edge loops across the quad ring through `edge`.
    LoopCut {
        #[serde(default)]
        object: Option<Uuid>,
        edge: u32,
        #[serde(default = "d_one")]
        cuts: u32,
        /// Position of a single cut along `edge` (0..1, exclusive).
        #[serde(default = "d_half")]
        factor: f32,
    },
    /// Catmull–Clark subdivision of the whole mesh.
    Subdivide {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default = "d_one")]
        levels: u32,
    },
    /// Merge vertices at their center, or weld by distance (whole mesh when nothing is selected).
    MergeVertices {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        vertices: Option<Vec<u32>>,
        #[serde(default)]
        mode: MergeMode,
        #[serde(default = "d_merge_threshold")]
        threshold: f32,
    },
    /// Delete vertices / edges / faces.
    DeleteElements {
        #[serde(default)]
        object: Option<Uuid>,
        /// Default: from the selection mode.
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
    },
    /// Mirror geometry across the local plane perpendicular to `axis`.
    Mirror {
        #[serde(default)]
        object: Option<Uuid>,
        axis: Axis,
        #[serde(default = "d_true")]
        merge: bool,
        #[serde(default = "d_merge_threshold")]
        merge_threshold: f32,
    },
    /// Make winding consistent and normals point outward (or inward).
    RecalculateNormals {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        inside: bool,
    },
    /// Reverse face winding (default: selected faces, or all when none selected).
    FlipNormals {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        faces: Option<Vec<u32>>,
    },
    /// Move/rotate/scale mesh elements in local space around `pivot`
    /// (default: median of the affected vertices).
    TransformElements {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
        #[serde(default)]
        indices: Option<Vec<u32>>,
        #[serde(default)]
        translation: Option<[f32; 3]>,
        #[serde(default)]
        rotation_euler_deg: Option<[f32; 3]>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
        #[serde(default)]
        pivot: Option<[f32; 3]>,
    },
    /// Replace an object's mesh with raw data.
    SetMeshData {
        #[serde(default)]
        object: Option<Uuid>,
        mesh: MeshData,
    },
    /// Boolean of `object` (default: active) with `other`; the result replaces
    /// `object`'s mesh and `other` is deleted unless `keep_other`.
    Boolean {
        #[serde(default)]
        object: Option<Uuid>,
        other: Uuid,
        operation: BooleanOp,
        #[serde(default)]
        keep_other: bool,
    },

    // ----- selection -----
    SetSelectMode {
        mode: SelectMode,
    },
    SelectObjects {
        objects: Vec<Uuid>,
        #[serde(default)]
        action: SelectAction,
    },
    SetActiveObject {
        object: Uuid,
    },
    /// Select everything: all objects in object mode (no object/kind given),
    /// else all elements of `kind` on `object`.
    SelectAll {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
    },
    /// Clear the selection (objects in object mode, otherwise elements).
    SelectNone {
        #[serde(default)]
        object: Option<Uuid>,
    },
    SelectInvert {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
    },
    /// Select elements by index.
    SelectElements {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
        indices: Vec<u32>,
        #[serde(default)]
        action: SelectAction,
    },
    /// Select elements whose normal is within `max_angle_deg` of `direction`.
    SelectByDirection {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
        direction: [f32; 3],
        #[serde(default = "d_angle")]
        max_angle_deg: f32,
        #[serde(default)]
        space: Space,
        #[serde(default)]
        action: SelectAction,
    },
    /// Select elements with a normal similar to element `reference`.
    SelectByNormal {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
        reference: u32,
        #[serde(default = "d_angle")]
        max_angle_deg: f32,
        #[serde(default)]
        action: SelectAction,
    },
    /// Select elements entirely inside an axis-aligned box.
    SelectInBox {
        #[serde(default)]
        object: Option<Uuid>,
        #[serde(default)]
        kind: Option<ElementKind>,
        min: [f32; 3],
        max: [f32; 3],
        #[serde(default)]
        space: Space,
        #[serde(default)]
        action: SelectAction,
    },
    /// Select the edge loop (or ring) through an edge.
    SelectEdgeLoop {
        #[serde(default)]
        object: Option<Uuid>,
        edge: u32,
        #[serde(default)]
        ring: bool,
        #[serde(default)]
        action: SelectAction,
    },

    // ----- composition -----
    /// Run several commands atomically as one undo step.
    Batch {
        #[serde(default)]
        description: Option<String>,
        commands: Vec<CommandSpec>,
    },
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

fn check3(name: &str, a: [f32; 3]) -> CoreResult<Vec3> {
    let v = v3(a);
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CoreError::param(name, "must contain finite numbers"))
    }
}

fn local_transform(
    translation: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> CoreResult<Affine3A> {
    let t = translation
        .map(|t| check3("translation", t))
        .transpose()?
        .unwrap_or(Vec3::ZERO);
    let r = rotation
        .map(|r| check3("rotation_euler_deg", r).map(|v| quat_from_euler_deg(v.to_array())))
        .transpose()?
        .unwrap_or_default();
    let s = scale
        .map(|s| check3("scale", s))
        .transpose()?
        .unwrap_or(Vec3::ONE);
    if s.abs().min_element() < 1e-8 {
        return Err(CoreError::param("scale", "components must be non-zero"));
    }
    Ok(Affine3A::from_scale_rotation_translation(s, r, t))
}

impl CommandSpec {
    /// Parse from JSON (errors map to `invalid_command`).
    pub fn from_json(json: &str) -> CoreResult<CommandSpec> {
        serde_json::from_str(json).map_err(|e| CoreError::InvalidCommand(e.to_string()))
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("CommandSpec serializes")
    }

    /// JSON Schema of the whole command language.
    pub fn json_schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(CommandSpec)).expect("schema serializes")
    }

    /// The `op` tag of this command.
    pub fn op_name(&self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(m)) => m
                .get("op")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    /// Validate parameters and build the executable command.
    pub fn into_command(self) -> CoreResult<Box<dyn Command>> {
        Ok(match self {
            CommandSpec::CreatePrimitive {
                primitive,
                name,
                parent,
                translation,
                rotation_euler_deg,
                scale,
                material,
            } => {
                let mesh = primitive.build()?;
                if let Some(m) = &material {
                    m.validate()?;
                }
                let mut node =
                    Node::new(name.unwrap_or_else(|| primitive.default_name().to_string()))
                        .with_mesh(mesh)
                        .with_transform(local_transform(translation, rotation_euler_deg, scale)?);
                node.material = material;
                let what = primitive.default_name().to_lowercase();
                Box::new(AddNode::new(node, parent, true, what))
            }
            CommandSpec::CreateMesh {
                name,
                mesh,
                parent,
                translation,
                material,
            } => {
                let mesh = Mesh::from_mesh_data(&mesh)?;
                if let Some(m) = &material {
                    m.validate()?;
                }
                let mut node = Node::new(name.unwrap_or_else(|| "Mesh".into()))
                    .with_mesh(mesh)
                    .with_transform(local_transform(translation, None, None)?);
                node.material = material;
                Box::new(AddNode::new(node, parent, true, "mesh"))
            }
            CommandSpec::CreateEmpty {
                name,
                parent,
                translation,
            } => {
                let node = Node::new(name.unwrap_or_else(|| "Empty".into()))
                    .with_transform(local_transform(translation, None, None)?);
                Box::new(AddNode::new(node, parent, true, "empty"))
            }
            CommandSpec::SetTransform {
                objects,
                translation,
                rotation_euler_deg,
                scale,
            } => {
                let op = TransformOp::Set {
                    translation: translation.map(v3),
                    rotation: rotation_euler_deg.map(quat_from_euler_deg),
                    scale: scale.map(v3),
                };
                Box::new(TransformObjects::new(objects, op)?)
            }
            CommandSpec::Translate {
                objects,
                offset,
                space,
            } => Box::new(TransformObjects::new(
                objects,
                TransformOp::Translate {
                    offset: v3(offset),
                    space,
                },
            )?),
            CommandSpec::Rotate {
                objects,
                axis,
                angle_deg,
                pivot,
                space,
            } => Box::new(TransformObjects::new(
                objects,
                TransformOp::Rotate {
                    axis: v3(axis),
                    angle_rad: angle_deg.to_radians(),
                    pivot: pivot.map(v3),
                    space,
                },
            )?),
            CommandSpec::Scale {
                objects,
                factor,
                pivot,
            } => Box::new(TransformObjects::new(
                objects,
                TransformOp::Scale {
                    factor: v3(factor),
                    pivot: pivot.map(v3),
                },
            )?),
            CommandSpec::DeleteObjects {
                objects,
                keep_children,
            } => Box::new(DeleteObjects::new(objects, keep_children)),
            CommandSpec::DuplicateObjects { objects, offset } => {
                Box::new(DuplicateObjects::new(objects, v3(offset))?)
            }
            CommandSpec::RenameObject { object, name } => {
                Box::new(RenameObject::new(object, name)?)
            }
            CommandSpec::Reparent {
                object,
                parent,
                keep_world_transform,
            } => Box::new(Reparent::new(object, parent, keep_world_transform)),
            CommandSpec::SetMaterial { objects, material } => {
                Box::new(SetMaterial::new(objects, material)?)
            }
            CommandSpec::SetVisibility { objects, visible } => {
                Box::new(SetVisibility::new(objects, visible))
            }
            CommandSpec::Extrude {
                object,
                faces,
                distance,
                offset,
            } => {
                let offset = offset.map(|o| check3("offset", o)).transpose()?;
                Box::new(MeshEdit::new(
                    object,
                    MeshOp::Extrude {
                        faces,
                        distance,
                        offset,
                    },
                ))
            }
            CommandSpec::Inset {
                object,
                faces,
                thickness,
                depth,
                individual,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::Inset {
                    faces,
                    thickness,
                    depth,
                    individual,
                },
            )),
            CommandSpec::Bevel {
                object,
                edges,
                width,
                segments,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::Bevel {
                    edges,
                    width,
                    segments,
                },
            )),
            CommandSpec::Solidify { object, thickness } => {
                Box::new(MeshEdit::new(object, MeshOp::Solidify { thickness }))
            }
            CommandSpec::BridgeEdgeLoops { object, edges } => {
                Box::new(MeshEdit::new(object, MeshOp::BridgeEdgeLoops { edges }))
            }
            CommandSpec::FillHoles {
                object,
                edges,
                triangle_fan,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::FillHoles {
                    edges,
                    fan: triangle_fan,
                },
            )),
            CommandSpec::Dissolve {
                object,
                kind,
                indices,
            } => Box::new(MeshEdit::new(object, MeshOp::Dissolve { kind, indices })),
            CommandSpec::Triangulate { object, faces } => {
                Box::new(MeshEdit::new(object, MeshOp::Triangulate { faces }))
            }
            CommandSpec::TrisToQuads {
                object,
                faces,
                max_angle_deg,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::TrisToQuads {
                    faces,
                    max_angle_deg,
                },
            )),
            CommandSpec::PokeFaces {
                object,
                faces,
                offset,
            } => Box::new(MeshEdit::new(object, MeshOp::Poke { faces, offset })),
            CommandSpec::SmoothVertices {
                object,
                vertices,
                factor,
                iterations,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::Smooth {
                    vertices,
                    factor,
                    iterations,
                },
            )),
            CommandSpec::Separate {
                object,
                faces,
                name,
            } => Box::new(SeparateFaces::new(object, faces, name)),
            CommandSpec::JoinObjects { objects, into } => Box::new(JoinObjects::new(objects, into)),
            CommandSpec::AddModifier {
                object,
                modifier,
                enabled,
                index,
            } => Box::new(ModifierEdit::new(
                object,
                ModifierOp::Add {
                    modifier: Modifier {
                        enabled,
                        kind: modifier,
                    },
                    index,
                },
            )?),
            CommandSpec::RemoveModifier { object, index } => {
                Box::new(ModifierEdit::new(object, ModifierOp::Remove { index })?)
            }
            CommandSpec::SetModifier {
                object,
                index,
                modifier,
                enabled,
            } => Box::new(ModifierEdit::new(
                object,
                ModifierOp::Set {
                    index,
                    kind: modifier,
                    enabled,
                },
            )?),
            CommandSpec::MoveModifier { object, from, to } => {
                Box::new(ModifierEdit::new(object, ModifierOp::Move { from, to })?)
            }
            CommandSpec::ApplyModifier { object, index } => {
                Box::new(ModifierEdit::new(object, ModifierOp::Apply { index })?)
            }
            CommandSpec::SetShading {
                objects,
                smooth,
                auto_smooth_angle_deg,
            } => Box::new(SetShading::new(objects, smooth, auto_smooth_angle_deg)?),
            CommandSpec::LoopCut {
                object,
                edge,
                cuts,
                factor,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::LoopCut { edge, cuts, factor },
            )),
            CommandSpec::Subdivide { object, levels } => {
                Box::new(MeshEdit::new(object, MeshOp::Subdivide { levels }))
            }
            CommandSpec::MergeVertices {
                object,
                vertices,
                mode,
                threshold,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::Merge {
                    vertices,
                    kind: match mode {
                        MergeMode::Center => MergeKind::Center,
                        MergeMode::Distance => MergeKind::Distance { threshold },
                    },
                },
            )),
            CommandSpec::DeleteElements {
                object,
                kind,
                indices,
            } => Box::new(MeshEdit::new(object, MeshOp::Delete { kind, indices })),
            CommandSpec::Mirror {
                object,
                axis,
                merge,
                merge_threshold,
            } => Box::new(MeshEdit::new(
                object,
                MeshOp::Mirror {
                    axis,
                    merge,
                    threshold: merge_threshold,
                },
            )),
            CommandSpec::RecalculateNormals { object, inside } => {
                Box::new(MeshEdit::new(object, MeshOp::RecalculateNormals { inside }))
            }
            CommandSpec::FlipNormals { object, faces } => {
                Box::new(MeshEdit::new(object, MeshOp::FlipNormals { faces }))
            }
            CommandSpec::TransformElements {
                object,
                kind,
                indices,
                translation,
                rotation_euler_deg,
                scale,
                pivot,
            } => {
                let m = local_transform(translation, rotation_euler_deg, scale)?;
                // `local_transform` composes T·R·S; applied about the pivot.
                Box::new(MeshEdit::new(
                    object,
                    MeshOp::TransformElements {
                        kind,
                        indices,
                        matrix_about_pivot: m,
                        pivot: pivot.map(|p| check3("pivot", p)).transpose()?,
                    },
                ))
            }
            CommandSpec::SetMeshData { object, mesh } => {
                let m = Mesh::from_mesh_data(&mesh)?;
                Box::new(MeshEdit::new(object, MeshOp::Replace(m)))
            }
            CommandSpec::Boolean {
                object,
                other,
                operation,
                keep_other,
            } => Box::new(BooleanObjects::new(object, other, operation, keep_other)),
            CommandSpec::SetSelectMode { mode } => {
                Box::new(SelectCommand::new(SelectionChange::Mode(mode)))
            }
            CommandSpec::SelectObjects { objects, action } => {
                Box::new(SelectCommand::new(SelectionChange::Objects {
                    objects,
                    action,
                }))
            }
            CommandSpec::SetActiveObject { object } => {
                Box::new(SelectCommand::new(SelectionChange::Active(object)))
            }
            CommandSpec::SelectAll { object, kind } => {
                Box::new(SelectCommand::new(SelectionChange::All { object, kind }))
            }
            CommandSpec::SelectNone { object } => {
                Box::new(SelectCommand::new(SelectionChange::None { object }))
            }
            CommandSpec::SelectInvert { object, kind } => {
                Box::new(SelectCommand::new(SelectionChange::Invert { object, kind }))
            }
            CommandSpec::SelectElements {
                object,
                kind,
                indices,
                action,
            } => Box::new(SelectCommand::new(SelectionChange::Elements {
                object,
                kind,
                indices,
                action,
            })),
            CommandSpec::SelectByDirection {
                object,
                kind,
                direction,
                max_angle_deg,
                space,
                action,
            } => Box::new(SelectCommand::new(SelectionChange::ByDirection {
                object,
                kind,
                direction: check3("direction", direction)?,
                max_angle_deg,
                space,
                action,
            })),
            CommandSpec::SelectByNormal {
                object,
                kind,
                reference,
                max_angle_deg,
                action,
            } => Box::new(SelectCommand::new(SelectionChange::ByNormal {
                object,
                kind,
                reference,
                max_angle_deg,
                action,
            })),
            CommandSpec::SelectInBox {
                object,
                kind,
                min,
                max,
                space,
                action,
            } => Box::new(SelectCommand::new(SelectionChange::InBox {
                object,
                kind,
                aabb: Aabb::new(check3("min", min)?, check3("max", max)?),
                space,
                action,
            })),
            CommandSpec::SelectEdgeLoop {
                object,
                edge,
                ring,
                action,
            } => Box::new(SelectCommand::new(SelectionChange::EdgeLoop {
                object,
                edge,
                ring,
                action,
            })),
            CommandSpec::Batch {
                description,
                commands,
            } => {
                if commands.is_empty() {
                    return Err(CoreError::param(
                        "commands",
                        "batch must contain at least one command",
                    ));
                }
                let cmds = commands
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| {
                        c.into_command().map_err(|e| CoreError::BatchFailed {
                            index: i,
                            source: Box::new(e),
                        })
                    })
                    .collect::<CoreResult<Vec<_>>>()?;
                Box::new(Batch::new(description.unwrap_or_default(), cmds))
            }
        })
    }
}

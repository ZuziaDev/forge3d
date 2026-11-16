//! Translation of UI choices into [`CommandSpec`]s.
//!
//! The GUI never edits the scene itself: tool buttons, menus and parameter
//! fields are turned into command data here and executed through the
//! document like any CLI or AI command.

use forge3d_core::mesh::ElementKind;
use forge3d_core::prelude::*;
use forge3d_core::spec::MergeMode;
use uuid::Uuid;

use crate::error::AppError;

/// Mesh editing tools of the toolbar (operating on the current selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshTool {
    Extrude,
    Inset,
    Bevel,
    LoopCut,
    Subdivide,
    MergeCenter,
    MergeByDistance,
    DeleteElements,
    Mirror(Axis),
    RecalculateNormals,
    FlipNormals,
}

impl MeshTool {
    pub fn label(self) -> &'static str {
        match self {
            MeshTool::Extrude => "Extrude",
            MeshTool::Inset => "Inset",
            MeshTool::Bevel => "Bevel",
            MeshTool::LoopCut => "Loop cut",
            MeshTool::Subdivide => "Subdivide",
            MeshTool::MergeCenter => "Merge at center",
            MeshTool::MergeByDistance => "Merge by distance",
            MeshTool::DeleteElements => "Delete elements",
            MeshTool::Mirror(Axis::X) => "Mirror X",
            MeshTool::Mirror(Axis::Y) => "Mirror Y",
            MeshTool::Mirror(Axis::Z) => "Mirror Z",
            MeshTool::RecalculateNormals => "Recalculate normals",
            MeshTool::FlipNormals => "Flip normals",
        }
    }
}

/// Numeric parameters of the mesh tools (edited in the toolbar).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolParams {
    pub extrude_distance: f32,
    pub inset_thickness: f32,
    pub inset_depth: f32,
    pub inset_individual: bool,
    pub bevel_width: f32,
    pub loop_cuts: u32,
    pub loop_factor: f32,
    pub subdivide_levels: u32,
    pub merge_threshold: f32,
    pub mirror_merge: bool,
    pub recalc_inside: bool,
    pub duplicate_offset: [f32; 3],
}

impl Default for ToolParams {
    fn default() -> Self {
        ToolParams {
            extrude_distance: 0.5,
            inset_thickness: 0.1,
            inset_depth: 0.0,
            inset_individual: false,
            bevel_width: 0.1,
            loop_cuts: 1,
            loop_factor: 0.5,
            subdivide_levels: 1,
            merge_threshold: 1e-3,
            mirror_merge: true,
            recalc_inside: false,
            duplicate_offset: [0.5, 0.0, 0.0],
        }
    }
}

fn edit_object(doc: &Document) -> Result<Uuid, AppError> {
    doc.selection
        .active()
        .ok_or_else(|| AppError::Usage("no active object: select an object first".into()))
}

/// Build the command for a mesh tool applied to the active object and its
/// current element selection.
pub fn mesh_tool_spec(
    tool: MeshTool,
    doc: &Document,
    p: &ToolParams,
) -> Result<CommandSpec, AppError> {
    let object = edit_object(doc)?;
    Ok(match tool {
        MeshTool::Extrude => CommandSpec::Extrude {
            object: Some(object),
            faces: None,
            distance: p.extrude_distance,
            offset: None,
        },
        MeshTool::Inset => CommandSpec::Inset {
            object: Some(object),
            faces: None,
            thickness: p.inset_thickness,
            depth: p.inset_depth,
            individual: p.inset_individual,
        },
        MeshTool::Bevel => CommandSpec::Bevel {
            object: Some(object),
            edges: None,
            width: p.bevel_width,
        },
        MeshTool::LoopCut => {
            let edge = doc
                .selection
                .elements(object)
                .and_then(|e| e.edges.iter().next().copied())
                .ok_or_else(|| {
                    AppError::Usage(
                        "loop cut needs a selected edge: switch to edge mode and click an edge"
                            .into(),
                    )
                })?;
            CommandSpec::LoopCut {
                object: Some(object),
                edge,
                cuts: p.loop_cuts.max(1),
                factor: p.loop_factor,
            }
        }
        MeshTool::Subdivide => CommandSpec::Subdivide {
            object: Some(object),
            levels: p.subdivide_levels.max(1),
        },
        MeshTool::MergeCenter => CommandSpec::MergeVertices {
            object: Some(object),
            vertices: None,
            mode: MergeMode::Center,
            threshold: p.merge_threshold,
        },
        MeshTool::MergeByDistance => CommandSpec::MergeVertices {
            object: Some(object),
            vertices: None,
            mode: MergeMode::Distance,
            threshold: p.merge_threshold,
        },
        MeshTool::DeleteElements => {
            let kind = doc
                .selection
                .mode
                .element_kind()
                .unwrap_or(ElementKind::Face);
            CommandSpec::DeleteElements {
                object: Some(object),
                kind: Some(kind),
                indices: None,
            }
        }
        MeshTool::Mirror(axis) => CommandSpec::Mirror {
            object: Some(object),
            axis,
            merge: p.mirror_merge,
            merge_threshold: 1e-4,
        },
        MeshTool::RecalculateNormals => CommandSpec::RecalculateNormals {
            object: Some(object),
            inside: p.recalc_inside,
        },
        MeshTool::FlipNormals => CommandSpec::FlipNormals {
            object: Some(object),
            faces: None,
        },
    })
}

/// Boolean of the active object with another selected object.
pub fn boolean_spec(doc: &Document, operation: BooleanOp) -> Result<CommandSpec, AppError> {
    let active = edit_object(doc)?;
    let other = doc
        .selection
        .objects()
        .iter()
        .rev()
        .copied()
        .find(|&o| o != active)
        .ok_or_else(|| {
            AppError::Usage(
                "boolean needs two selected objects: the active one is modified, the other is consumed"
                    .into(),
            )
        })?;
    Ok(CommandSpec::Boolean {
        object: Some(active),
        other,
        operation,
        keep_other: false,
    })
}

/// Selected objects that are not descendants of other selected objects
/// (transforming both a parent and its child would apply the move twice).
pub fn top_level_selection(doc: &Document) -> Vec<Uuid> {
    let ids: Vec<Uuid> = doc
        .selection
        .objects()
        .iter()
        .copied()
        .filter(|&i| doc.scene.contains(i))
        .collect();
    ids.iter()
        .copied()
        .filter(|&id| !ids.iter().any(|&o| o != id && doc.scene.is_ancestor(o, id)))
        .collect()
}

/// Primitive kinds of the Add menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrimitiveKind {
    #[default]
    Cube,
    Plane,
    UvSphere,
    IcoSphere,
    Cylinder,
    Cone,
    Torus,
}

impl PrimitiveKind {
    pub const ALL: [PrimitiveKind; 7] = [
        PrimitiveKind::Cube,
        PrimitiveKind::Plane,
        PrimitiveKind::UvSphere,
        PrimitiveKind::IcoSphere,
        PrimitiveKind::Cylinder,
        PrimitiveKind::Cone,
        PrimitiveKind::Torus,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PrimitiveKind::Cube => "Cube",
            PrimitiveKind::Plane => "Plane",
            PrimitiveKind::UvSphere => "UV sphere",
            PrimitiveKind::IcoSphere => "Ico sphere",
            PrimitiveKind::Cylinder => "Cylinder",
            PrimitiveKind::Cone => "Cone",
            PrimitiveKind::Torus => "Torus",
        }
    }
}

/// Editable primitive parameters (the "Add with parameters" dialog).
#[derive(Debug, Clone, PartialEq)]
pub struct PrimitiveDraft {
    pub kind: PrimitiveKind,
    pub size: f32,
    pub subdivisions: u32,
    pub radius: f32,
    pub depth: f32,
    pub segments: u32,
    pub rings: u32,
    pub ico_subdivisions: u32,
    pub radius_top: f32,
    pub major_radius: f32,
    pub minor_radius: f32,
    pub major_segments: u32,
    pub minor_segments: u32,
}

impl Default for PrimitiveDraft {
    fn default() -> Self {
        PrimitiveDraft {
            kind: PrimitiveKind::Cube,
            size: 2.0,
            subdivisions: 1,
            radius: 1.0,
            depth: 2.0,
            segments: 32,
            rings: 16,
            ico_subdivisions: 2,
            radius_top: 0.0,
            major_radius: 1.0,
            minor_radius: 0.25,
            major_segments: 48,
            minor_segments: 12,
        }
    }
}

impl PrimitiveDraft {
    /// Draft with defaults for `kind`.
    pub fn of(kind: PrimitiveKind) -> PrimitiveDraft {
        PrimitiveDraft {
            kind,
            ..PrimitiveDraft::default()
        }
    }

    /// The primitive description for the current values.
    pub fn primitive(&self) -> PrimitiveSpec {
        match self.kind {
            PrimitiveKind::Cube => PrimitiveSpec::Cube { size: self.size },
            PrimitiveKind::Plane => PrimitiveSpec::Plane {
                size: self.size,
                subdivisions: self.subdivisions,
            },
            PrimitiveKind::UvSphere => PrimitiveSpec::UvSphere {
                radius: self.radius,
                segments: self.segments,
                rings: self.rings,
            },
            PrimitiveKind::IcoSphere => PrimitiveSpec::IcoSphere {
                radius: self.radius,
                subdivisions: self.ico_subdivisions,
            },
            PrimitiveKind::Cylinder => PrimitiveSpec::Cylinder {
                radius: self.radius,
                depth: self.depth,
                segments: self.segments,
            },
            PrimitiveKind::Cone => PrimitiveSpec::Cone {
                radius_bottom: self.radius,
                radius_top: self.radius_top,
                depth: self.depth,
                segments: self.segments,
            },
            PrimitiveKind::Torus => PrimitiveSpec::Torus {
                major_radius: self.major_radius,
                minor_radius: self.minor_radius,
                major_segments: self.major_segments,
                minor_segments: self.minor_segments,
            },
        }
    }

    /// The `create_primitive` command for the current values.
    pub fn spec(&self) -> CommandSpec {
        create_primitive(self.primitive())
    }
}

/// `create_primitive` with default placement.
pub fn create_primitive(primitive: PrimitiveSpec) -> CommandSpec {
    CommandSpec::CreatePrimitive {
        primitive,
        name: None,
        parent: None,
        translation: None,
        rotation_euler_deg: None,
        scale: None,
        material: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge3d_core::command::Origin;

    fn doc_with_cube() -> Document {
        let mut doc = Document::new();
        doc.execute_spec(PrimitiveDraft::default().spec(), Origin::Gui)
            .unwrap();
        doc
    }

    #[test]
    fn every_primitive_draft_builds_a_mesh() {
        for kind in PrimitiveKind::ALL {
            let mut doc = Document::new();
            doc.execute_spec(PrimitiveDraft::of(kind).spec(), Origin::Gui)
                .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(doc.scene.len(), 1);
        }
    }

    #[test]
    fn mesh_tools_run_on_the_selection() {
        let mut doc = doc_with_cube();
        let cube = doc.selection.active().unwrap();
        let p = ToolParams::default();
        // face mode + select a face, extrude
        doc.execute_spec(
            CommandSpec::SetSelectMode {
                mode: SelectMode::Face,
            },
            Origin::Gui,
        )
        .unwrap();
        doc.execute_spec(
            CommandSpec::SelectElements {
                object: Some(cube),
                kind: Some(ElementKind::Face),
                indices: vec![0],
                action: SelectAction::Set,
            },
            Origin::Gui,
        )
        .unwrap();
        let spec = mesh_tool_spec(MeshTool::Extrude, &doc, &p).unwrap();
        doc.execute_spec(spec, Origin::Gui).unwrap();
        assert_eq!(doc.scene.mesh(cube).unwrap().vertex_count(), 12);
        for tool in [
            MeshTool::Inset,
            MeshTool::Subdivide,
            MeshTool::RecalculateNormals,
            MeshTool::FlipNormals,
            MeshTool::Mirror(Axis::X),
        ] {
            let spec = mesh_tool_spec(tool, &doc, &p).unwrap();
            doc.execute_spec(spec, Origin::Gui)
                .unwrap_or_else(|e| panic!("{tool:?}: {e}"));
        }
    }

    #[test]
    fn loop_cut_needs_an_edge_and_reports_a_usage_error() {
        let doc = doc_with_cube();
        let err = mesh_tool_spec(MeshTool::LoopCut, &doc, &ToolParams::default()).unwrap_err();
        assert_eq!(err.code(), "invalid_action");
        assert!(err.to_string().contains("edge"));
    }

    #[test]
    fn tools_without_an_object_are_usage_errors() {
        let doc = Document::new();
        assert!(mesh_tool_spec(MeshTool::Extrude, &doc, &ToolParams::default()).is_err());
        assert!(boolean_spec(&doc, BooleanOp::Union).is_err());
    }

    #[test]
    fn boolean_uses_active_and_the_other_selected_object() {
        let mut doc = doc_with_cube();
        let a = doc.selection.active().unwrap();
        doc.execute_spec(
            CommandSpec::CreatePrimitive {
                primitive: PrimitiveSpec::Cube { size: 1.0 },
                name: None,
                parent: None,
                translation: Some([0.5, 0.0, 0.0]),
                rotation_euler_deg: None,
                scale: None,
                material: None,
            },
            Origin::Gui,
        )
        .unwrap();
        let b = doc.selection.active().unwrap();
        doc.execute_spec(
            CommandSpec::SelectObjects {
                objects: vec![a, b],
                action: SelectAction::Set,
            },
            Origin::Gui,
        )
        .unwrap();
        match boolean_spec(&doc, BooleanOp::Union).unwrap() {
            CommandSpec::Boolean { object, other, .. } => {
                assert_eq!(object, Some(b));
                assert_eq!(other, a);
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn top_level_selection_skips_selected_descendants() {
        let mut doc = doc_with_cube();
        let parent = doc.selection.active().unwrap();
        doc.execute_spec(
            CommandSpec::CreatePrimitive {
                primitive: PrimitiveSpec::Cube { size: 1.0 },
                name: None,
                parent: Some(parent),
                translation: None,
                rotation_euler_deg: None,
                scale: None,
                material: None,
            },
            Origin::Gui,
        )
        .unwrap();
        let child = doc.selection.active().unwrap();
        doc.execute_spec(
            CommandSpec::SelectObjects {
                objects: vec![parent, child],
                action: SelectAction::Set,
            },
            Origin::Gui,
        )
        .unwrap();
        assert_eq!(top_level_selection(&doc), vec![parent]);
    }
}

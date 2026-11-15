//! # forge3d-core
//!
//! Platform-independent core of the forge3d modeler:
//!
//! * [`mesh`] – half-edge polygon mesh (n-gons, boundaries, validation),
//! * [`primitives`] – parametric shapes,
//! * [`ops`] – mesh editing operations (extrude, inset, bevel, loop cut,
//!   Catmull–Clark, merge, delete, mirror, boolean, normals),
//! * [`scene`] – scene graph with nodes, transforms and PBR materials,
//! * [`selection`] – selection model and queries,
//! * [`command`], [`commands`], [`document`] – undoable command system with
//!   history, origins (GUI / CLI / AI) and atomic batches,
//! * [`spec`] – [`spec::CommandSpec`], the serializable command language
//!   shared by GUI, CLI and the MCP/AI layer (with JSON Schema),
//! * [`error`] – [`error::CoreError`] with stable codes and hints.
//!
//! Every change to a [`document::Document`] goes through a command:
//!
//! ```
//! use forge3d_core::prelude::*;
//!
//! let mut doc = Document::new();
//! doc.execute_json(r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#, Origin::Ai)
//!     .unwrap();
//! doc.execute_json(r#"{"op":"extrude","faces":[1],"distance":0.5}"#, Origin::Ai)
//!     .unwrap();
//! let cube = doc.selection.active().unwrap();
//! assert_eq!(doc.scene.mesh(cube).unwrap().vertex_count(), 12);
//! assert_eq!(doc.history_list()[1].label, "AI: Extrude 1 face on 'Cube'");
//! doc.undo().unwrap();
//! assert_eq!(doc.scene.mesh(cube).unwrap().vertex_count(), 8);
//! ```

#![forbid(unsafe_code)]

pub mod command;
pub mod commands;
pub mod document;
pub mod error;
pub mod math;
pub mod mesh;
pub mod ops;
pub mod primitives;
pub mod scene;
pub mod selection;
pub mod spec;

/// Commonly used types.
pub mod prelude {
    pub use crate::command::{Batch, ChangeSummary, Command, ObjectSummary, Origin};
    pub use crate::document::{Document, DocumentState, HistoryEntryInfo};
    pub use crate::error::{CoreError, CoreResult, ErrorReport};
    pub use crate::math::{Aabb, Axis, Space};
    pub use crate::mesh::{
        EdgeId, ElementKind, FaceId, LoopId, Mesh, MeshData, ValidationReport, VertexId,
    };
    pub use crate::ops::BooleanOp;
    pub use crate::primitives::PrimitiveSpec;
    pub use crate::scene::{Material, Node, Scene};
    pub use crate::selection::{ElementSelection, SelectAction, SelectMode, Selection};
    pub use crate::spec::{CommandSpec, MergeMode};
}

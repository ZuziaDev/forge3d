//! Error type shared by every layer of forge3d.
//!
//! Each variant maps to a stable, machine-readable [`CoreError::code`] and a
//! corrective [`CoreError::hint`], so that API/MCP layers can surface
//! `{code, message, hint}` and an AI agent can self-correct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mesh::ElementKind;

/// Result alias used throughout the crate.
pub type CoreResult<T> = Result<T, CoreError>;

/// All errors produced by `forge3d-core`.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", content = "details", rename_all = "snake_case")]
pub enum CoreError {
    /// A node id does not exist in the scene.
    #[error("node {0} not found")]
    NodeNotFound(Uuid),
    /// A node exists but carries no mesh while the operation requires one.
    #[error("node {0} has no mesh")]
    NoMesh(Uuid),
    /// The selection is not usable for the requested operation.
    #[error("invalid selection: {0}")]
    InvalidSelection(String),
    /// The operation needs selected objects or elements but nothing is selected.
    #[error("nothing selected: {0}")]
    EmptySelection(String),
    /// An element index is outside the mesh.
    #[error("{kind:?} index {index} out of range (mesh has {count})")]
    ElementOutOfRange {
        kind: ElementKind,
        index: u32,
        count: u32,
    },
    /// The operation requires manifold topology around the affected elements.
    #[error("non-manifold geometry: {0}")]
    NonManifold(String),
    /// A parameter value is out of its valid range.
    #[error("invalid parameter `{name}`: {reason}")]
    InvalidParameter { name: String, reason: String },
    /// Mesh input data (indices, polygons) is malformed.
    #[error("invalid mesh data: {0}")]
    InvalidMesh(String),
    /// Reparenting would create a cycle in the scene hierarchy.
    #[error("reparenting {child} under {parent} would create a cycle")]
    HierarchyCycle { child: Uuid, parent: Uuid },
    /// A node with this id already exists.
    #[error("node {0} already exists")]
    DuplicateNode(Uuid),
    /// Undo requested with an empty undo stack.
    #[error("nothing to undo")]
    NothingToUndo,
    /// Redo requested with an empty redo stack.
    #[error("nothing to redo")]
    NothingToRedo,
    /// The boolean operation could not produce a result.
    #[error("boolean operation failed: {0}")]
    BooleanFailed(String),
    /// A command description (JSON) could not be parsed.
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    /// A command was used in a state it does not support (e.g. undo before apply).
    #[error("command state error: {0}")]
    CommandState(String),
    /// A step of a batch failed; everything was rolled back.
    #[error("batch step {index} failed and the batch was rolled back: {source}")]
    BatchFailed {
        index: usize,
        #[source]
        source: Box<CoreError>,
    },
}

/// Serializable error report (`{code, message, hint}`) for API layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorReport {
    pub code: String,
    pub message: String,
    pub hint: String,
}

impl CoreError {
    /// Convenience constructor for [`CoreError::InvalidParameter`].
    pub fn param(name: impl Into<String>, reason: impl Into<String>) -> Self {
        CoreError::InvalidParameter {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::NodeNotFound(_) => "node_not_found",
            CoreError::NoMesh(_) => "node_has_no_mesh",
            CoreError::InvalidSelection(_) => "invalid_selection",
            CoreError::EmptySelection(_) => "empty_selection",
            CoreError::ElementOutOfRange { .. } => "element_out_of_range",
            CoreError::NonManifold(_) => "non_manifold",
            CoreError::InvalidParameter { .. } => "invalid_parameter",
            CoreError::InvalidMesh(_) => "invalid_mesh",
            CoreError::HierarchyCycle { .. } => "hierarchy_cycle",
            CoreError::DuplicateNode(_) => "duplicate_node",
            CoreError::NothingToUndo => "nothing_to_undo",
            CoreError::NothingToRedo => "nothing_to_redo",
            CoreError::BooleanFailed(_) => "boolean_failed",
            CoreError::InvalidCommand(_) => "invalid_command",
            CoreError::CommandState(_) => "command_state",
            CoreError::BatchFailed { .. } => "batch_failed",
        }
    }

    /// A corrective suggestion for the caller.
    pub fn hint(&self) -> String {
        match self {
            CoreError::NodeNotFound(_) => {
                "List the scene objects to obtain valid node ids; the node may have been deleted or the id mistyped.".into()
            }
            CoreError::NoMesh(_) => {
                "Target an object that has mesh data (e.g. one created with create_primitive), not an empty/group node.".into()
            }
            CoreError::InvalidSelection(_) => {
                "Check the selection mode and the selected element kinds; select elements of the kind this operation expects.".into()
            }
            CoreError::EmptySelection(_) => {
                "Select the objects or mesh elements first (e.g. select_elements / select_by_direction) or pass them explicitly.".into()
            }
            CoreError::ElementOutOfRange { count, .. } => format!(
                "Element indices must be in 0..{count}; query the mesh again because indices change after topology edits."
            ),
            CoreError::NonManifold(_) => {
                "Run recalculate_normals or merge_vertices (by distance) to repair the mesh, or choose elements on manifold geometry.".into()
            }
            CoreError::InvalidParameter { name, .. } => {
                format!("Adjust `{name}` to a value within its documented range.")
            }
            CoreError::InvalidMesh(_) => {
                "Polygons need at least 3 distinct, in-range vertex indices.".into()
            }
            CoreError::HierarchyCycle { .. } => {
                "Choose a parent that is not the node itself or one of its descendants.".into()
            }
            CoreError::DuplicateNode(_) => "Use a fresh node id.".into(),
            CoreError::NothingToUndo => "The undo history is empty; nothing was changed.".into(),
            CoreError::NothingToRedo => {
                "There is no undone operation to redo; executing a new command clears the redo stack.".into()
            }
            CoreError::BooleanFailed(_) => {
                "Make sure both operands are closed meshes that actually overlap; try slightly offsetting coplanar faces.".into()
            }
            CoreError::InvalidCommand(_) => {
                "Validate the command JSON against the published JSON Schema for CommandSpec.".into()
            }
            CoreError::CommandState(_) => "This is an internal sequencing error; re-run the operation.".into(),
            CoreError::BatchFailed { source, .. } => source.hint(),
        }
    }

    /// Build a serializable `{code, message, hint}` report.
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code().to_string(),
            message: self.to_string(),
            hint: self.hint(),
        }
    }
}

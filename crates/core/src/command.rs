//! Command trait, change summaries, origins and batches.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::document::Document;
use crate::error::{CoreError, CoreResult};
use crate::scene::Scene;

/// An undoable operation on a [`Document`].
///
/// Contract:
/// * `apply` either succeeds or leaves the document unchanged (validate
///   before mutating);
/// * after a successful `apply`, `undo` restores the exact prior state;
/// * after `undo`, `apply` may be called again (redo) and must reproduce
///   the same result (commands cache resolved targets / generated ids).
pub trait Command: Send {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary>;
    fn undo(&mut self, doc: &mut Document) -> CoreResult<()>;
    /// Human readable description, e.g. "Extrude 1 face on Cube".
    fn describe(&self) -> String;
}

/// Who issued a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Gui,
    Cli,
    Ai,
}

impl Origin {
    /// History label for a description: `"AI: …"`, `"CLI: …"`, or the plain
    /// description for GUI operations.
    pub fn label(self, description: &str) -> String {
        match self {
            Origin::Gui => description.to_string(),
            Origin::Cli => format!("CLI: {description}"),
            Origin::Ai => format!("AI: {description}"),
        }
    }
}

/// Per-object summary included in [`ChangeSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectSummary {
    pub id: Uuid,
    pub name: String,
    pub has_mesh: bool,
    pub vertex_count: usize,
    pub edge_count: usize,
    pub face_count: usize,
}

impl ObjectSummary {
    /// Summarize a node currently in the scene.
    pub fn of(scene: &Scene, id: Uuid) -> CoreResult<ObjectSummary> {
        let n = scene.node(id)?;
        let m = n.mesh.as_ref();
        Ok(ObjectSummary {
            id,
            name: n.name.clone(),
            has_mesh: m.is_some(),
            vertex_count: m.map_or(0, |m| m.vertex_count()),
            edge_count: m.map_or(0, |m| m.edge_count()),
            face_count: m.map_or(0, |m| m.face_count()),
        })
    }
}

/// What a command changed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSummary {
    pub created: Vec<ObjectSummary>,
    pub modified: Vec<ObjectSummary>,
    pub deleted: Vec<ObjectSummary>,
    /// Whether the selection (mode, objects or elements) was changed.
    pub selection_changed: bool,
}

impl ChangeSummary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn created(mut self, scene: &Scene, id: Uuid) -> CoreResult<Self> {
        self.created.push(ObjectSummary::of(scene, id)?);
        Ok(self)
    }

    pub fn modified(mut self, scene: &Scene, id: Uuid) -> CoreResult<Self> {
        self.modified.push(ObjectSummary::of(scene, id)?);
        Ok(self)
    }

    /// Record a deletion (pass the summary taken before removal).
    pub fn deleted(mut self, summary: ObjectSummary) -> Self {
        self.deleted.push(summary);
        self
    }

    pub fn with_selection_changed(mut self) -> Self {
        self.selection_changed = true;
        self
    }

    /// Ids of all created/modified/deleted objects.
    pub fn touched_ids(&self) -> Vec<Uuid> {
        self.created
            .iter()
            .chain(&self.modified)
            .chain(&self.deleted)
            .map(|o| o.id)
            .collect()
    }

    /// Merge a later change into this one (net effect).
    pub fn merge(&mut self, later: ChangeSummary) {
        for c in later.created {
            if let Some(i) = self.deleted.iter().position(|d| d.id == c.id) {
                self.deleted.remove(i);
                upsert(&mut self.modified, c);
            } else {
                upsert(&mut self.created, c);
            }
        }
        for m in later.modified {
            if let Some(slot) = self.created.iter_mut().find(|x| x.id == m.id) {
                *slot = m;
            } else {
                upsert(&mut self.modified, m);
            }
        }
        for d in later.deleted {
            if let Some(i) = self.created.iter().position(|x| x.id == d.id) {
                self.created.remove(i);
                continue;
            }
            self.modified.retain(|x| x.id != d.id);
            upsert(&mut self.deleted, d);
        }
        self.selection_changed |= later.selection_changed;
    }
}

fn upsert(list: &mut Vec<ObjectSummary>, s: ObjectSummary) {
    match list.iter_mut().find(|x| x.id == s.id) {
        Some(slot) => *slot = s,
        None => list.push(s),
    }
}

/// Several commands applied atomically as a single undo step. If any step
/// fails, the already applied steps are undone in reverse order and a
/// `batch_failed` error carrying the failing step's error is returned.
pub struct Batch {
    description: String,
    commands: Vec<Box<dyn Command>>,
}

impl Batch {
    pub fn new(description: impl Into<String>, commands: Vec<Box<dyn Command>>) -> Batch {
        Batch {
            description: description.into(),
            commands,
        }
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Command for Batch {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let mut summary = ChangeSummary::new();
        let selection_before = doc.selection.clone();
        for i in 0..self.commands.len() {
            match self.commands[i].apply(doc) {
                Ok(s) => {
                    doc.selection.prune(&doc.scene);
                    summary.merge(s);
                }
                Err(e) => {
                    for j in (0..i).rev() {
                        // Undo of an applied command restores its prior state;
                        // a failure here would indicate a broken command.
                        if let Err(u) = self.commands[j].undo(doc) {
                            return Err(CoreError::CommandState(format!(
                                "rollback of batch step {j} failed: {u}"
                            )));
                        }
                    }
                    doc.selection = selection_before;
                    return Err(CoreError::BatchFailed {
                        index: i,
                        source: Box::new(e),
                    });
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
        if !self.description.is_empty() {
            return self.description.clone();
        }
        let parts: Vec<String> = self.commands.iter().map(|c| c.describe()).collect();
        format!("Batch: {}", parts.join("; "))
    }
}

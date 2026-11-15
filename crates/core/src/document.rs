//! The document: scene + selection + undo history.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::command::{ChangeSummary, Command, Origin};
use crate::error::{CoreError, CoreResult};
use crate::scene::Scene;
use crate::selection::Selection;
use crate::spec::CommandSpec;

struct HistoryEntry {
    seq: u64,
    origin: Origin,
    label: String,
    description: String,
    command: Option<Box<dyn Command>>,
    selection_before: Selection,
    selection_after: Selection,
    summary: ChangeSummary,
}

/// Public view of a history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntryInfo {
    /// Monotonic sequence number (unique within the document's lifetime).
    pub seq: u64,
    /// Display label, e.g. `"AI: Extrude 1 face on Cube"`.
    pub label: String,
    /// The command's own description.
    pub description: String,
    pub origin: Origin,
    /// True when the entry has been undone (it is on the redo stack).
    pub undone: bool,
    /// What the command changed when it was (last) applied.
    pub summary: ChangeSummary,
}

/// Serializable snapshot of the document content (without history).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DocumentState {
    pub scene: Scene,
    pub selection: Selection,
}

/// Scene, selection and linear undo/redo history. Every change should go
/// through [`Document::execute`] (or [`Document::execute_spec`]).
pub struct Document {
    pub scene: Scene,
    pub selection: Selection,
    entries: Vec<HistoryEntry>,
    /// Number of applied (not undone) entries.
    cursor: usize,
    next_seq: u64,
    limit: Option<usize>,
    revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("scene", &self.scene)
            .field("selection", &self.selection)
            .field("history_len", &self.entries.len())
            .field("cursor", &self.cursor)
            .field("revision", &self.revision)
            .finish()
    }
}

impl Document {
    pub fn new() -> Document {
        Document {
            scene: Scene::new(),
            selection: Selection::new(),
            entries: Vec::new(),
            cursor: 0,
            next_seq: 1,
            limit: None,
            revision: 0,
        }
    }

    /// Document with existing content and empty history.
    pub fn from_state(state: DocumentState) -> Document {
        let mut d = Document::new();
        d.scene = state.scene;
        d.selection = state.selection;
        d.selection.prune(&d.scene);
        d
    }

    /// Snapshot of scene + selection.
    pub fn state(&self) -> DocumentState {
        DocumentState {
            scene: self.scene.clone(),
            selection: self.selection.clone(),
        }
    }

    /// Counter incremented on every execute/undo/redo (for change detection).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Limit the number of history entries kept (`None` = unlimited).
    pub fn set_history_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
        self.enforce_limit();
    }

    fn enforce_limit(&mut self) {
        if let Some(max) = self.limit {
            while self.entries.len() > max && self.cursor > 0 {
                self.entries.remove(0);
                self.cursor -= 1;
            }
        }
    }

    /// Execute a command and record it in the history.
    pub fn execute(&mut self, cmd: Box<dyn Command>, origin: Origin) -> CoreResult<ChangeSummary> {
        self.execute_labeled(cmd, origin, None)
    }

    /// Execute with an explicit label (the origin prefix is still applied).
    pub fn execute_labeled(
        &mut self,
        mut cmd: Box<dyn Command>,
        origin: Origin,
        label: Option<&str>,
    ) -> CoreResult<ChangeSummary> {
        let selection_before = self.selection.clone();
        match cmd.apply(self) {
            Ok(mut summary) => {
                self.selection.prune(&self.scene);
                if self.selection != selection_before {
                    summary.selection_changed = true;
                }
                let description = cmd.describe();
                let label = origin.label(label.unwrap_or(&description));
                self.entries.truncate(self.cursor);
                self.entries.push(HistoryEntry {
                    seq: self.next_seq,
                    origin,
                    label,
                    description,
                    command: Some(cmd),
                    selection_before,
                    selection_after: self.selection.clone(),
                    summary: summary.clone(),
                });
                self.next_seq += 1;
                self.cursor = self.entries.len();
                self.revision += 1;
                self.enforce_limit();
                Ok(summary)
            }
            Err(e) => {
                self.selection = selection_before;
                Err(e)
            }
        }
    }

    /// Build a command from its data description and execute it.
    pub fn execute_spec(&mut self, spec: CommandSpec, origin: Origin) -> CoreResult<ChangeSummary> {
        let cmd = spec.into_command()?;
        self.execute(cmd, origin)
    }

    /// Execute a JSON command description (see [`CommandSpec`]).
    pub fn execute_json(&mut self, json: &str, origin: Origin) -> CoreResult<ChangeSummary> {
        self.execute_spec(CommandSpec::from_json(json)?, origin)
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    /// Undo the last applied entry.
    pub fn undo(&mut self) -> CoreResult<HistoryEntryInfo> {
        if self.cursor == 0 {
            return Err(CoreError::NothingToUndo);
        }
        let i = self.cursor - 1;
        let mut cmd = self.entries[i]
            .command
            .take()
            .ok_or_else(|| CoreError::CommandState("history entry is busy".into()))?;
        let res = cmd.undo(self);
        self.entries[i].command = Some(cmd);
        res?;
        self.selection = self.entries[i].selection_before.clone();
        self.cursor -= 1;
        self.revision += 1;
        Ok(self.info(i))
    }

    /// Re-apply the next undone entry.
    pub fn redo(&mut self) -> CoreResult<HistoryEntryInfo> {
        if self.cursor >= self.entries.len() {
            return Err(CoreError::NothingToRedo);
        }
        let i = self.cursor;
        let mut cmd = self.entries[i]
            .command
            .take()
            .ok_or_else(|| CoreError::CommandState("history entry is busy".into()))?;
        let before = self.selection.clone();
        let res = cmd.apply(self);
        self.entries[i].command = Some(cmd);
        match res {
            Ok(summary) => {
                self.selection = self.entries[i].selection_after.clone();
                self.selection.prune(&self.scene);
                self.entries[i].summary = summary;
                self.cursor += 1;
                self.revision += 1;
                Ok(self.info(i))
            }
            Err(e) => {
                self.selection = before;
                Err(e)
            }
        }
    }

    fn info(&self, i: usize) -> HistoryEntryInfo {
        let e = &self.entries[i];
        HistoryEntryInfo {
            seq: e.seq,
            label: e.label.clone(),
            description: e.description.clone(),
            origin: e.origin,
            undone: i >= self.cursor,
            summary: e.summary.clone(),
        }
    }

    /// All history entries, oldest first (undone entries flagged).
    pub fn history_list(&self) -> Vec<HistoryEntryInfo> {
        (0..self.entries.len()).map(|i| self.info(i)).collect()
    }

    /// Drop all history (content is kept).
    pub fn clear_history(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }
}

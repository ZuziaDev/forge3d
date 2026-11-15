//! Imported sub-scenes and their integration into a document.

use std::collections::HashMap;
use std::path::Path;

use forge3d_core::command::{Batch, ChangeSummary, Command, Origin};
use forge3d_core::commands::AddNode;
use forge3d_core::document::Document;
use forge3d_core::error::CoreResult;
use forge3d_core::scene::{Node, Scene};
use uuid::Uuid;

use crate::error::IoResult;
use crate::util::PlannedNode;
use crate::{import_file, ImportOptions};

/// The result of parsing a file: a detached sub-scene with fresh node ids.
///
/// Nothing has been added to any document yet. Use [`import_command`] to
/// obtain an undoable command, or [`import_into`] to merge it directly.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The imported nodes (hierarchy, transforms, meshes, materials).
    pub scene: Scene,
    /// Label of the source, usually the file stem.
    pub name: String,
    /// Non-fatal problems (skipped primitives, missing material libraries, ...).
    pub warnings: Vec<String>,
}

impl Imported {
    pub(crate) fn new(scene: Scene, name: String, warnings: Vec<String>) -> Imported {
        Imported {
            scene,
            name,
            warnings,
        }
    }

    /// Copy `scene` assigning fresh node ids, so that importing the same
    /// project twice (or into the scene it came from) never collides.
    pub(crate) fn from_scene(scene: &Scene, name: String, warnings: Vec<String>) -> Imported {
        let mut out = Scene::new();
        let mut ids: HashMap<Uuid, Uuid> = HashMap::new();
        for id in scene.traverse() {
            let src = scene.get(id).expect("id comes from traverse()");
            let mut n = Node::new(src.name.clone());
            n.transform = src.transform;
            n.mesh = src.mesh.clone();
            n.material = src.material.clone();
            n.visible = src.visible;
            let parent = src.parent().and_then(|p| ids.get(&p).copied());
            let new_id = out
                .add_node(n, parent)
                .expect("fresh ids and parents inserted in pre-order cannot clash");
            ids.insert(id, new_id);
        }
        Imported::new(out, name, warnings)
    }

    /// Ids of the top-level imported nodes.
    pub fn roots(&self) -> &[Uuid] {
        self.scene.roots()
    }

    /// Number of imported nodes.
    pub fn node_count(&self) -> usize {
        self.scene.len()
    }

    /// Number of imported nodes that carry a mesh.
    pub fn mesh_count(&self) -> usize {
        self.scene.nodes().filter(|n| n.mesh.is_some()).count()
    }
}

/// Build a scene from export-planned nodes, preserving their ids.
pub(crate) fn sub_scene(scene: &Scene, plan: &[PlannedNode]) -> IoResult<Scene> {
    let mut out = Scene::new();
    for p in plan {
        let src = scene.node(p.id)?;
        let mut n = Node::with_id(p.id, src.name.clone());
        n.transform = p.transform;
        n.mesh = src.mesh.clone();
        n.material = src.material.clone();
        n.visible = src.visible;
        out.add_node(n, p.parent)?;
    }
    Ok(out)
}

/// Merge an imported sub-scene into `scene` under `parent` (or as roots)
/// without going through the command system. Names are made unique. Returns
/// the ids of the new top-level nodes.
pub fn import_into(
    scene: &mut Scene,
    imported: Imported,
    parent: Option<Uuid>,
) -> IoResult<Vec<Uuid>> {
    if let Some(p) = parent {
        scene.node(p)?;
    }
    let mut roots = Vec::new();
    for id in imported.scene.traverse() {
        let mut node = imported.scene.node(id)?.clone();
        let target_parent = match node.parent() {
            Some(p) => Some(p),
            None => parent,
        };
        node.name = scene.unique_name(&node.name);
        scene.add_node(node, target_parent)?;
        if imported.scene.node(id)?.parent().is_none() {
            roots.push(id);
        }
    }
    Ok(roots)
}

/// Adds a whole imported sub-scene as one undo step and selects its roots.
struct ImportCommand {
    batch: Batch,
    roots: Vec<Uuid>,
}

impl Command for ImportCommand {
    fn apply(&mut self, doc: &mut Document) -> CoreResult<ChangeSummary> {
        let summary = self.batch.apply(doc)?;
        doc.selection.set_objects(self.roots.iter().copied());
        Ok(summary.with_selection_changed())
    }

    fn undo(&mut self, doc: &mut Document) -> CoreResult<()> {
        self.batch.undo(doc)
    }

    fn describe(&self) -> String {
        self.batch.describe()
    }
}

/// Turn an imported sub-scene into a single undoable command: it adds every
/// node (parents before children), selects the imported top-level nodes, and
/// is reverted by one `undo`. If `parent` does not exist, applying fails and
/// nothing is changed.
pub fn import_command(imported: Imported, parent: Option<Uuid>) -> Box<dyn Command> {
    let mut commands: Vec<Box<dyn Command>> = Vec::with_capacity(imported.scene.len());
    let mut roots = Vec::new();
    for id in imported.scene.traverse() {
        let node = imported
            .scene
            .get(id)
            .expect("id comes from traverse()")
            .clone();
        let p = match node.parent() {
            Some(p) => Some(p),
            None => {
                roots.push(id);
                parent
            }
        };
        commands.push(Box::new(AddNode::new(node, p, false, "imported object")));
    }
    let count = commands.len();
    let description = format!(
        "Import '{}' ({} object{})",
        imported.name,
        count,
        if count == 1 { "" } else { "s" }
    );
    Box::new(ImportCommand {
        batch: Batch::new(description, commands),
        roots,
    })
}

/// Parse a file and return the undoable import command together with the
/// parsed content's warnings.
pub fn import_file_command(
    path: impl AsRef<Path>,
    opts: &ImportOptions,
    parent: Option<Uuid>,
) -> IoResult<(Box<dyn Command>, Vec<String>)> {
    let imported = import_file(path, opts)?;
    let warnings = imported.warnings.clone();
    Ok((import_command(imported, parent), warnings))
}

/// Parse a file and execute the import on `doc` as one history entry.
pub fn import_file_into_document(
    doc: &mut Document,
    path: impl AsRef<Path>,
    opts: &ImportOptions,
    parent: Option<Uuid>,
    origin: Origin,
) -> IoResult<ChangeSummary> {
    let (cmd, _) = import_file_command(path, opts, parent)?;
    Ok(doc.execute(cmd, origin)?)
}

//! Scene graph: nodes with transforms, optional mesh and material, organized
//! in a hierarchy.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use glam::Affine3A;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::math::Aabb;
use crate::mesh::Mesh;
use crate::modifier::{self, Modifier, ModifierWarning};

fn d_base_color() -> [f32; 4] {
    [0.8, 0.8, 0.8, 1.0]
}
fn d_roughness() -> f32 {
    0.5
}
fn d_material_name() -> String {
    "Material".into()
}

/// Simple PBR metallic-roughness material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Material {
    #[serde(default = "d_material_name")]
    pub name: String,
    /// Linear RGBA base color, components in 0..=1.
    #[serde(default = "d_base_color")]
    pub base_color: [f32; 4],
    /// 0 = dielectric, 1 = metal.
    #[serde(default)]
    pub metallic: f32,
    /// 0 = mirror-smooth, 1 = fully rough.
    #[serde(default = "d_roughness")]
    pub roughness: f32,
    /// Linear RGB emission.
    #[serde(default)]
    pub emissive: [f32; 3],
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: d_material_name(),
            base_color: d_base_color(),
            metallic: 0.0,
            roughness: d_roughness(),
            emissive: [0.0; 3],
        }
    }
}

impl Material {
    /// Check that all factors are finite and in range.
    pub fn validate(&self) -> CoreResult<()> {
        let in01 = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if !self.base_color.iter().all(|&c| in01(c)) {
            return Err(CoreError::param(
                "base_color",
                "components must be in 0..=1",
            ));
        }
        if !in01(self.metallic) {
            return Err(CoreError::param("metallic", "must be in 0..=1"));
        }
        if !in01(self.roughness) {
            return Err(CoreError::param("roughness", "must be in 0..=1"));
        }
        if !self.emissive.iter().all(|&c| c.is_finite() && c >= 0.0) {
            return Err(CoreError::param(
                "emissive",
                "components must be finite and >= 0",
            ));
        }
        Ok(())
    }
}

/// A scene node. Hierarchy links (`parent`, `children`) are maintained by
/// [`Scene`]; use its methods to change them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub(crate) id: Uuid,
    pub name: String,
    /// Local transform relative to the parent.
    pub transform: Affine3A,
    pub mesh: Option<Mesh>,
    pub material: Option<Material>,
    #[serde(default = "d_true")]
    pub visible: bool,
    pub(crate) parent: Option<Uuid>,
    pub(crate) children: Vec<Uuid>,
    /// Non-destructive modifier stack, evaluated in order on top of `mesh`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<Modifier>,
    /// Smooth (interpolated) shading instead of flat faces.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shade_smooth: bool,
    /// With smooth shading: edges whose faces meet at more than this angle
    /// stay sharp (`None` = everything smooth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_smooth_angle_deg: Option<f32>,
}

fn d_true() -> bool {
    true
}

impl Node {
    /// New node with a fresh random id and identity transform.
    pub fn new(name: impl Into<String>) -> Node {
        Node::with_id(Uuid::new_v4(), name)
    }

    /// New node with a given id.
    pub fn with_id(id: Uuid, name: impl Into<String>) -> Node {
        Node {
            id,
            name: name.into(),
            transform: Affine3A::IDENTITY,
            mesh: None,
            material: None,
            visible: true,
            parent: None,
            children: Vec::new(),
            modifiers: Vec::new(),
            shade_smooth: false,
            auto_smooth_angle_deg: None,
        }
    }

    /// Builder: attach a mesh.
    pub fn with_mesh(mut self, mesh: Mesh) -> Node {
        self.mesh = Some(mesh);
        self
    }

    /// Builder: set the local transform.
    pub fn with_transform(mut self, t: Affine3A) -> Node {
        self.transform = t;
        self
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn parent(&self) -> Option<Uuid> {
        self.parent
    }
    pub fn children(&self) -> &[Uuid] {
        &self.children
    }

    /// The mesh with all enabled modifiers applied (borrowed when there is
    /// nothing to apply). `None` for nodes without a mesh.
    pub fn evaluated_mesh(&self) -> Option<Cow<'_, Mesh>> {
        self.evaluated_mesh_with_warnings().map(|(m, _)| m)
    }

    /// Like [`Node::evaluated_mesh`], also reporting modifiers that failed
    /// (and were skipped).
    pub fn evaluated_mesh_with_warnings(&self) -> Option<(Cow<'_, Mesh>, Vec<ModifierWarning>)> {
        let mesh = self.mesh.as_ref()?;
        Some(modifier::evaluate(mesh, &self.modifiers))
    }

    /// Whether any enabled modifier changes the base mesh.
    pub fn has_active_modifiers(&self) -> bool {
        self.modifiers.iter().any(|m| m.enabled)
    }

    /// Crease angle for shading normals: 0 for flat shading, the auto-smooth
    /// angle (or 180 = fully smooth) for smooth shading.
    pub fn shading_crease_deg(&self) -> f32 {
        if self.shade_smooth {
            self.auto_smooth_angle_deg.unwrap_or(180.0)
        } else {
            0.0
        }
    }
}

/// A removed node with its whole subtree, restorable with [`Scene::restore_subtree`].
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedSubtree {
    /// Root first, then descendants (pre-order).
    pub nodes: Vec<Node>,
    pub parent: Option<Uuid>,
    /// Position among the parent's children (or among the roots).
    pub index: usize,
}

/// Serialized form of a [`Scene`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SceneData {
    roots: Vec<Uuid>,
    nodes: Vec<Node>,
}

/// The scene: a forest of nodes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(into = "SceneData", try_from = "SceneData")]
pub struct Scene {
    nodes: HashMap<Uuid, Node>,
    roots: Vec<Uuid>,
}

impl From<Scene> for SceneData {
    fn from(s: Scene) -> Self {
        let order = s.traverse();
        let mut nodes = s.nodes;
        SceneData {
            roots: s.roots,
            nodes: order
                .into_iter()
                .filter_map(|id| nodes.remove(&id))
                .collect(),
        }
    }
}

impl TryFrom<SceneData> for Scene {
    type Error = CoreError;
    fn try_from(d: SceneData) -> CoreResult<Scene> {
        let mut nodes = HashMap::with_capacity(d.nodes.len());
        for n in d.nodes {
            let id = n.id;
            if nodes.insert(id, n).is_some() {
                return Err(CoreError::DuplicateNode(id));
            }
        }
        let scene = Scene {
            nodes,
            roots: d.roots,
        };
        scene.check_hierarchy()?;
        Ok(scene)
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn contains(&self, id: Uuid) -> bool {
        self.nodes.contains_key(&id)
    }
    /// Top-level nodes in order.
    pub fn roots(&self) -> &[Uuid] {
        &self.roots
    }
    pub fn get(&self, id: Uuid) -> Option<&Node> {
        self.nodes.get(&id)
    }
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }
    /// Lookup returning a `node_not_found` error.
    pub fn node(&self, id: Uuid) -> CoreResult<&Node> {
        self.nodes.get(&id).ok_or(CoreError::NodeNotFound(id))
    }
    /// Mutable lookup returning a `node_not_found` error.
    pub fn node_mut(&mut self, id: Uuid) -> CoreResult<&mut Node> {
        self.nodes.get_mut(&id).ok_or(CoreError::NodeNotFound(id))
    }
    /// The mesh of a node (`node_has_no_mesh` error when absent).
    pub fn mesh(&self, id: Uuid) -> CoreResult<&Mesh> {
        self.node(id)?.mesh.as_ref().ok_or(CoreError::NoMesh(id))
    }
    /// Evaluated mesh (modifiers applied) of a node.
    pub fn evaluated_mesh(&self, id: Uuid) -> CoreResult<Cow<'_, Mesh>> {
        self.node(id)?.evaluated_mesh().ok_or(CoreError::NoMesh(id))
    }
    /// Mutable mesh of a node.
    pub fn mesh_mut(&mut self, id: Uuid) -> CoreResult<&mut Mesh> {
        self.node_mut(id)?
            .mesh
            .as_mut()
            .ok_or(CoreError::NoMesh(id))
    }
    /// Iterate all nodes (arbitrary order).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }
    /// Node ids in depth-first pre-order following root/children order.
    pub fn traverse(&self) -> Vec<Uuid> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<Uuid> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if let Some(n) = self.nodes.get(&id) {
                out.push(id);
                stack.extend(n.children.iter().rev().copied());
            }
        }
        out
    }
    /// First node (in traversal order) with the given name.
    pub fn find_by_name(&self, name: &str) -> Option<Uuid> {
        self.traverse()
            .into_iter()
            .find(|id| self.nodes[id].name == name)
    }
    /// All descendants of a node (pre-order, excluding the node).
    pub fn descendants(&self, id: Uuid) -> Vec<Uuid> {
        let mut out = Vec::new();
        let mut stack: Vec<Uuid> = match self.nodes.get(&id) {
            Some(n) => n.children.iter().rev().copied().collect(),
            None => return out,
        };
        while let Some(c) = stack.pop() {
            if let Some(n) = self.nodes.get(&c) {
                out.push(c);
                stack.extend(n.children.iter().rev().copied());
            }
        }
        out
    }
    /// Whether `ancestor` is a (strict) ancestor of `id`.
    pub fn is_ancestor(&self, ancestor: Uuid, id: Uuid) -> bool {
        let mut cur = self.nodes.get(&id).and_then(|n| n.parent);
        let mut guard = 0;
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.nodes.get(&p).and_then(|n| n.parent);
            guard += 1;
            if guard > self.nodes.len() {
                break;
            }
        }
        false
    }

    /// World transform (product of local transforms from the root).
    pub fn world_transform(&self, id: Uuid) -> CoreResult<Affine3A> {
        let mut n = self.node(id)?;
        let mut m = n.transform;
        let mut guard = 0;
        while let Some(p) = n.parent {
            n = self.node(p)?;
            m = n.transform * m;
            guard += 1;
            if guard > self.nodes.len() {
                return Err(CoreError::HierarchyCycle {
                    child: id,
                    parent: p,
                });
            }
        }
        Ok(m)
    }

    /// World-space bounding box of a node's own mesh.
    pub fn world_bounding_box(&self, id: Uuid) -> CoreResult<Option<Aabb>> {
        let w = self.world_transform(id)?;
        Ok(self
            .node(id)?
            .mesh
            .as_ref()
            .and_then(|m| m.bounding_box())
            .map(|b| b.transformed(&w)))
    }

    /// A name not used by any node: `base`, else `base.001`, `base.002`, …
    /// (an existing numeric suffix on `base` is stripped first).
    pub fn unique_name(&self, base: &str) -> String {
        let taken: HashSet<&str> = self.nodes.values().map(|n| n.name.as_str()).collect();
        if !taken.contains(base) {
            return base.to_string();
        }
        let stem = match base.rsplit_once('.') {
            Some((s, suffix))
                if !s.is_empty()
                    && suffix.len() == 3
                    && suffix.chars().all(|c| c.is_ascii_digit()) =>
            {
                s
            }
            _ => base,
        };
        (1..)
            .map(|i| format!("{stem}.{i:03}"))
            .find(|c| !taken.contains(c.as_str()))
            .expect("infinite candidates")
    }

    /// Add a node (its children list is reset) under `parent` (or as a root),
    /// appended at the end. Returns the id.
    pub fn add_node(&mut self, node: Node, parent: Option<Uuid>) -> CoreResult<Uuid> {
        self.insert_node(node, parent, None)
    }

    /// Add a node at a given position among its siblings.
    pub fn insert_node(
        &mut self,
        mut node: Node,
        parent: Option<Uuid>,
        index: Option<usize>,
    ) -> CoreResult<Uuid> {
        if self.nodes.contains_key(&node.id) {
            return Err(CoreError::DuplicateNode(node.id));
        }
        if let Some(p) = parent {
            self.node(p)?;
        }
        node.children.clear();
        node.parent = parent;
        let id = node.id;
        self.nodes.insert(id, node);
        self.link(id, parent, index);
        Ok(id)
    }

    fn sibling_list_mut(&mut self, parent: Option<Uuid>) -> &mut Vec<Uuid> {
        match parent {
            Some(p) => &mut self.nodes.get_mut(&p).expect("parent exists").children,
            None => &mut self.roots,
        }
    }

    fn link(&mut self, id: Uuid, parent: Option<Uuid>, index: Option<usize>) {
        let list = self.sibling_list_mut(parent);
        let i = index.unwrap_or(list.len()).min(list.len());
        list.insert(i, id);
        self.nodes.get_mut(&id).expect("node exists").parent = parent;
    }

    /// Detach from parent/roots; returns the former index.
    fn unlink(&mut self, id: Uuid) -> usize {
        let parent = self.nodes[&id].parent;
        let list = self.sibling_list_mut(parent);
        let i = list.iter().position(|&c| c == id).unwrap_or(list.len());
        if i < list.len() {
            list.remove(i);
        }
        i
    }

    /// Position of a node among its siblings.
    pub fn sibling_index(&self, id: Uuid) -> CoreResult<usize> {
        let n = self.node(id)?;
        let list = match n.parent {
            Some(p) => &self.node(p)?.children,
            None => &self.roots,
        };
        Ok(list.iter().position(|&c| c == id).unwrap_or(0))
    }

    /// Move a node under a new parent (or to the roots). Rejects cycles.
    /// With `keep_world_transform` the local transform is adjusted so the
    /// world transform does not change.
    pub fn set_parent(
        &mut self,
        child: Uuid,
        new_parent: Option<Uuid>,
        keep_world_transform: bool,
        index: Option<usize>,
    ) -> CoreResult<()> {
        self.node(child)?;
        if let Some(p) = new_parent {
            self.node(p)?;
            if p == child || self.is_ancestor(child, p) {
                return Err(CoreError::HierarchyCycle { child, parent: p });
            }
        }
        let world = self.world_transform(child)?;
        self.unlink(child);
        self.link(child, new_parent, index);
        if keep_world_transform {
            let pw = match new_parent {
                Some(p) => self.world_transform(p)?,
                None => Affine3A::IDENTITY,
            };
            self.nodes.get_mut(&child).expect("exists").transform = pw.inverse() * world;
        }
        Ok(())
    }

    /// Remove a node and all of its descendants.
    pub fn remove_subtree(&mut self, id: Uuid) -> CoreResult<RemovedSubtree> {
        let parent = self.node(id)?.parent;
        let index = self.unlink(id);
        let mut ids = vec![id];
        ids.extend(self.descendants(id));
        let nodes = ids
            .into_iter()
            .map(|i| self.nodes.remove(&i).expect("descendant exists"))
            .collect();
        Ok(RemovedSubtree {
            nodes,
            parent,
            index,
        })
    }

    /// Re-insert a subtree removed with [`Scene::remove_subtree`].
    pub fn restore_subtree(&mut self, sub: RemovedSubtree) -> CoreResult<()> {
        if let Some(p) = sub.parent {
            self.node(p)?;
        }
        for n in &sub.nodes {
            if self.nodes.contains_key(&n.id) {
                return Err(CoreError::DuplicateNode(n.id));
            }
        }
        let root = sub.nodes.first().map(|n| n.id);
        for n in sub.nodes {
            self.nodes.insert(n.id, n);
        }
        if let Some(r) = root {
            self.link(r, sub.parent, Some(sub.index));
        }
        Ok(())
    }

    /// Verify parent/children links (used after deserialization).
    pub fn check_hierarchy(&self) -> CoreResult<()> {
        let mut seen = HashSet::new();
        for &r in &self.roots {
            let n = self.node(r)?;
            if n.parent.is_some() {
                return Err(CoreError::InvalidCommand(format!("root {r} has a parent")));
            }
        }
        for n in self.nodes.values() {
            for &c in &n.children {
                let child = self.node(c)?;
                if child.parent != Some(n.id) {
                    return Err(CoreError::InvalidCommand(format!(
                        "child {c} does not point back to parent {}",
                        n.id
                    )));
                }
            }
            if let Some(p) = n.parent {
                if !self.node(p)?.children.contains(&n.id) {
                    return Err(CoreError::InvalidCommand(format!(
                        "node {} missing from its parent's children",
                        n.id
                    )));
                }
            } else if !self.roots.contains(&n.id) {
                return Err(CoreError::InvalidCommand(format!(
                    "parentless node {} is not a root",
                    n.id
                )));
            }
        }
        for id in self.traverse() {
            if !seen.insert(id) {
                return Err(CoreError::HierarchyCycle {
                    child: id,
                    parent: id,
                });
            }
        }
        if seen.len() != self.nodes.len() {
            return Err(CoreError::InvalidCommand(
                "scene hierarchy contains unreachable nodes or cycles".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn unique_names() {
        let mut s = Scene::new();
        s.add_node(Node::new("Cube"), None).unwrap();
        assert_eq!(s.unique_name("Cube"), "Cube.001");
        s.add_node(Node::new("Cube.001"), None).unwrap();
        assert_eq!(s.unique_name("Cube.001"), "Cube.002");
        assert_eq!(s.unique_name("Sphere"), "Sphere");
    }

    #[test]
    fn world_transform_and_keep_world() {
        let mut s = Scene::new();
        let a = s
            .add_node(
                Node::new("a").with_transform(Affine3A::from_translation(Vec3::X)),
                None,
            )
            .unwrap();
        let b = s
            .add_node(
                Node::new("b").with_transform(Affine3A::from_translation(Vec3::Y)),
                Some(a),
            )
            .unwrap();
        let w = s.world_transform(b).unwrap();
        assert!(w
            .translation
            .abs_diff_eq(glam::Vec3A::new(1.0, 1.0, 0.0), 1e-6));
        s.set_parent(b, None, true, None).unwrap();
        let w2 = s.world_transform(b).unwrap();
        assert!(w2.abs_diff_eq(w, 1e-6));
        assert_eq!(s.roots().len(), 2);
    }
}

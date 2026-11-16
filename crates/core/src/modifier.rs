//! Non-destructive modifier stack.
//!
//! Every node may carry an ordered list of [`Modifier`]s. The base mesh
//! (`Node::mesh`) is what edit operations change; the *evaluated* mesh is the
//! base mesh run through all enabled modifiers in order and is what
//! rendering and export use. Applying a modifier bakes it into the base mesh.

use std::borrow::Cow;

use glam::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::math::Axis;
use crate::mesh::{EdgeId, Mesh};
use crate::ops;

fn d_true() -> bool {
    true
}
fn d_threshold() -> f32 {
    0.001
}
fn d_count() -> u32 {
    2
}
fn d_offset() -> [f32; 3] {
    [2.0, 0.0, 0.0]
}
fn d_one() -> u32 {
    1
}
fn d_thickness() -> f32 {
    0.1
}
fn d_width() -> f32 {
    0.05
}
fn d_segments() -> u32 {
    3
}
fn d_angle() -> f32 {
    30.0
}

/// Modifier type and parameters (tagged by `type`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModifierKind {
    /// Mirror across the local plane perpendicular to `axis`; with `merge`,
    /// vertices within `merge_threshold` of the plane are welded.
    Mirror {
        axis: Axis,
        #[serde(default = "d_true")]
        merge: bool,
        #[serde(default = "d_threshold")]
        merge_threshold: f32,
    },
    /// `count` copies, each shifted by `offset` (local units) from the
    /// previous; `merge` welds touching copies.
    Array {
        #[serde(default = "d_count")]
        count: u32,
        #[serde(default = "d_offset")]
        offset: [f32; 3],
        #[serde(default)]
        merge: bool,
        #[serde(default = "d_threshold")]
        merge_threshold: f32,
    },
    /// Catmull–Clark subdivision surface.
    Subdivision {
        #[serde(default = "d_one")]
        levels: u32,
    },
    /// Shell of `thickness` grown against the normals (rim on open edges).
    Solidify {
        #[serde(default = "d_thickness")]
        thickness: f32,
    },
    /// Bevel every interior edge whose faces meet at more than `angle_deg`
    /// (width is clamped to fit short edges).
    Bevel {
        #[serde(default = "d_width")]
        width: f32,
        #[serde(default = "d_segments")]
        segments: u32,
        #[serde(default = "d_angle")]
        angle_deg: f32,
    },
    /// Split all faces into triangles.
    Triangulate,
}

/// One entry of a node's modifier stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Modifier {
    /// Disabled modifiers are kept but skipped during evaluation.
    #[serde(default = "d_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: ModifierKind,
}

impl Modifier {
    pub fn new(kind: ModifierKind) -> Modifier {
        Modifier {
            enabled: true,
            kind,
        }
    }
}

/// A modifier that failed during evaluation (it is skipped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModifierWarning {
    pub index: usize,
    pub code: String,
    pub message: String,
}

impl ModifierKind {
    /// Short type name (`"mirror"`, `"array"`, …).
    pub fn type_name(&self) -> &'static str {
        match self {
            ModifierKind::Mirror { .. } => "mirror",
            ModifierKind::Array { .. } => "array",
            ModifierKind::Subdivision { .. } => "subdivision",
            ModifierKind::Solidify { .. } => "solidify",
            ModifierKind::Bevel { .. } => "bevel",
            ModifierKind::Triangulate => "triangulate",
        }
    }

    /// Check parameter ranges without evaluating.
    pub fn validate(&self) -> CoreResult<()> {
        let fin = |n: &str, v: f32| {
            if v.is_finite() {
                Ok(())
            } else {
                Err(CoreError::param(n, "must be a finite number"))
            }
        };
        match self {
            ModifierKind::Mirror {
                merge_threshold, ..
            } => {
                fin("merge_threshold", *merge_threshold)?;
                if *merge_threshold < 0.0 {
                    return Err(CoreError::param("merge_threshold", "must be >= 0"));
                }
            }
            ModifierKind::Array {
                count,
                offset,
                merge_threshold,
                ..
            } => {
                if !(1..=ops::MAX_ARRAY_COUNT).contains(count) {
                    return Err(CoreError::param(
                        "count",
                        format!("must be in 1..={}", ops::MAX_ARRAY_COUNT),
                    ));
                }
                for v in offset {
                    fin("offset", *v)?;
                }
                fin("merge_threshold", *merge_threshold)?;
            }
            ModifierKind::Subdivision { levels } => {
                if !(1..=ops::MAX_SUBDIVISION_LEVELS).contains(levels) {
                    return Err(CoreError::param(
                        "levels",
                        format!("must be in 1..={}", ops::MAX_SUBDIVISION_LEVELS),
                    ));
                }
            }
            ModifierKind::Solidify { thickness } => {
                fin("thickness", *thickness)?;
                if thickness.abs() < 1e-6 {
                    return Err(CoreError::param("thickness", "must be non-zero"));
                }
            }
            ModifierKind::Bevel {
                width,
                segments,
                angle_deg,
            } => {
                if !(width.is_finite() && *width > 0.0) {
                    return Err(CoreError::param(
                        "width",
                        "must be a positive finite number",
                    ));
                }
                if !(1..=ops::MAX_BEVEL_SEGMENTS).contains(segments) {
                    return Err(CoreError::param(
                        "segments",
                        format!("must be in 1..={}", ops::MAX_BEVEL_SEGMENTS),
                    ));
                }
                if !(angle_deg.is_finite() && (0.0..180.0).contains(angle_deg)) {
                    return Err(CoreError::param("angle_deg", "must be in 0..180"));
                }
            }
            ModifierKind::Triangulate => {}
        }
        Ok(())
    }

    /// Evaluate this modifier on `mesh`.
    pub fn apply(&self, mesh: &Mesh) -> CoreResult<Mesh> {
        self.validate()?;
        match self {
            ModifierKind::Mirror {
                axis,
                merge,
                merge_threshold,
            } => ops::mirror(mesh, *axis, *merge, *merge_threshold),
            ModifierKind::Array {
                count,
                offset,
                merge,
                merge_threshold,
            } => ops::array(
                mesh,
                *count,
                Vec3::from_array(*offset),
                *merge,
                *merge_threshold,
            ),
            ModifierKind::Subdivision { levels } => ops::subdivide_catmull_clark(mesh, *levels),
            ModifierKind::Solidify { thickness } => ops::solidify(mesh, *thickness),
            ModifierKind::Bevel {
                width,
                segments,
                angle_deg,
            } => {
                let cos = angle_deg.to_radians().cos();
                let edges: Vec<EdgeId> = mesh
                    .edge_ids()
                    .filter(|&e| {
                        let fs = mesh.edge_faces(e);
                        fs.len() == 2
                            && fs[0] != fs[1]
                            && mesh.face_normal(fs[0]).dot(mesh.face_normal(fs[1])) < cos
                    })
                    .collect();
                if edges.is_empty() {
                    return Ok(mesh.clone());
                }
                let min_len = edges
                    .iter()
                    .map(|&e| mesh.edge_length(e))
                    .fold(f32::INFINITY, f32::min);
                let w = width.min(0.45 * min_len);
                Ok(ops::bevel_edges(mesh, &edges, w, *segments)?.mesh)
            }
            ModifierKind::Triangulate => {
                if mesh.face_count() == 0 {
                    return Ok(mesh.clone());
                }
                let all: Vec<_> = mesh.face_ids().collect();
                Ok(ops::triangulate_faces(mesh, &all)?.mesh)
            }
        }
    }
}

/// Run `mesh` through the enabled modifiers. Failing modifiers are skipped
/// and reported.
pub fn evaluate<'a>(mesh: &'a Mesh, stack: &[Modifier]) -> (Cow<'a, Mesh>, Vec<ModifierWarning>) {
    let mut cur: Cow<'a, Mesh> = Cow::Borrowed(mesh);
    let mut warnings = Vec::new();
    for (i, m) in stack.iter().enumerate().filter(|(_, m)| m.enabled) {
        match m.kind.apply(&cur) {
            Ok(next) => cur = Cow::Owned(next),
            Err(e) => warnings.push(ModifierWarning {
                index: i,
                code: e.code().to_string(),
                message: e.to_string(),
            }),
        }
    }
    (cur, warnings)
}

//! Small math helpers shared across the crate.

use glam::{Affine3A, EulerRot, Quat, Vec3};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// Box from two corners (order independent).
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Aabb {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// Smallest box containing all points, `None` for an empty iterator.
    pub fn from_points<I: IntoIterator<Item = Vec3>>(points: I) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut b = Aabb {
            min: first,
            max: first,
        };
        for p in it {
            b.expand(p);
        }
        Some(b)
    }

    /// Grow the box to include `p`.
    pub fn expand(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    /// Union of two boxes.
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// Length of the diagonal.
    pub fn diagonal(&self) -> f32 {
        self.size().length()
    }

    /// Inclusive point containment test.
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Whether two boxes overlap (inclusive).
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    /// Transform the 8 corners and return their bounding box.
    pub fn transformed(&self, m: &Affine3A) -> Aabb {
        let mut out: Option<Aabb> = None;
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            );
            let p = m.transform_point3(c);
            match &mut out {
                Some(b) => b.expand(p),
                None => out = Some(Aabb { min: p, max: p }),
            }
        }
        out.expect("8 corners")
    }
}

/// Coordinate axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }

    pub fn unit(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

/// Coordinate space for relative transforms and spatial queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    /// World coordinates.
    #[default]
    World,
    /// The object's own (local) coordinates.
    Local,
}

/// Euler angles in degrees (XYZ order) to a quaternion.
pub fn quat_from_euler_deg(deg: [f32; 3]) -> Quat {
    Quat::from_euler(
        EulerRot::XYZ,
        deg[0].to_radians(),
        deg[1].to_radians(),
        deg[2].to_radians(),
    )
}

/// Quaternion to Euler angles in degrees (XYZ order).
pub fn euler_deg_from_quat(q: Quat) -> [f32; 3] {
    let (x, y, z) = q.to_euler(EulerRot::XYZ);
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// Build an affine transform from translation, rotation and scale.
pub fn trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Affine3A {
    Affine3A::from_scale_rotation_translation(scale, rotation, translation)
}

/// Newell's method: un-normalized polygon normal whose length is twice the area.
pub fn newell_normal<I: IntoIterator<Item = Vec3>>(points: I) -> Vec3 {
    let pts: Vec<Vec3> = points.into_iter().collect();
    newell_normal_slice(&pts)
}

/// Newell's method on a slice.
pub fn newell_normal_slice(pts: &[Vec3]) -> Vec3 {
    let n = pts.len();
    let mut acc = Vec3::ZERO;
    if n < 3 {
        return acc;
    }
    // Use the centroid as origin for numerical robustness.
    let c = pts.iter().copied().sum::<Vec3>() / n as f32;
    for i in 0..n {
        let a = pts[i] - c;
        let b = pts[(i + 1) % n] - c;
        acc += a.cross(b);
    }
    acc
}

/// Approximate float comparison of vectors.
pub fn approx_eq_vec(a: Vec3, b: Vec3, eps: f32) -> bool {
    (a - b).abs().max_element() <= eps
}

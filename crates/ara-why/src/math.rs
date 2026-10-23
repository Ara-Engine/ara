//! Just enough transform math for explanations: world positions, axis scales and
//! view directions from `Transform` values (Y-up, right-handed, XYZ Euler degrees
//! composed as `Rx · Ry · Rz`, cameras look down −Z).

use serde_json::Value;

/// A 3-vector.
pub type Vec3 = [f64; 3];

/// An affine transform: a 3×3 linear part (rotation · scale, columns are axes) and
/// a translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    /// Column-major linear part.
    pub cols: [Vec3; 3],
    /// Translation.
    pub t: Vec3,
}

impl Affine {
    /// The identity.
    pub const IDENTITY: Affine = Affine {
        cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    /// From position, XYZ Euler degrees and scale.
    pub fn from_trs(position: Vec3, rotation_deg: Vec3, scale: Vec3) -> Affine {
        let [x, y, z] = rotation_deg.map(f64::to_radians);
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        // R = Rx · Ry · Rz, written as rows.
        let r = [
            [cy * cz, -cy * sz, sy],
            [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
            [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
        ];
        let col = |j: usize| [r[0][j] * scale[j], r[1][j] * scale[j], r[2][j] * scale[j]];
        Affine {
            cols: [col(0), col(1), col(2)],
            t: position,
        }
    }

    /// From a `Transform` component value (missing fields take their defaults).
    pub fn from_transform(v: Option<&Value>) -> Affine {
        let field = |k: &str, d: f64| vec3(v.and_then(|v| v.get(k))).unwrap_or([d; 3]);
        Affine::from_trs(field("position", 0.0), field("rotation", 0.0), field("scale", 1.0))
    }

    /// Applies the linear part.
    pub fn vector(&self, v: Vec3) -> Vec3 {
        let c = &self.cols;
        [
            c[0][0] * v[0] + c[1][0] * v[1] + c[2][0] * v[2],
            c[0][1] * v[0] + c[1][1] * v[1] + c[2][1] * v[2],
            c[0][2] * v[0] + c[1][2] * v[1] + c[2][2] * v[2],
        ]
    }

    /// Applies the transform to a point.
    pub fn point(&self, p: Vec3) -> Vec3 {
        let v = self.vector(p);
        [v[0] + self.t[0], v[1] + self.t[1], v[2] + self.t[2]]
    }

    /// `self · child` (child expressed in this frame).
    pub fn then(&self, child: &Affine) -> Affine {
        Affine {
            cols: child.cols.map(|c| self.vector(c)),
            t: self.point(child.t),
        }
    }

    /// Length of each axis (the world scale per axis, ignoring shear).
    pub fn axis_scales(&self) -> Vec3 {
        self.cols.map(length)
    }

    /// True if every number is finite.
    pub fn is_finite(&self) -> bool {
        self.cols.iter().flatten().chain(self.t.iter()).all(|x| x.is_finite())
    }
}

/// Euclidean length.
pub fn length(v: Vec3) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `a − b`.
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Dot product.
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A JSON `[x, y, z]`. Non-numbers (`null`, how documents hold NaN) become NaN, so
/// callers see a non-finite value instead of a default.
pub fn vec3(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some([0, 1, 2].map(|i| a[i].as_f64().unwrap_or(f64::NAN)))
}

/// A number for display: `12`, `0.5`, `NaN`.
pub fn show(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{x:.0}")
    } else {
        let s = format!("{x:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// A vector for display: `[1, 0, 1]`.
pub fn show3(v: Vec3) -> String {
    format!("[{}, {}, {}]", show(v[0]), show(v[1]), show(v[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        length(sub(a, b)) < 1e-9
    }

    #[test]
    fn composes_parents_like_the_editor() {
        let parent = Affine::from_trs([10.0, 0.0, 0.0], [0.0, 90.0, 0.0], [2.0, 2.0, 2.0]);
        let child = Affine::from_trs([1.0, 0.0, 0.0], [0.0; 3], [1.0, 0.0, 1.0]);
        let world = parent.then(&child);
        // Yaw 90° turns +X into −Z; scale 2 doubles the offset.
        assert!(close(world.t, [10.0, 0.0, -2.0]), "{:?}", world.t);
        assert!(close(world.axis_scales(), [2.0, 0.0, 2.0]));
        let forward = Affine::from_trs([0.0; 3], [0.0, 180.0, 0.0], [1.0; 3]).vector([0.0, 0.0, -1.0]);
        assert!(close(forward, [0.0, 0.0, 1.0]));
        assert!(!Affine::from_transform(Some(&serde_json::json!({ "position": [0, null, 0] }))).is_finite());
        assert_eq!(show3([1.0, 0.25, f64::NAN]), "[1, 0.25, NaN]");
    }
}

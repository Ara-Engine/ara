use crate::{Result, RuntimeError, model::TextureTransform};
use glam::{Mat2, Vec2};

/// Affine UV expressions are lowered once; sampling retains the source UV stream.
#[derive(Debug, Clone, Copy)]
pub(super) struct Affine {
    pub matrix: Mat2,
    pub offset: Vec2,
}

impl Affine {
    pub fn identity() -> Self {
        Self {
            matrix: Mat2::IDENTITY,
            offset: Vec2::ZERO,
        }
    }

    pub fn constant(value: Vec2) -> Self {
        Self {
            matrix: Mat2::ZERO,
            offset: value,
        }
    }

    pub fn scale(self, value: Vec2) -> Self {
        let scale = Mat2::from_diagonal(value);
        Self {
            matrix: scale * self.matrix,
            offset: scale * self.offset,
        }
    }

    pub fn translate(self, value: Vec2) -> Self {
        Self {
            offset: self.offset + value,
            ..self
        }
    }

    pub fn rotate(self, radians: f32) -> Self {
        let rotation = Mat2::from_angle(radians);
        let center = Vec2::splat(0.5);
        Self {
            matrix: rotation * self.matrix,
            offset: center + rotation * (self.offset - center),
        }
    }

    pub fn texture(self, asset: &str, node: &str) -> Result<TextureTransform> {
        let invalid = |reason: &str| RuntimeError(format!("{asset}#{node}: {reason}"));
        if !self.matrix.is_finite() || !self.offset.is_finite() {
            return Err(invalid("non-finite UV transform"));
        }
        let (x, y) = (self.matrix.x_axis, self.matrix.y_axis);
        let (sx, sy) = (x.length(), y.length());
        if x.dot(y).abs() > 1e-5 * (sx * sy).max(1e-12) {
            return Err(invalid(
                "UV shear cannot be represented by a texture transform",
            ));
        }
        let rotation = if sx > 1e-8 {
            x.y.atan2(x.x)
        } else if sy > 1e-8 {
            (-y.x).atan2(y.y)
        } else {
            0.
        };
        let sy = if sx > 1e-8 && self.matrix.determinant() < 0. {
            -sy
        } else {
            sy
        };
        let transform = TextureTransform {
            offset: self.offset.to_array(),
            scale: [sx, sy],
            rotation,
            tex_coord: 0,
        };
        // The threshold above only accepts a decomposition that reconstructs the expression.
        for point in [Vec2::ZERO, Vec2::X, Vec2::Y] {
            if !Vec2::from_array(transform.apply(point.to_array()))
                .abs_diff_eq(self.matrix * point + self.offset, 1e-4)
            {
                return Err(invalid("UV transform cannot be represented exactly"));
            }
        }
        Ok(transform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_uv_expressions_retain_order_rotation_center_and_mirroring() {
        for scale in [
            Vec2::new(2., 3.),
            Vec2::new(-2., 3.),
            Vec2::new(0., 3.),
            Vec2::ZERO,
        ] {
            let affine = Affine::identity()
                .scale(scale)
                .rotate(0.6)
                .translate(Vec2::new(0.2, -0.3));
            let transform = affine.texture("gr_test", "n_uv").expect("TRS");
            for point in [Vec2::new(0.1, 0.4), Vec2::new(-2., 3.)] {
                assert!(
                    Vec2::from_array(transform.apply(point.to_array()))
                        .abs_diff_eq(affine.matrix * point + affine.offset, 1e-4)
                );
            }
        }
        assert!(
            Affine::identity()
                .rotate(0.6)
                .scale(Vec2::new(2., 3.))
                .texture("gr_test", "n_uv")
                .is_err()
        );
    }
}

//! Pixel/texel budgets use the actual pass projection, including orthographic
//! cameras and perspective local lights. No camera distance leaks into shadows.
use super::*;

impl View {
    pub(super) fn new(matrix: Mat4, size: [u32; 2], error: f32, minimum: f32) -> Self {
        Self {
            matrix,
            size,
            error,
            minimum,
            // The projection is shared by every instance in this pass.
            row_lengths: Vec3::new(
                matrix.row(0).truncate().length(),
                matrix.row(1).truncate().length(),
                matrix.row(3).truncate().length(),
            ),
        }
    }
    pub(in crate::renderer::native) fn shadow(matrix: Mat4, resolution: u32, minimum: f32) -> Self {
        Self::new(matrix, [resolution; 2], 1., minimum.max(2.))
    }

    pub(super) fn lod(self, group: &Group, row: &InstanceRow, bounds: [Vec3; 2]) -> Option<usize> {
        let radius = (bounds[1] - bounds[0]).length() * 0.5;
        let pixels = self.density((bounds[0] + bounds[1]) * 0.5, radius);
        if radius * pixels < self.minimum {
            return None;
        }
        let scale = row
            .world
            .x_axis
            .truncate()
            .length_squared()
            .max(row.world.y_axis.truncate().length_squared())
            .max(row.world.z_axis.truncate().length_squared())
            .sqrt();
        let error = self.error / pixels;
        Some(
            group
                .errors
                .iter()
                .enumerate()
                .rev()
                .find_map(|(lod, value)| (*value * scale <= error).then_some(lod))
                .unwrap_or(0),
        )
    }

    /// Conservative per-axis pixels per world metre over the bounding sphere. Crossing
    /// the eye plane keeps full detail; the six-plane frustum test still rejects
    /// bounds wholly outside the view. Orthographic projection has constant W.
    fn density(self, centre: Vec3, radius: f32) -> f32 {
        let w = self.matrix.row(3);
        let nearest = w.dot(centre.extend(1.)) - radius * self.row_lengths.z;
        if nearest <= 1e-6 {
            return f32::INFINITY;
        }
        let rate = |row: Vec4, length: f32| {
            let ndc = (row.dot(centre.extend(1.)).abs() + radius * length) / nearest;
            length + ndc * self.row_lengths.z
        };
        let x = rate(self.matrix.row(0), self.row_lengths.x) * self.size[0] as f32;
        let y = rate(self.matrix.row(1), self.row_lengths.y) * self.size[1] as f32;
        0.5 * x.max(y) / nearest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orthographic_texel_density_is_distance_independent_and_eye_intersections_are_kept() {
        let ortho = View::shadow(
            Mat4::orthographic_rh(-50., 50., -50., 50., 100., 0.05),
            512,
            0.,
        );
        assert_eq!(
            ortho.density(Vec3::new(0., 0., -1.), 0.5),
            ortho.density(Vec3::new(0., 0., -90.), 0.5)
        );
        let perspective = View::shadow(Mat4::perspective_rh(1., 1., 100., 0.05), 512, 0.);
        assert!(
            perspective
                .density(Vec3::new(0., 0., -0.1), 0.2)
                .is_infinite()
        );
        assert!(
            perspective.density(Vec3::new(0., 0., -10.), 0.5)
                > perspective.density(Vec3::new(0., 0., -20.), 0.5)
        );
    }

    #[test]
    fn off_axis_density_conservatively_bounds_projected_displacements() {
        let view = View::shadow(Mat4::perspective_rh(1., 1., 100., 0.05), 512, 0.);
        let centre = Vec3::new(2., 1.5, -5.);
        let density = view.density(centre, 0.75);
        let screen = |position: Vec3| {
            let p = view.matrix * position.extend(1.);
            p.truncate().truncate() / p.w * 256.
        };
        for offset in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
            let offset = offset * 0.75;
            let displacement = (screen(centre + offset) - screen(centre))
                .abs()
                .max_element();
            assert!(displacement <= density * offset.length());
        }
    }
}

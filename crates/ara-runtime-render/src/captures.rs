//! Reflection captures for the raster renderer (`ara_render::look::captures`): the
//! frame's enabled authored `ReflectionProbe` components in world space, or, without
//! any, automatic placement over the published content (computed once per generation).
use ara_render::instanced::Scene;
use ara_render::look::captures::{CaptureSpec, auto_place};
use ara_runtime::extract::{ProbeView, RenderFrame};
use glam30::{Quat, Vec3};

/// Automatic placements of one content generation (`RasterView`).
#[derive(Default)]
pub(crate) struct Placement {
    key: Option<(u64, usize)>,
    automatic: Vec<CaptureSpec>,
}

impl Placement {
    /// The captures the renderer should bake for `frame` over content `scene`
    /// (generation `key`): authored ones when the frame has any, else at most
    /// `limit` automatic ones (zero: the quality bakes none).
    pub(crate) fn specs(
        &mut self,
        frame: &RenderFrame,
        scene: &Scene,
        key: u64,
        limit: usize,
    ) -> Vec<CaptureSpec> {
        if limit == 0 {
            return Vec::new();
        }
        if !frame.reflection_probes.is_empty() {
            return frame
                .reflection_probes
                .iter()
                .filter(|v| v.probe.enabled && v.probe.invalid().is_none())
                .map(authored)
                .collect();
        }
        if self.key != Some((key, limit)) {
            self.automatic = auto_place(scene, limit);
            self.key = Some((key, limit));
        }
        self.automatic.clone()
    }
}

/// An authored probe in world space: the entity's scale stretches its boxes and
/// offsets (half sizes of the authored full `bounds`); a projection box with any zero
/// size uses the box of influence.
pub(crate) fn authored(view: &ProbeView) -> CaptureSpec {
    let c = &view.probe;
    let scale = Vec3::from_array(view.scale).abs().max(Vec3::splat(1e-4));
    let extents = (Vec3::from_array(c.bounds) * 0.5 * scale).max(Vec3::splat(0.005));
    let projection = Vec3::from_array(c.projection_bounds) * 0.5 * scale;
    let rotation = Quat::from_array(view.rotation);
    CaptureSpec {
        position: Vec3::from_array(view.translation),
        rotation: if rotation.is_finite() && rotation.length() > 0.5 {
            rotation.normalize()
        } else {
            Quat::IDENTITY
        },
        influence: extents,
        blend: c.blend_distance.max(0.),
        projection_centre: Vec3::from_array(c.projection_offset) * scale,
        projection: if projection.min_element() > 0. {
            projection
        } else {
            extents
        },
        capture_offset: Vec3::from_array(c.capture_offset) * scale,
        priority: c.priority,
        resolution: c.capture_resolution,
        intensity: c.intensity.max(0.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_components::render::ReflectionProbe;

    #[test]
    fn authored_probes_follow_their_entity_and_default_the_projection_box() {
        let view = ProbeView {
            address: "probe".into(),
            entity: 7,
            probe: ReflectionProbe {
                bounds: [4., 2., 6.],
                capture_offset: [0., 0.5, 0.],
                capture_resolution: 512,
                ..Default::default()
            },
            translation: [1., 2., 3.],
            rotation: Quat::from_rotation_y(1.).to_array(),
            scale: [2., 2., 2.],
        };
        let spec = authored(&view);
        spec.validate().expect("valid authored capture");
        assert_eq!(spec.influence, Vec3::new(4., 2., 6.));
        assert_eq!(spec.projection, spec.influence);
        assert_eq!(spec.capture_offset, Vec3::new(0., 1., 0.));
        assert_eq!(spec.resolution, 512);
        assert!((spec.capture_point() - Vec3::new(1., 3., 3.)).length() < 1e-5);
    }
}

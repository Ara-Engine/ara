//! Reflection captures for the raster renderer (`ara_render::look::captures`): the
//! frame's enabled authored `ReflectionProbe` components in world space, or, without
//! any and when the quality opts in, automatic placement: from what the probe volume
//! saw once its probes arrive (`ara_render::look::placement`), over the published
//! content's bounds until then (computed once per generation and probe delivery).
use ara_render::instanced::Scene;
use ara_render::look::captures::{CaptureSpec, auto_place};
use ara_render::look::gi::{ProbeLayout, ProbeUpdate};
use ara_render::look::placement::{ProbeSight, place_from_probes};
use ara_runtime::extract::{ProbeView, RenderFrame};
use glam30::{Quat, Vec3};

/// Automatic placements of one content generation (`RasterView`).
#[derive(Default)]
pub(crate) struct Placement {
    key: Option<(u64, usize)>,
    automatic: Vec<CaptureSpec>,
    /// What the delivered probes saw, by id, for the current probe layout.
    sights: Option<(ProbeLayout, Vec<Option<ProbeSight>>)>,
    /// Probes arrived since the last placement.
    sighted: bool,
    /// The last [`Self::specs`] placed automatically, with this limit.
    placing: Option<usize>,
}

impl Placement {
    /// Notes a probe delivery: its sights replace those of the same probes (a new
    /// layout starts over), and the next [`Self::specs`] places again.
    pub(crate) fn observe(&mut self, update: &ProbeUpdate) {
        let layout = update.layout();
        if self.sights.as_ref().is_none_or(|(l, _)| *l != layout) {
            self.sights = Some((layout, vec![None; layout.probe_count()]));
        }
        if let Some((_, sights)) = &mut self.sights {
            let ids: Vec<u32> = update.ids(0..update.len()).collect();
            for &id in &ids {
                if let Some(s) = sights.get_mut(id as usize) {
                    *s = None;
                }
            }
            for (id, sight) in update.sights() {
                if let Some(s) = sights.get_mut(id as usize) {
                    *s = Some(sight);
                }
            }
            self.sighted = true;
        }
    }

    /// The captures the renderer should bake for `frame` over content `scene`
    /// (generation `key`): authored ones when the frame has any, else at most
    /// `limit` automatic ones when `automatic` (zero: the quality bakes none).
    pub(crate) fn specs(
        &mut self,
        frame: &RenderFrame,
        scene: &Scene,
        key: u64,
        limit: usize,
        automatic: bool,
    ) -> Vec<CaptureSpec> {
        self.placing = None;
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
        if !automatic {
            return Vec::new();
        }
        self.placing = Some(limit);
        self.specs_automatic(scene, key, limit)
    }

    /// The automatic placement again when probes arrived after the last
    /// [`Self::specs`] placed automatically (a capture boundary waits for the probe
    /// volume after preparing its frame); `None` otherwise.
    pub(crate) fn after_probes(&mut self, scene: &Scene, key: u64) -> Option<Vec<CaptureSpec>> {
        let limit = self.placing?;
        self.sighted.then(|| {
            self.key = None;
            self.specs_automatic(scene, key, limit)
        })
    }

    fn specs_automatic(&mut self, scene: &Scene, key: u64, limit: usize) -> Vec<CaptureSpec> {
        if self.key != Some((key, limit)) || self.sighted {
            self.automatic = match &self.sights {
                Some((layout, sights)) if sights.iter().any(Option::is_some) => {
                    let specs = place_from_probes(layout, sights, limit);
                    eprintln!(
                        "ara-runtime-render: placed {} reflection captures where the probes saw open space: {:?}",
                        specs.len(),
                        specs
                            .iter()
                            .map(|c| (
                                c.capture_point()
                                    .to_array()
                                    .map(|v| (v * 10.).round() / 10.),
                                (c.position - c.influence)
                                    .to_array()
                                    .map(|v| (v * 10.).round() / 10.),
                                (c.position + c.influence)
                                    .to_array()
                                    .map(|v| (v * 10.).round() / 10.)
                            ))
                            .collect::<Vec<_>>()
                    );
                    specs
                }
                _ => auto_place(scene, limit),
            };
            self.key = Some((key, limit));
            self.sighted = false;
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
        key: view.entity,
        // Authored probes bake through capture jobs (`crate::probes`) or cache loads.
        explicit: true,
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

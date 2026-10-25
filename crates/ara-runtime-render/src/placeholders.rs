//! Presentation policy for models whose first cook has not arrived yet.
use ara_runtime::extract::{Geometry, RenderFrame};
use std::collections::{BTreeMap, BTreeSet};

/// Quiet loading bounds, distinct from the magenta missing/broken placeholder.
pub const PENDING_MODEL_COLOR: [f32; 4] = [0.65, 0.65, 0.65, 0.18];

/// Tracks visible pending models across publications. This is derived presentation
/// state; it never schedules imports or changes an authored component.
#[derive(Default, Clone)]
pub struct ModelPlaceholders {
    pending: BTreeMap<String, String>,
    arrivals: BTreeSet<String>,
}
impl ModelPlaceholders {
    /// Style fallback shapes using asset IDs keyed by draw address. `is_pending`
    /// must be false for failed/unregistered IDs and whenever a generation exists.
    /// Existing model draws keep their last-good materials, including during a
    /// reimport. `whole` removes state for entities absent from a complete frame;
    /// value patches pass false so unrelated entities retain their state.
    pub fn style(
        &mut self,
        frame: &mut RenderFrame,
        models: &BTreeMap<String, String>,
        is_pending: impl Fn(&str) -> bool,
        whole: bool,
    ) -> bool {
        let mut changed = false;
        for draw in &mut frame.draws {
            let Some(asset) = models.get(&draw.address) else {
                continue;
            };
            if let Geometry::Model { model, .. } = &draw.geometry {
                if self.pending.remove(&draw.address).as_deref() == Some(model.asset.as_str()) {
                    self.arrivals.insert(model.asset.to_owned());
                }
                continue;
            }
            let color = if is_pending(asset) {
                self.pending
                    .insert(draw.address.to_owned(), asset.to_owned());
                PENDING_MODEL_COLOR
            } else {
                self.pending.remove(&draw.address);
                [1., 0., 1., 1.]
            };
            self.arrivals.remove(asset);
            changed |= draw.color != color
                || draw.roughness != 1.
                || draw.metallic != 0.
                || draw.emissive != [0.; 3]
                || draw.emissive_factor.is_some()
                || draw.material.is_some();
            draw.color = color;
            draw.roughness = 1.;
            draw.metallic = 0.;
            draw.emissive = [0.; 3];
            draw.emissive_factor = None;
            draw.material = None;
        }
        if whole {
            let visible: BTreeSet<_> = frame
                .draws
                .iter()
                .map(|draw| draw.address.as_str())
                .collect();
            let ready: BTreeSet<_> = frame
                .draws
                .iter()
                .filter_map(|draw| match &draw.geometry {
                    Geometry::Model { model, .. } => Some(model.asset.as_str()),
                    _ => None,
                })
                .collect();
            self.pending.retain(|address, asset| {
                visible.contains(address.as_str())
                    && models
                        .get(address)
                        .is_some_and(|requested| requested.as_str() == asset.as_str())
            });
            self.arrivals
                .retain(|address| ready.contains(address.as_str()));
        }
        changed
    }

    /// Consume asset IDs that changed from loading bounds to a real model. Start
    /// their visual fade only after publishing that content generation succeeds.
    pub fn take_arrivals(&mut self) -> BTreeSet<String> {
        std::mem::take(&mut self.arrivals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_runtime::{components::MeshShape, extract::Draw};
    fn frame() -> RenderFrame {
        let mut frame = RenderFrame::default();
        frame.draws = vec![Draw {
            entity: 1,
            address: "e_model".into(),
            label: "Model".into(),
            geometry: Geometry::Shape {
                shape: MeshShape::Box,
                size: [1.; 3],
            },
            world: glam::Mat4::IDENTITY,
            color: [1., 0., 1., 1.],
            roughness: 0.5,
            metallic: 1.,
            emissive: [10.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
        }];
        frame
    }
    #[test]
    fn pending_failed_and_missing_are_distinct_and_procedural_shapes_are_untouched() {
        let mut tracker = ModelPlaceholders::default();
        let mut frame = frame();
        let models = BTreeMap::from([("e_model".into(), "as_model".into())]);
        assert!(tracker.style(&mut frame, &models, |_| true, true));
        assert_eq!(frame.draws[0].color, PENDING_MODEL_COLOR);
        assert_eq!(
            (
                frame.draws[0].roughness,
                frame.draws[0].metallic,
                frame.draws[0].emissive
            ),
            (1., 0., [0.; 3])
        );
        assert!(!tracker.style(&mut frame, &models, |_| true, true));
        assert!(tracker.style(&mut frame, &models, |_| false, true));
        assert_eq!(frame.draws[0].color, [1., 0., 1., 1.]);
        frame.draws[0].color = [0.2, 0.4, 0.8, 1.];
        assert!(!tracker.style(&mut frame, &BTreeMap::new(), |_| true, true));
        assert_eq!(frame.draws[0].color, [0.2, 0.4, 0.8, 1.]);
    }
}

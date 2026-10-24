//! Presentation-only loading bounds. No imports, gameplay state or GPU ownership.
use ara_asset::Digest;
use ara_play::ImportState;
use ara_runtime::{
    assets::asset_id,
    components::MeshRenderer,
    extract::{Geometry, RenderFrame},
};
use ara_sim::Identity;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct PendingModels {
    pub state: ImportState,
    sources: BTreeMap<String, String>,
    entities: Option<u32>,
}
impl PendingModels {
    pub fn invalidate(&mut self) {
        self.entities = None;
    }
    pub fn is_pending(&self, asset: &str, pins: &BTreeMap<String, Digest>) -> bool {
        self.state.pending.contains(asset) && !pins.contains_key(asset)
    }
    pub fn style(
        &mut self,
        runtime: &crate::net::Play,
        pins: &BTreeMap<String, Digest>,
        frame: &mut RenderFrame,
    ) {
        if self.state.pending.is_empty() {
            return;
        }
        let world = runtime.world();
        let entities = world.entities().len();
        // Streamed cells can replace equally sized entity sets. A newly seen
        // fallback address must refresh the cache even when the count matches.
        let unknown = frame.draws.iter().any(|draw| {
            matches!(draw.geometry, Geometry::Shape { .. })
                && draw.color == [1., 0., 1., 1.]
                && !self.sources.contains_key(&draw.address)
        });
        if self.entities != Some(entities) || unknown {
            self.sources = sources(runtime);
            self.entities = Some(entities);
        }
        for draw in &mut frame.draws {
            if !matches!(draw.geometry, Geometry::Shape { .. }) {
                continue;
            }
            if self
                .sources
                .get(&draw.address)
                .is_some_and(|asset| self.is_pending(asset, pins))
            {
                draw.color = ara_runtime_render::PENDING_MODEL_COLOR;
                draw.roughness = 1.;
                draw.metallic = 0.;
                draw.emissive = [0.; 3];
                draw.emissive_factor = None;
                draw.material = None;
            }
        }
    }
}

/// Only called on content boundaries; primitive/material edits need no rescan.
pub(crate) fn sources(runtime: &crate::net::Play) -> BTreeMap<String, String> {
    runtime
        .world()
        .iter_entities()
        .filter_map(|entity| {
            let address = &entity.get::<Identity>()?.0;
            let model = entity.get::<MeshRenderer>()?.model.as_ref()?;
            Some((address.to_owned(), asset_id(model).ok()?.to_owned()))
        })
        .collect()
}

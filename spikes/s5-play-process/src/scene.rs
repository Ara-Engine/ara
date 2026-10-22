//! The JSON scene document (A3 shape: a flat entity map keyed by short IDs) and a deterministic
//! scene generator. JSON is the authoring source of truth and also the "wsJson" snapshot format.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{IdIndex, NextSceneId, SceneId, sorted_entities};
use crate::registry::{component, entity_json};

pub const SCENE_FORMAT: &str = "ara.scene/0";

/// A scene file. `BTreeMap`s and `serde_json::Value` objects keep every key sorted, so writing it
/// is canonical without hand-ordering.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneDoc {
    pub format: String,
    pub next_id: SceneId,
    pub entities: BTreeMap<SceneId, BTreeMap<String, Value>>,
}

impl SceneDoc {
    /// Captures every entity with a stable ID.
    pub fn from_world(world: &mut World) -> Self {
        let rows = sorted_entities(world);
        let entities = rows
            .into_iter()
            .map(|(id, e)| (id, entity_json(world, e, None)))
            .collect();
        Self {
            format: SCENE_FORMAT.to_owned(),
            next_id: SceneId(world.resource::<NextSceneId>().0),
            entities,
        }
    }

    /// Builds a fresh world, inserting each component type as one batch.
    pub fn into_world(self) -> Result<World> {
        if self.format != SCENE_FORMAT {
            bail!("unsupported scene format {:?}", self.format);
        }
        let mut world = World::new();
        let ids: Vec<SceneId> = self.entities.keys().copied().collect();
        let handles: Vec<Entity> = world.spawn_batch(ids.iter().copied()).collect();
        let mut columns: BTreeMap<String, Vec<(Entity, Value)>> = BTreeMap::new();
        for (e, components) in handles.iter().zip(self.entities.into_values()) {
            for (name, value) in components {
                columns.entry(name).or_default().push((*e, value));
            }
        }
        for (name, rows) in columns {
            (component(&name)?.insert_json)(&mut world, rows)?;
        }
        world.insert_resource(IdIndex(ids.into_iter().zip(handles).collect()));
        world.insert_resource(NextSceneId(self.next_id.0));
        Ok(world)
    }
}

/// SplitMix64: a tiny seeded generator, local to the scene generator.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`, rounded to 1/1024 so the JSON stays short.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        ((lo + (hi - lo) * unit) * 1024.0).round() / 1024.0
    }
}

/// Generates `count` entities: 80% orbiters (`Orbit` + `Transform` + `Tint` + `Label`) and 20%
/// static props without `Orbit`, so the world has more than one archetype.
pub fn generate(count: usize, seed: u64) -> SceneDoc {
    let mut rng = SplitMix(seed);
    let mut entities = BTreeMap::new();
    for i in 0..count {
        let id = SceneId(i as u32 + 1);
        let center = [
            rng.range(-200.0, 200.0),
            rng.range(0.0, 20.0),
            rng.range(-200.0, 200.0),
        ];
        let mut components = BTreeMap::new();
        components.insert(
            "Tint".to_owned(),
            json!({ "rgba": [rng.range(0.0, 1.0), rng.range(0.0, 1.0), rng.range(0.0, 1.0), 1.0] }),
        );
        if i % 5 == 4 {
            components.insert("Label".to_owned(), json!(format!("Prop {i}")));
            components.insert(
                "Transform".to_owned(),
                json!({ "position": center, "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] }),
            );
        } else {
            let radius = rng.range(1.0, 12.0);
            components.insert("Label".to_owned(), json!(format!("Orbiter {i}")));
            components.insert(
                "Orbit".to_owned(),
                json!({ "center": center, "radius": radius, "angular_speed": rng.range(-3.0, 3.0), "phase": rng.range(0.0, std::f32::consts::TAU) }),
            );
            components.insert(
                "Transform".to_owned(),
                json!({ "position": [center[0] + radius, center[1], center[2]], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [0.5, 0.5, 0.5] }),
            );
        }
        entities.insert(id, components);
    }
    SceneDoc {
        format: SCENE_FORMAT.to_owned(),
        next_id: SceneId(count as u32 + 1),
        entities,
    }
}

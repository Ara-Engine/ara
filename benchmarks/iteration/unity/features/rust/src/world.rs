//! Real ECS components and a query plan retained across every measured step.
use bevy_ecs::{batching::BatchingStrategy, change_detection::ContiguousMut, prelude::*};

use crate::protocol::Record;

#[derive(Component)]
struct StableId(u32);

#[derive(Component)]
struct Value {
    value: u32,
    updates: u32,
}

macro_rules! tags {
    ($($name:ident),*) => { $(#[derive(Component)] struct $name;)* };
}
tags!(TagA, TagB, TagC, TagD, TagE, TagF);

/// The integer mixer shared by the scalar reference and Burst fixture.
fn mix(mut value: u32) -> u32 {
    value = (value ^ (value >> 16)).wrapping_mul(0x7feb_352d);
    value = (value ^ (value >> 15)).wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

type Filtered = QueryState<(&'static StableId, &'static mut Value), (With<TagA>, Without<TagB>)>;

/// Owns authoritative state and its retained archetype-filter query.
pub struct Fixture {
    world: World,
    query: Filtered,
    count: u32,
    archetypes: u32,
}

impl Fixture {
    /// Group initial IDs by archetype in both engines; initialization is untimed.
    pub fn new(count: u32, archetypes: u32) -> Self {
        let mut world = World::new();
        for mask in 0..archetypes {
            for id in (mask..count).step_by(archetypes as usize) {
                let mut entity = world.spawn((
                    StableId(id),
                    Value {
                        value: mix(id ^ 0x6d2b_79f5),
                        updates: 0,
                    },
                ));
                if mask & 1 != 0 {
                    entity.insert(TagA);
                }
                if mask & 2 != 0 {
                    entity.insert(TagB);
                }
                if mask & 4 != 0 {
                    entity.insert(TagC);
                }
                if mask & 8 != 0 {
                    entity.insert(TagD);
                }
                if mask & 16 != 0 {
                    entity.insert(TagE);
                }
                if mask & 32 != 0 {
                    entity.insert(TagF);
                }
            }
        }
        let query: Filtered = world.query_filtered();
        Self {
            world,
            query,
            count,
            archetypes,
        }
    }

    /// Includes change-tick advancement, cached query dispatch and component writes.
    pub fn step(&mut self, tick: u32, parallel: bool, batch: usize) {
        self.world.increment_change_tick();
        let update = |(ids, values): (&[StableId], ContiguousMut<Value>)| {
            for (id, value) in ids.iter().zip(ContiguousMut::split(values).0) {
                value.value = value
                    .value
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223 ^ mix(id.0.wrapping_add(tick)));
                value.updates += 1;
            }
        };
        if parallel {
            self.query
                .contiguous_par_iter_mut(&mut self.world)
                .expect("table components and archetype filters support contiguous access")
                .batching_strategy(BatchingStrategy::fixed(batch))
                .for_each(update);
        } else {
            self.query
                .contiguous_iter_mut(&mut self.world)
                .expect("table components and archetype filters support contiguous access")
                .for_each(update);
        }
    }

    /// Checks actual tags, unique IDs, selected updates and all live records.
    pub fn records(
        &mut self,
        steps: u32,
    ) -> std::result::Result<Vec<Record>, Box<dyn std::error::Error>> {
        let mut records = vec![Record::default(); self.count as usize];
        let mut seen = vec![false; self.count as usize];
        let mut query = self.world.query::<(Entity, &StableId, &Value)>();
        for (entity, id, value) in query.iter(&self.world) {
            let index = id.0 as usize;
            if index >= records.len() || seen[index] {
                return Err("duplicate or out-of-range stable ID".into());
            }
            seen[index] = true;
            let tags = u32::from(self.world.get::<TagA>(entity).is_some())
                | (u32::from(self.world.get::<TagB>(entity).is_some()) << 1)
                | (u32::from(self.world.get::<TagC>(entity).is_some()) << 2)
                | (u32::from(self.world.get::<TagD>(entity).is_some()) << 3)
                | (u32::from(self.world.get::<TagE>(entity).is_some()) << 4)
                | (u32::from(self.world.get::<TagF>(entity).is_some()) << 5);
            let expected_updates = if tags & 1 != 0 && tags & 2 == 0 {
                steps
            } else {
                0
            };
            if tags != (id.0 & (self.archetypes - 1)) || value.updates != expected_updates {
                return Err(
                    "tag presence or update count differs from the feature contract".into(),
                );
            }
            records[index] = Record([id.0, tags, value.value, 0, 0, 0, 0, value.updates, 0]);
        }
        if seen.contains(&false) {
            return Err("missing stable ID or required component".into());
        }
        Ok(records)
    }

    /// Uses the retained query; this count is collected outside timing.
    pub fn matched_count(&mut self) -> usize {
        self.query.iter(&self.world).count()
    }
}

//! Binds authoritative lifetimes to their local presentation entities.
use ara_components::Transform;
use ara_net::{NetId, Spawn};
use ara_sim::{Identity, Session};
use bevy_ecs::entity::Entity;
use std::collections::BTreeMap;

/// Streamed rows can arrive after a baseline or leave while it remains relevant.
/// Re-resolve their projection handles; only runtime-created rows get fallbacks.
pub(super) fn reconcile(
    session: &mut Session,
    spawns: &mut [Spawn],
    entities: &mut BTreeMap<NetId, Entity>,
) {
    spawns.sort_unstable_by_key(|spawn| spawn.id);
    entities.retain(|id, _| spawns.binary_search_by_key(id, |spawn| spawn.id).is_ok());
    for spawn in spawns {
        let entity = if spawn.projected {
            session.projection().entity(&spawn.identity)
        } else {
            Some(
                entities
                    .get(&spawn.id)
                    .copied()
                    .filter(|entity| session.world().get_entity(*entity).is_ok())
                    .or_else(|| session.entity(&spawn.identity))
                    .unwrap_or_else(|| {
                        session
                            .world_mut()
                            .spawn((Identity(spawn.identity.clone()), Transform::default()))
                            .id()
                    }),
            )
        };
        match entity {
            Some(entity) => {
                entities.insert(spawn.id, entity);
            }
            None => {
                entities.remove(&spawn.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_doc::{AssetId, Document};
    use ara_sim::{EditWorld, SimRegistry, StateHash, TickConfig};
    use serde_json::json;

    #[test]
    #[ignore = "focused replica binding profiling; run explicitly"]
    fn profile_resident_replica_bindings() -> Result<(), ara_sim::SimError> {
        let mut schema = ara_schema::SchemaDb::new();
        ara_doc::register(&mut schema);
        ara_components::register(&mut schema);
        let registry = SimRegistry::new(schema)?;
        let mut session = Session::local(
            EditWorld::new(
                registry.clone(),
                Document::new("scene", AssetId::parse("sc_bindingbench").unwrap()),
            )?,
            TickConfig::default(),
            1,
            Default::default(),
        )?;
        let mut spawns = Vec::new();
        for slot in 0..128 {
            let identity = format!("e_binding{slot}");
            session.mirror_streaming(|boundary| {
                boundary
                    .spawn(registry.prepare_stream_entity(identity.clone(), Default::default())?)
            })?;
            spawns.push(Spawn {
                id: NetId { slot, epoch: 0 },
                identity: identity.clone(),
                projected: true,
                prefab: identity,
                version: [0; 32],
                owner: None,
                team: None,
            });
        }
        let mut bindings = BTreeMap::new();
        reconcile(&mut session, &mut spawns, &mut bindings);
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            reconcile(
                &mut session,
                std::hint::black_box(&mut spawns),
                std::hint::black_box(&mut bindings),
            );
        }
        println!(
            "128 resident bindings: {:.3} us/frame",
            start.elapsed().as_secs_f64() * 100.0
        );
        Ok(())
    }

    #[test]
    fn replica_and_local_streaming_share_one_identity_in_either_arrival_order()
    -> Result<(), ara_sim::SimError> {
        for replication_first in [true, false] {
            let mut schema = ara_schema::SchemaDb::new();
            ara_doc::register(&mut schema);
            ara_components::register(&mut schema);
            let registry = SimRegistry::new(schema)?;
            let scene = Document::new("scene", AssetId::parse("sc_mirrorstream").unwrap());
            let mut session = Session::local(
                EditWorld::new(registry.clone(), scene)?,
                TickConfig::default(),
                1,
                Default::default(),
            )?;
            let prepare = || {
                registry.prepare_stream_entity(
                    "e_c10rock2".into(),
                    serde_json::from_value(json!({
                        "name":"Streamed rock",
                        "Transform":{"position":[36,2,14]}
                    }))
                    .unwrap(),
                )
            };
            let mut spawn = Spawn {
                id: NetId { slot: 1, epoch: 0 },
                identity: "e_c10rock2".into(),
                projected: true,
                prefab: "e_c10rock2".into(),
                version: [0; 32],
                owner: None,
                team: None,
            };
            let mut bindings = BTreeMap::new();
            if replication_first {
                reconcile(
                    &mut session,
                    std::slice::from_mut(&mut spawn),
                    &mut bindings,
                );
            }
            session.mirror_streaming(|boundary| boundary.spawn(prepare()?))?;
            reconcile(
                &mut session,
                std::slice::from_mut(&mut spawn),
                &mut bindings,
            );
            // The same strict snapshot identity check used by player captures.
            let build = StateHash::of_bytes(b"replica-streaming");
            session.snapshot(build)?;
            let first = bindings[&spawn.id];
            assert_eq!(session.projection().entity(&spawn.identity), Some(first));
            assert_eq!(
                session.world().get::<Transform>(first).unwrap().position,
                [36.0, 2.0, 14.0]
            );

            // The authoritative baseline can outlive local cell residency.
            session.mirror_streaming(|boundary| boundary.despawn(&spawn.identity))?;
            reconcile(
                &mut session,
                std::slice::from_mut(&mut spawn),
                &mut bindings,
            );
            assert!(!bindings.contains_key(&spawn.id));
            assert!(session.entity(&spawn.identity).is_none());
            session.mirror_streaming(|boundary| boundary.spawn(prepare()?))?;
            reconcile(
                &mut session,
                std::slice::from_mut(&mut spawn),
                &mut bindings,
            );
            assert_ne!(bindings[&spawn.id], first);
            assert_eq!(
                session.projection().entity(&spawn.identity),
                Some(bindings[&spawn.id])
            );
            session.snapshot(build)?;
            // C# and other runtime-created entities still receive a fallback,
            // reused without resetting its component state on later baselines.
            let mut dynamic = Spawn {
                id: NetId { slot: 2, epoch: 0 },
                identity: "e_supply".into(),
                projected: false,
                ..spawn
            };
            reconcile(
                &mut session,
                std::slice::from_mut(&mut dynamic),
                &mut bindings,
            );
            let handle = bindings[&dynamic.id];
            session
                .world_mut()
                .get_mut::<Transform>(handle)
                .unwrap()
                .position[0] = 9.0;
            reconcile(
                &mut session,
                std::slice::from_mut(&mut dynamic),
                &mut bindings,
            );
            assert_eq!(bindings[&dynamic.id], handle);
            assert_eq!(
                session.world().get::<Transform>(handle).unwrap().position[0],
                9.0
            );
            session.snapshot(build)?;
        }
        Ok(())
    }
}

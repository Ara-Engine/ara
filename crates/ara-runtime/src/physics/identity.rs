//! Deterministic automatic identities for projected and streamed physics actors.
use crate::Result;
use ara_physics::RigidBody;
use bevy_ecs::prelude::*;
/// Stable identities are assigned before binding, with explicit positive IDs reserved.
pub(super) fn assign_ids(world: &mut World) -> Result<()> {
    let mut query = world.query_filtered::<(
        Entity,
        Option<&ara_physics::PhysicsId>,
        Option<&ara_sim::Identity>,
    ), (
        Or<(
            With<RigidBody>,
            With<ara_physics::CharacterController>,
            With<ara_physics::Vehicle>,
            With<ara_physics::PhysicsId>,
        )>,
        Or<(
            Without<ara_physics::PhysicsId>,
            Changed<ara_physics::PhysicsId>,
        )>,
    )>();
    let mut wanted = Vec::new();
    for (entity, id, identity) in query.iter(world) {
        if id.is_none_or(|id| id.value == 0) {
            wanted.push((
                identity.map_or_else(|| format!("entity:{}", entity.to_bits()), |id| id.0.clone()),
                entity,
            ));
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    wanted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut ids = std::collections::BTreeSet::new();
    let mut query = world.query::<&ara_physics::PhysicsId>();
    for id in query.iter(world) {
        if id.value != 0 {
            ids.insert(id.value);
        }
    }
    for (identity, entity) in wanted {
        let hash = identity.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        let mut id = hash.max(1);
        while !ids.insert(id) {
            id = id.wrapping_add(1).max(1);
        }
        world
            .entity_mut(entity)
            .insert(ara_physics::PhysicsId { value: id });
    }
    Ok(())
}

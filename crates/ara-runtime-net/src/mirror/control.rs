//! View/control identity is derived from received authoritative spawn ownership.
use super::*;
pub(super) fn adopt(
    world: &mut bevy_ecs::world::World,
    spawns: &[ara_net::Spawn],
    entities: &BTreeMap<NetId, Entity>,
    player: PlayerId,
    controlled: &mut Option<String>,
) {
    for spawn in spawns {
        if let Some(&entity) = entities.get(&spawn.id)
            && let Some(mut control) = world.get_mut::<PlayerControl>(entity)
        {
            let owner = spawn.owner.map_or(0, |owner| owner.0);
            if control.player != owner {
                control.player = owner;
            }
        }
    }
    let actor = spawns
        .iter()
        .find(|spawn| spawn.owner == Some(player) && entities.contains_key(&spawn.id))
        .map(|spawn| spawn.identity.as_str());
    if actor == controlled.as_deref() {
        return;
    }
    *controlled = actor.map(str::to_owned);
    for mut camera in world.query::<&mut FollowCamera>().iter_mut(world) {
        if camera.player == player.0 {
            camera.target = actor.map(|actor| EntityRef(actor.into()));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_metadata_retargets_only_the_receiving_players_camera_and_control_ids() {
        let mut world = bevy_ecs::world::World::new();
        let a = world
            .spawn(PlayerControl {
                player: 1,
                ..Default::default()
            })
            .id();
        let b = world
            .spawn(PlayerControl {
                player: 0,
                ..Default::default()
            })
            .id();
        let camera = world
            .spawn(FollowCamera {
                player: 1,
                target: Some(EntityRef("e_first".into())),
                ..Default::default()
            })
            .id();
        let other = world
            .spawn(FollowCamera {
                player: 2,
                target: Some(EntityRef("e_other".into())),
                ..Default::default()
            })
            .id();
        let ids = [NetId { slot: 1, epoch: 0 }, NetId { slot: 2, epoch: 0 }];
        let entities = BTreeMap::from([(ids[0], a), (ids[1], b)]);
        let spawn = |id, identity: &str, owner| ara_net::Spawn {
            id,
            identity: identity.into(),
            projected: true,
            prefab: format!("@play#{identity}"),
            version: [0; 32],
            owner,
            team: None,
        };
        let mut controlled = Some("e_first".into());
        adopt(
            &mut world,
            &[
                spawn(ids[0], "e_first", None),
                spawn(ids[1], "e_second", Some(PlayerId(1))),
            ],
            &entities,
            PlayerId(1),
            &mut controlled,
        );
        assert_eq!(
            world.get::<PlayerControl>(a).map(|control| control.player),
            Some(0)
        );
        assert_eq!(
            world.get::<PlayerControl>(b).map(|control| control.player),
            Some(1)
        );
        assert_eq!(
            world
                .get::<FollowCamera>(camera)
                .and_then(|camera| camera.target.as_ref())
                .map(|target| target.0.as_str()),
            Some("e_second")
        );
        assert_eq!(
            world
                .get::<FollowCamera>(other)
                .and_then(|camera| camera.target.as_ref())
                .map(|target| target.0.as_str()),
            Some("e_other")
        );
    }
}

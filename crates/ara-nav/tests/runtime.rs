#![allow(clippy::unwrap_used)]
mod common;
use ara_nav::*;
use bevy_ecs::{prelude::*, schedule::Schedule};
use common::*;

fn world(mesh: NavMesh, reverse: bool, count: u64) -> (World, Schedule) {
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default()).unwrap();
    world.resource_mut::<NavWorld>().stage(mesh);
    let mut ids: Vec<_> = (0..count).collect();
    if reverse {
        ids.reverse();
    }
    for id in ids {
        let x = 3.0 + (id % 25) as f32 * 1.2;
        let z = 3.0 + (id / 25) as f32 * 1.2;
        let mut state = NavAgentState::new(id);
        state.target = Some([x + 10.0, 0.1, z + 3.0]);
        state.status = NavStatus::Moving;
        world.spawn((NavAgent::default(), NavPosition([x, 0.1, z]), state));
    }
    (world, schedule)
}
fn snapshot(world: &mut World) -> Vec<(u64, [f32; 3], NavAgentState)> {
    let mut result: Vec<_> = world
        .query::<(&NavPosition, &NavAgentState)>()
        .iter(world)
        .map(|(p, s)| (s.id, p.0, s.clone()))
        .collect();
    result.sort_by_key(|(id, _, _)| *id);
    result
}
#[test]
fn crowd_500_replays_bit_exact_across_ecs_order_and_snapshot_restore() {
    let (_, mesh) = bake(&plane(64.0));
    let (mut a, mut sa) = world(mesh.clone(), false, 500);
    let (mut b, mut sb) = world(mesh, true, 500);
    for _ in 0..20 {
        sa.run(&mut a);
        sb.run(&mut b);
    }
    assert!(
        a.resource::<NavWorld>().errors().is_empty(),
        "{:?}",
        a.resource::<NavWorld>().errors()
    );
    let before = snapshot(&mut a);
    assert_eq!(before, snapshot(&mut b));
    assert!(before[0].1[0] > 3.1);
    assert!(
        before
            .iter()
            .all(|(_, p, _)| p.iter().all(|v| v.is_finite()))
    );
    for _ in 0..10 {
        sa.run(&mut a);
    }
    let after = snapshot(&mut a);
    let mut q = b.query::<(&mut NavPosition, &mut NavAgentState)>();
    for (mut p, mut s) in q.iter_mut(&mut b) {
        let (_, pos, state) = before.iter().find(|(id, _, _)| *id == s.id).unwrap();
        p.0 = *pos;
        *s = state.clone();
    }
    for _ in 0..10 {
        sb.run(&mut b);
    }
    assert_eq!(after, snapshot(&mut b));
    // Neighbor avoidance must change at least some desired straight-line velocities.
    assert!(after.iter().any(|(_, _, s)| s.velocity[2].abs() > 0.1));
}
#[test]
fn goto_arrival_authority_swap_policy_and_duplicate_ids() {
    let (_, mesh) = bake(&plane(16.0));
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default()).unwrap();
    let actor = world
        .spawn((
            NavAgent::default(),
            NavPosition([3.0, 0.1, 3.0]),
            NavAgentState::new(1),
        ))
        .id();
    let goal = world.spawn(NavPosition([5.0, 0.1, 3.0])).id();
    world.resource_mut::<NavWorld>().allow_mesh_swap(false);
    world.resource_mut::<NavWorld>().stage(mesh);
    goto(&mut world, actor, GotoTarget::Entity(goal)).unwrap();
    schedule.run(&mut world);
    assert!(world.resource::<NavWorld>().mesh(0).is_none());
    world.resource_mut::<NavWorld>().allow_mesh_swap(true);
    goto(&mut world, actor, GotoTarget::Entity(goal)).unwrap();
    for _ in 0..240 {
        schedule.run(&mut world);
    }
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
    world
        .resource_mut::<NavWorld>()
        .set_authority(Authority::Client);
    let before = *world.get::<NavPosition>(actor).unwrap();
    assert!(goto(&mut world, actor, GotoTarget::Point([8.0, 0.1, 3.0])).is_err());
    schedule.run(&mut world);
    assert_eq!(before, *world.get::<NavPosition>(actor).unwrap());
    world
        .resource_mut::<NavWorld>()
        .set_authority(Authority::Server);
    world.spawn(NavAgentState::new(1));
    schedule.run(&mut world);
    assert_eq!(
        world.resource::<NavWorld>().errors()[0].1.kind,
        "duplicateAgentId"
    );
}
#[test]
fn crowd_follows_gap_link_without_hidden_native_traversal_state() {
    let (_, mesh) = bake(&level());
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default()).unwrap();
    world.resource_mut::<NavWorld>().stage(mesh);
    let actor = world
        .spawn((
            NavAgent::default(),
            NavPosition([44.0, 3.7, 6.0]),
            NavAgentState::new(1),
        ))
        .id();
    goto(&mut world, actor, GotoTarget::Point([54.0, 3.7, 6.0])).unwrap();
    let mut traversed = false;
    for _ in 0..420 {
        schedule.run(&mut world);
        traversed |= world
            .get::<NavAgentState>(actor)
            .unwrap()
            .traversal
            .is_some();
    }
    assert!(traversed);
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
}

#[test]
fn counterflow_agents_clear_shared_obstacle_corners_and_arrive() {
    let mut input = plane(24.0);
    input.obstacles.push(Obstacle {
        id: 1,
        bounds: Bounds {
            min: [10.0, -1.0, 6.0],
            max: [14.0, 3.0, 18.0],
        },
        area: Obstacle::CARVE,
    });
    let (_, mesh) = bake(&input);
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default()).unwrap();
    world.resource_mut::<NavWorld>().stage(mesh);
    for id in 0..40 {
        let x = if id < 20 { 3.0 } else { 21.0 };
        let z = 4.0 + (id % 20) as f32 * 0.75;
        let entity = world
            .spawn((
                NavAgent::default(),
                NavPosition([x, 0.1, z]),
                NavAgentState::new(id),
            ))
            .id();
        goto(&mut world, entity, GotoTarget::Point([24.0 - x, 0.1, z])).unwrap();
    }
    let mut reached = std::collections::BTreeSet::new();
    for _ in 0..1800 {
        schedule.run(&mut world);
        reached.extend(
            world
                .query::<&NavAgentState>()
                .iter(&world)
                .filter(|s| s.status == NavStatus::Arrived)
                .map(|s| s.id),
        );
    }
    assert!(world.resource::<NavWorld>().errors().is_empty());
    assert_eq!(reached.len(), 40);
}

#[test]
fn published_cutout_invalidates_paths_and_invalid_agent_does_not_block_neighbors() {
    let mut input = plane(16.0);
    let baker = Baker::new(None);
    let initial = baker.bake(&input, None).unwrap();
    let mesh = NavMesh::from_artifact(&initial).unwrap();
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default()).unwrap();
    world.resource_mut::<NavWorld>().stage(mesh);
    let actor = world
        .spawn((
            NavAgent::default(),
            NavPosition([3.0, 0.1, 3.0]),
            NavAgentState::new(1),
        ))
        .id();
    let outside = world
        .spawn((
            NavAgent::default(),
            NavPosition([99.0, 0.1, 99.0]),
            NavAgentState::new(2),
        ))
        .id();
    goto(&mut world, actor, GotoTarget::Point([13.0, 0.1, 3.0])).unwrap();
    schedule.run(&mut world);
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Moving
    );
    assert_eq!(
        world.get::<NavAgentState>(outside).unwrap().status,
        NavStatus::Unreachable
    );
    input.obstacles.push(Obstacle {
        id: 1,
        bounds: Bounds {
            min: [7.0, -1.0, 0.0],
            max: [9.0, 3.0, 16.0],
        },
        area: Obstacle::CARVE,
    });
    let changed = baker.bake(&input, Some(&initial)).unwrap();
    world
        .resource_mut::<NavWorld>()
        .stage(NavMesh::from_artifact(&changed).unwrap());
    schedule.run(&mut world);
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Unreachable
    );
}

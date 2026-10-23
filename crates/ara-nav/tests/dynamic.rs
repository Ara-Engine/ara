#![allow(clippy::unwrap_used)]
//! Dynamic world: typed links and gates, obstacles and agent types.
mod common;
use ara_nav::*;
use bevy_ecs::{prelude::*, schedule::Schedule};
use common::*;

fn sim(config: NavConfig) -> (World, Schedule) {
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, config).unwrap();
    (world, schedule)
}

fn agent(world: &mut World, id: u64, at: [f32; 3]) -> Entity {
    world
        .spawn((NavAgent::default(), NavPosition(at), NavAgentState::new(id)))
        .id()
}

fn run_until_arrived(
    world: &mut World,
    schedule: &mut Schedule,
    actor: Entity,
    ticks: usize,
) -> usize {
    for tick in 0..ticks {
        schedule.run(world);
        if world.get::<NavAgentState>(actor).unwrap().status == NavStatus::Arrived {
            return tick;
        }
    }
    panic!(
        "not arrived: {:?} at {:?}",
        world.get::<NavAgentState>(actor).unwrap().issue,
        world.get::<NavPosition>(actor).unwrap()
    );
}

/// Two platforms split by a 2 m trench, joined by a far bridge and a door link.
fn trench(kind: LinkKind) -> BakeInput {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 20.0], [0.0, 44.0], 0.0, 0);
    floor(&mut g, [22.0, 40.0], [0.0, 44.0], 0.0, 0);
    floor(&mut g, [20.0, 22.0], [40.0, 44.0], 0.0, 0);
    let mut request = input(
        g,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [40.0, 2.0, 44.0],
        },
    );
    request.links.push(OffMeshLink {
        id: 7,
        start: [19.0, 0.0, 6.0],
        end: [23.0, 0.0, 6.0],
        radius: 0.8,
        area: 0,
        bidirectional: true,
        enabled: true,
        kind,
        duration: 0.0,
        arc_height: 1.0,
    });
    request
}

#[test]
fn door_waits_for_opening_then_traverses_with_typed_hook() {
    let (_, mesh) = bake(&trench(LinkKind::Door));
    let (mut world, mut schedule) = sim(NavConfig::default());
    world.resource_mut::<NavWorld>().stage(mesh);
    let door = world
        .spawn(NavLinkState {
            id: 7,
            open: false,
            ..Default::default()
        })
        .id();
    let actor = agent(&mut world, 1, [10.0, 0.1, 6.0]);
    goto(&mut world, actor, GotoTarget::Point([30.0, 0.1, 6.0])).unwrap();
    for _ in 0..240 {
        schedule.run(&mut world);
    }
    let state = world.get::<NavAgentState>(actor).unwrap();
    assert_eq!(state.waiting.map(|w| w.0), Some(7));
    assert_eq!(state.issue, Some(NavIssue::WaitingForLink { link: 7 }));
    assert_eq!(world.get::<NavLinkState>(door).unwrap().waiting, 1);
    world.get_mut::<NavLinkState>(door).unwrap().open = true;
    let mut kinds = Vec::new();
    for _ in 0..600 {
        schedule.run(&mut world);
        let state = world.get::<NavAgentState>(actor).unwrap();
        if let Some(t) = state.traversal {
            kinds.push((t.kind, t.link));
        }
        if state.status == NavStatus::Arrived {
            break;
        }
    }
    assert!(kinds.contains(&(LinkKind::Door, 7)), "{kinds:?}");
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
    assert_eq!(world.get::<NavLinkState>(door).unwrap().waiting, 0);
}

#[test]
fn door_timeout_replans_around_and_locked_links_are_never_planned() {
    let (_, mesh) = bake(&trench(LinkKind::Door));
    let (mut world, mut schedule) = sim(NavConfig {
        link_wait_ticks: 30,
        ..Default::default()
    });
    world.resource_mut::<NavWorld>().stage(mesh.clone());
    world.spawn(NavLinkState {
        id: 7,
        open: false,
        ..Default::default()
    });
    let actor = agent(&mut world, 1, [10.0, 0.1, 6.0]);
    goto(&mut world, actor, GotoTarget::Point([30.0, 0.1, 6.0])).unwrap();
    let mut timed_out = false;
    for _ in 0..3000 {
        schedule.run(&mut world);
        let state = world.get::<NavAgentState>(actor).unwrap();
        timed_out |= state.issue == Some(NavIssue::LinkTimeout { link: 7 });
        if state.status == NavStatus::Arrived {
            break;
        }
    }
    assert!(timed_out);
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
    // Went around via the far bridge.
    assert!(world.get::<NavAgentState>(actor).unwrap().avoid_link == Some(7));

    // A locked jump with no alternative route: planning fails immediately.
    let mut input = trench(LinkKind::Jump);
    input.geometry = Geometry::default();
    floor(&mut input.geometry, [0.0, 20.0], [0.0, 12.0], 0.0, 0);
    floor(&mut input.geometry, [22.0, 40.0], [0.0, 12.0], 0.0, 0);
    let (_, mesh) = bake(&input);
    let (mut world, mut schedule) = sim(NavConfig::default());
    world.resource_mut::<NavWorld>().stage(mesh);
    let lock = world
        .spawn(NavLinkState {
            id: 7,
            locked: true,
            ..Default::default()
        })
        .id();
    let actor = agent(&mut world, 1, [10.0, 0.1, 6.0]);
    goto(&mut world, actor, GotoTarget::Point([30.0, 0.1, 6.0])).unwrap();
    schedule.run(&mut world);
    let state = world.get::<NavAgentState>(actor).unwrap();
    assert_eq!(state.status, NavStatus::Unreachable);
    assert!(matches!(state.issue, Some(NavIssue::NoPath { .. })));
    // The explanation names the locked link instead of "missing link".
    let mesh = world.resource::<NavWorld>().mesh(0).unwrap().clone();
    let why = why_unreachable_blocked(&mesh, [10.0, 0.1, 6.0], [30.0, 0.1, 6.0], &QueryFilter::default(), &[7]).unwrap();
    assert_eq!(why.reason, ReachabilityReason::ClosedLink { id: 7 });
    // Unlocked: a jump follows a ballistic arc above the straight line.
    world.get_mut::<NavLinkState>(lock).unwrap().locked = false;
    goto(&mut world, actor, GotoTarget::Point([30.0, 0.1, 6.0])).unwrap();
    let mut apex: f32 = 0.0;
    for _ in 0..600 {
        schedule.run(&mut world);
        let state = world.get::<NavAgentState>(actor).unwrap();
        if let Some(t) = state.traversal {
            assert_eq!(t.kind, LinkKind::Jump);
            apex = apex.max(world.get::<NavPosition>(actor).unwrap().0[1]);
        }
        if state.status == NavStatus::Arrived {
            break;
        }
    }
    assert!(apex > 0.6, "jump apex {apex}");
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
}

#[test]
fn carved_obstacle_publishes_after_fixed_latency_and_is_avoided() {
    let input = plane(32.0);
    let artifact = Baker::new(None).bake(&input, None).unwrap();
    let latency = 6;
    let (mut world, mut schedule) = sim(NavConfig {
        rebake_latency_ticks: latency,
        ..Default::default()
    });
    world
        .resource_mut::<NavWorld>()
        .set_bake_source(input, artifact, None)
        .unwrap();
    let before = world.resource::<NavWorld>().mesh(0).unwrap().content_hash();
    let actor = agent(&mut world, 1, [4.0, 0.1, 16.0]);
    goto(&mut world, actor, GotoTarget::Point([28.0, 0.1, 16.0])).unwrap();
    schedule.run(&mut world);
    // A 2 m x 20 m wall across the straight line.
    world.spawn((
        NavObstacle {
            id: 5,
            half_extents: [1.0, 1.5, 10.0],
            mode: ObstacleMode::Carve,
            area: 0,
        },
        NavPosition([16.0, 1.0, 16.0]),
    ));
    let mut published = None;
    for tick in 0..900 {
        schedule.run(&mut world);
        let hash = world.resource::<NavWorld>().mesh(0).unwrap().content_hash();
        if published.is_none() && hash != before {
            published = Some(tick);
        }
        let p = world.get::<NavPosition>(actor).unwrap().0;
        assert!(
            !((15.0..17.0).contains(&p[0]) && (6.0..26.0).contains(&p[2])),
            "entered obstacle at {p:?}"
        );
        if world.get::<NavAgentState>(actor).unwrap().status == NavStatus::Arrived {
            break;
        }
    }
    // Detected on the first tick after spawning; published `latency` ticks later.
    assert_eq!(published, Some(latency as usize));
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
    let checkpoint = world.resource::<NavWorld>().rebake_checkpoint();
    assert_eq!(checkpoint.live[&0].len(), 1);
    assert!(checkpoint.pending.is_empty());
}

#[test]
fn avoid_only_obstacle_never_rebakes_and_area_costs_follow_agent_types() {
    let (_, mesh) = bake(&plane(32.0));
    let hash = mesh.content_hash();
    let (mut world, mut schedule) = sim(NavConfig::default());
    world.resource_mut::<NavWorld>().stage(mesh);
    world.spawn((
        NavObstacle {
            id: 1,
            half_extents: [1.5, 1.0, 1.5],
            mode: ObstacleMode::Avoid,
            area: 0,
        },
        NavPosition([16.0, 1.0, 16.0]),
    ));
    let actor = agent(&mut world, 1, [4.0, 0.1, 16.0]);
    goto(&mut world, actor, GotoTarget::Point([28.0, 0.1, 16.0])).unwrap();
    for _ in 0..900 {
        schedule.run(&mut world);
        let p = world.get::<NavPosition>(actor).unwrap().0;
        assert!(
            (p[0] - 16.0).abs().max((p[2] - 16.0).abs()) > 1.4,
            "entered avoid-only obstacle at {p:?}"
        );
        if world.get::<NavAgentState>(actor).unwrap().status == NavStatus::Arrived {
            break;
        }
    }
    assert_eq!(
        world.get::<NavAgentState>(actor).unwrap().status,
        NavStatus::Arrived
    );
    assert_eq!(
        world.resource::<NavWorld>().mesh(0).unwrap().content_hash(),
        hash
    );

    // Area obstacle (area 3) strip; the type makes area 3 fifty times as costly.
    let mut input = plane(40.0);
    input.obstacles.push(Obstacle {
        id: 2,
        bounds: Bounds {
            min: [18.0, -1.0, 0.0],
            max: [22.0, 2.0, 32.0],
        },
        area: 3,
    });
    let (_, mesh) = bake(&input);
    let crossing = |world: &mut World, typed: bool| {
        let mut schedule = Schedule::default();
        register_sim(world, &mut schedule, NavConfig::default()).unwrap();
        world.resource_mut::<NavWorld>().stage(mesh.clone());
        if typed {
            let mut costs = vec![1.0; 16];
            costs[3] = 50.0;
            world.spawn(NavAgentType {
                profile: 0,
                name: "Careful".into(),
                area_costs: costs,
                ..Default::default()
            });
        }
        let actor = agent(world, 1, [4.0, 0.1, 10.0]);
        goto(world, actor, GotoTarget::Point([36.0, 0.1, 10.0])).unwrap();
        schedule.run(world);
        let path = world
            .get::<NavAgentState>(actor)
            .unwrap()
            .path
            .clone()
            .unwrap();
        path.corners
            .iter()
            .map(|c| c.position[2])
            .fold(0.0f32, f32::max)
    };
    let straight = crossing(&mut World::new(), false);
    let careful = crossing(&mut World::new(), true);
    assert!(
        straight < 12.0,
        "default filter crosses directly: {straight}"
    );
    assert!(
        careful > 31.0,
        "typed agent detours around the strip: {careful}"
    );
}

#[test]
fn detour_solver_still_supported() {
    let (_, mesh) = bake(&trench(LinkKind::Walk));
    let (mut world, mut schedule) = sim(NavConfig {
        crowd: CrowdSolver::Detour,
        ..Default::default()
    });
    world.resource_mut::<NavWorld>().stage(mesh);
    let actor = agent(&mut world, 1, [10.0, 0.1, 6.0]);
    goto(&mut world, actor, GotoTarget::Point([30.0, 0.1, 6.0])).unwrap();
    run_until_arrived(&mut world, &mut schedule, actor, 900);
}

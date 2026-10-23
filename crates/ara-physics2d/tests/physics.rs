#![allow(clippy::unwrap_used)]
use ara_physics2d::{Body2d, BodyKind2d, Character2d, Collider2d, Physics2d};
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, Identity, Session, SimRegistry, TickConfig, bevy_ecs::prelude::*};
use serde_json::json;

fn world(reverse: bool) -> World {
    let mut world = World::new();
    let mut bodies = vec![(
        "e_floor".to_owned(),
        Body2d {
            position: [0., -0.5],
            ..Default::default()
        },
        Collider2d {
            half_extents: [20., 0.5],
            ..Default::default()
        },
    )];
    for i in 0..128 {
        bodies.push((
            format!("e_b{i:04}"),
            Body2d {
                kind: BodyKind2d::Dynamic,
                position: [(i % 16) as f32 * 1.1 - 8., (i / 16) as f32 * 1.1 + 1.],
                ..Default::default()
            },
            Collider2d::default(),
        ));
    }
    if reverse {
        bodies.reverse();
    }
    for (id, body, collider) in bodies {
        world.spawn((Identity(id), body, collider));
    }
    world
}
fn state(world: &mut World) -> Vec<(String, Body2d)> {
    let mut q = world.query::<(&Identity, &Body2d)>();
    let mut result: Vec<_> = q.iter(world).map(|(id, b)| (id.0.clone(), *b)).collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
#[test]
fn solver_repeats_across_ecs_insertion_orders_and_full_restore() {
    let mut a = world(false);
    let mut b = world(true);
    let mut pa = Physics2d::new(60, [0., -9.81]).unwrap();
    let mut pb = Physics2d::new(60, [0., -9.81]).unwrap();
    for _ in 0..90 {
        pa.step(&mut a, 1. / 60.).unwrap();
        pb.step(&mut b, 1. / 60.).unwrap();
    }
    assert_eq!(pa.snapshot().unwrap(), pb.snapshot().unwrap());
    assert_eq!(state(&mut a), state(&mut b));
    let checkpoint = pa.snapshot().unwrap();
    let components = state(&mut a);
    for _ in 0..60 {
        pa.step(&mut a, 1. / 60.).unwrap();
    }
    let future = pa.snapshot().unwrap();
    let future_bodies = state(&mut a);
    pa.restore(&checkpoint).unwrap();
    for (id, body) in components {
        let mut q = a.query::<(&Identity, &mut Body2d)>();
        for (identity, mut current) in q.iter_mut(&mut a) {
            if identity.0 == id {
                *current = body;
            }
        }
    }
    for _ in 0..60 {
        pa.step(&mut a, 1. / 60.).unwrap();
    }
    assert_eq!(future, pa.snapshot().unwrap());
    assert_eq!(future_bodies, state(&mut a));
    let before = pa.snapshot().unwrap();
    assert!(pa.restore(b"invalid").is_err());
    assert_eq!(before, pa.snapshot().unwrap());
}
#[test]
fn character_grounding_and_ccd_stop_at_solid_geometry() {
    let mut world = World::new();
    world.spawn((
        Identity("e_floor".into()),
        Body2d {
            position: [0., -0.5],
            ..Default::default()
        },
        Collider2d {
            half_extents: [20., 0.5],
            ..Default::default()
        },
    ));
    world.spawn((
        Identity("e_wall".into()),
        Body2d {
            position: [2., 2.],
            ..Default::default()
        },
        Collider2d {
            half_extents: [0.05, 2.],
            ..Default::default()
        },
    ));
    let character = world
        .spawn((
            Identity("e_player".into()),
            Body2d {
                kind: BodyKind2d::Kinematic,
                position: [0., 0.52],
                lock_rotation: true,
                ..Default::default()
            },
            Collider2d::default(),
            Character2d::default(),
        ))
        .id();
    let bullet = world
        .spawn((
            Identity("e_bullet".into()),
            Body2d {
                kind: BodyKind2d::Dynamic,
                position: [0., 2.],
                velocity: [500., 0.],
                ccd: true,
                ..Default::default()
            },
            Collider2d {
                half_extents: [0.05; 2],
                ..Default::default()
            },
        ))
        .id();
    let mut physics = Physics2d::new(60, [0., -9.81]).unwrap();
    for _ in 0..30 {
        world.get_mut::<Character2d>(character).unwrap().translation = [0.1, -0.05];
        physics.step(&mut world, 1. / 60.).unwrap();
    }
    assert!(world.get::<Character2d>(character).unwrap().grounded);
    assert!(world.get::<Body2d>(character).unwrap().position[0] < 1.51);
    assert!(world.get::<Body2d>(bullet).unwrap().position[0] < 2.);
}
#[test]
fn simulation_checkpoints_restore_solver_and_component_state_together() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_physics2d::register(&mut db);
    let mut registry = SimRegistry::new(db.clone()).unwrap();
    ara_physics2d::register_sim(&mut registry).unwrap();
    let value = json!({"$schema":"ara://schema/scene/1","id":"sc_physics1","records":{"e_floor":{"Body2d":{"position":[0,-0.5]},"Collider2d":{"halfExtents":[10,0.5]}},"e_ball":{"Body2d":{"kind":"dynamic","position":[0,3]},"Collider2d":{}}}});
    let source = serde_json::to_string(&value).unwrap();
    let read = ara_doc::read(&source, "physics.scene", &db, None);
    assert!(!read.diags.iter().any(|d| d.is_error()), "{:?}", read.diags);
    let edit = EditWorld::new(registry, read.doc.unwrap()).unwrap();
    let mut session = Session::local(edit, TickConfig::default(), 1, Default::default()).unwrap();
    session
        .world_mut()
        .insert_resource(Physics2d::new(60, [0., -9.81]).unwrap());
    session
        .add_system("physics2d", ara_physics2d::step_system)
        .unwrap();
    for _ in 0..30 {
        session.step().unwrap();
    }
    assert!(session.world().resource::<Physics2d>().error().is_none());
    let build = ara_sim::StateHash::of_bytes(b"physics2d integration test");
    let checkpoint = session.snapshot(build).unwrap();
    for _ in 0..40 {
        session.step().unwrap();
    }
    let expected = session.state_hash().unwrap();
    session.restore(&checkpoint, build).unwrap();
    for _ in 0..40 {
        session.step().unwrap();
    }
    assert_eq!(expected, session.state_hash().unwrap());
}

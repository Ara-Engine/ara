#![allow(clippy::unwrap_used)]
use ara_physics2d::*;
use ara_schema::{Schema, SchemaDb, Validator};
use ara_sim::{EditWorld, Identity, Session, SimRegistry, TickConfig, bevy_ecs::prelude::*};
use serde::Serialize;
use serde_json::json;

fn spawn(world: &mut World, id: &str, motion: MotionType2d, at: [f32; 2], c: Collider2d) -> Entity {
    world
        .spawn((
            Identity(id.into()),
            RigidBody2d {
                motion,
                ..Default::default()
            },
            c,
            PhysicsPose2d {
                position: at,
                rotation: 0.,
            },
        ))
        .id()
}
fn slab(half: [f32; 2]) -> Collider2d {
    Collider2d {
        half_extents: half,
        ..Default::default()
    }
}
fn physics(world: &mut World) {
    world.insert_resource(Physics2d::new(Physics2dConfig::default()).unwrap());
}
fn tick(world: &mut World, ticks: u32) {
    for _ in 0..ticks {
        tick_ecs(world).unwrap();
    }
}
fn stack(reverse: bool) -> World {
    let mut world = World::new();
    let mut ids: Vec<_> = (0..128).collect();
    if reverse {
        ids.reverse();
    }
    for i in ids {
        let at = [(i % 16) as f32 * 1.1 - 8., (i / 16) as f32 * 1.1 + 1.];
        spawn(
            &mut world,
            &format!("e_b{i:04}"),
            MotionType2d::Dynamic,
            at,
            Collider2d::default(),
        );
    }
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([20., 0.5]),
    );
    physics(&mut world);
    world
}
fn poses(world: &mut World) -> Vec<(String, PhysicsPose2d, Velocity2d)> {
    let mut q = world.query::<(&Identity, &PhysicsPose2d, &Velocity2d)>();
    let mut result: Vec<_> = q
        .iter(world)
        .map(|(id, p, v)| (id.0.clone(), *p, *v))
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
fn snapshot(world: &World) -> Vec<u8> {
    world.resource::<Physics2d>().snapshot().unwrap()
}

#[test]
fn solver_repeats_across_insertion_orders_and_restores_completely() {
    let (mut a, mut b) = (stack(false), stack(true));
    tick(&mut a, 90);
    tick(&mut b, 90);
    assert_eq!(snapshot(&a), snapshot(&b));
    assert_eq!(poses(&mut a), poses(&mut b));
    let checkpoint = (snapshot(&a), poses(&mut a));
    tick(&mut a, 60);
    let future = (snapshot(&a), poses(&mut a));
    a.resource_mut::<Physics2d>()
        .restore(&checkpoint.0)
        .unwrap();
    let mut q = a.query::<(&Identity, &mut PhysicsPose2d, &mut Velocity2d)>();
    for (id, mut pose, mut velocity) in q.iter_mut(&mut a) {
        let (_, p, v) = checkpoint.1.iter().find(|row| row.0 == id.0).unwrap();
        (*pose, *velocity) = (*p, *v);
    }
    tick(&mut a, 60);
    assert_eq!(future, (snapshot(&a), poses(&mut a)));
    let before = snapshot(&a);
    assert!(a.resource_mut::<Physics2d>().restore(b"invalid").is_err());
    let other = Physics2d::new(Physics2dConfig {
        tick_hz: 30,
        ..Default::default()
    })
    .unwrap();
    assert!(
        a.resource_mut::<Physics2d>()
            .restore(&other.snapshot().unwrap())
            .is_err()
    );
    assert_eq!(before, snapshot(&a));
}

#[test]
fn character_walks_jumps_lands_and_is_blocked() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([20., 0.5]),
    );
    spawn(
        &mut world,
        "e_wall",
        MotionType2d::Static,
        [3., 2.],
        slab([0.1, 2.]),
    );
    let hero = world
        .spawn((
            Identity("e_hero".into()),
            CharacterController2d::default(),
            PhysicsPose2d {
                position: [0., 1.],
                rotation: 0.,
            },
            CharacterInput2d {
                desired_velocity: [4., 0.],
                jump_speed: 0.,
            },
        ))
        .id();
    physics(&mut world);
    tick(&mut world, 120);
    let motion = *world.get::<CharacterMotion2d>(hero).unwrap();
    let pose = *world.get::<PhysicsPose2d>(hero).unwrap();
    assert!(motion.grounded, "settled on the floor");
    assert!((motion.ground_normal[1] - 1.).abs() < 1e-3);
    assert!(
        (pose.position[1] - 0.9).abs() < 0.05,
        "capsule centre rests at half height"
    );
    assert!(
        pose.position[0] < 3. - 0.1 - 0.3 + 0.02,
        "wall blocks the capsule"
    );
    world.get_mut::<CharacterInput2d>(hero).unwrap().jump_speed = 6.;
    tick(&mut world, 1);
    world.get_mut::<CharacterInput2d>(hero).unwrap().jump_speed = 0.;
    let launch = world.get::<Velocity2d>(hero).unwrap().linear[1];
    assert!(launch > 5., "jump launched at {launch}");
    tick(&mut world, 15);
    assert!(!world.get::<CharacterMotion2d>(hero).unwrap().grounded);
    assert!(world.get::<PhysicsPose2d>(hero).unwrap().position[1] > 2.);
    tick(&mut world, 90);
    assert!(
        world.get::<CharacterMotion2d>(hero).unwrap().grounded,
        "landed"
    );
}

#[test]
fn character_climbs_steps_and_ccd_stops_fast_bodies() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([20., 0.5]),
    );
    spawn(
        &mut world,
        "e_step",
        MotionType2d::Static,
        [2.5, 0.1],
        slab([1., 0.1]),
    );
    spawn(
        &mut world,
        "e_wall",
        MotionType2d::Static,
        [6., 2.],
        slab([0.05, 2.]),
    );
    let bullet = spawn(
        &mut world,
        "e_bullet",
        MotionType2d::Dynamic,
        [4., 3.],
        Collider2d {
            half_extents: [0.05; 2],
            ..Default::default()
        },
    );
    world.entity_mut(bullet).insert((
        RigidBody2d {
            motion: MotionType2d::Dynamic,
            continuous: true,
            ..Default::default()
        },
        Velocity2d {
            linear: [600., 0.],
            angular: 0.,
        },
    ));
    let hero = world
        .spawn((
            Identity("e_hero".into()),
            CharacterController2d::default(),
            PhysicsPose2d {
                position: [0., 0.92],
                rotation: 0.,
            },
            CharacterInput2d {
                desired_velocity: [3., 0.],
                jump_speed: 0.,
            },
        ))
        .id();
    physics(&mut world);
    tick(&mut world, 60);
    let pose = world.get::<PhysicsPose2d>(hero).unwrap();
    assert!(pose.position[0] > 2., "walked onto the step");
    assert!(pose.position[1] > 1.05, "stepped up 0.2 m");
    assert!(world.get::<PhysicsPose2d>(bullet).unwrap().position[0] < 6.);
}

#[test]
fn layers_filter_collisions_and_sensors_report_overlaps() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([20., 0.5]),
    );
    let ghost = spawn(
        &mut world,
        "e_ghost",
        MotionType2d::Dynamic,
        [-2., 1.],
        Collider2d {
            layer: 2,
            ..Default::default()
        },
    );
    let crate_ = spawn(
        &mut world,
        "e_crate",
        MotionType2d::Dynamic,
        [2., 1.],
        Collider2d::default(),
    );
    spawn(
        &mut world,
        "e_coin",
        MotionType2d::Static,
        [2., 0.5],
        Collider2d {
            sensor: true,
            ..slab([0.3, 0.3])
        },
    );
    let mut config = Physics2dConfig::default();
    config.layer_masks[2] &= !1;
    config.layer_masks[0] &= !(1 << 2);
    world.insert_resource(Physics2d::new(config).unwrap());
    tick(&mut world, 60);
    assert!(
        world.get::<PhysicsPose2d>(ghost).unwrap().position[1] < -1.,
        "fell through"
    );
    assert!((world.get::<PhysicsPose2d>(crate_).unwrap().position[1] - 0.5).abs() < 0.05);
    let physics = world.resource::<Physics2d>();
    assert_eq!(physics.touching("e_coin"), ["e_crate"]);
    assert_eq!(physics.touching("e_crate"), ["e_coin", "e_floor"]);
    let hit = physics
        .raycast([2., 5.], [0., -1.], 10., false)
        .unwrap()
        .unwrap();
    assert_eq!(hit.entity, "e_crate");
    assert!(hit.normal[0].abs() < 1e-6 && (hit.normal[1] - 1.).abs() < 1e-6);
    let mut asymmetric = Physics2dConfig::default();
    asymmetric.layer_masks[3] = 0;
    assert!(Physics2d::new(asymmetric).is_err());
}

#[test]
fn invalid_extractions_fault_before_mutating_the_solver() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([20., 0.5]),
    );
    let bad = world
        .spawn((Identity("e_bad".into()), RigidBody2d::default()))
        .id();
    physics(&mut world);
    assert!(tick_ecs(&mut world).is_err());
    assert!(world.resource::<Physics2d>().is_empty());
    world
        .entity_mut(bad)
        .insert(CharacterController2d::default());
    world.entity_mut(bad).insert(Collider2d::default());
    assert!(
        tick_ecs(&mut world).is_err(),
        "body and character are exclusive"
    );
    world.entity_mut(bad).remove::<(RigidBody2d, Collider2d)>();
    world.insert_resource(Physics2dStatus::default());
    world.entity_mut(bad).insert(CharacterInput2d {
        desired_velocity: [0., 3.],
        jump_speed: 0.,
    });
    step_system(&mut world);
    assert!(
        world.resource::<Physics2dStatus>().error.is_some(),
        "Y input under gravity"
    );
}

#[test]
fn session_checkpoints_restore_solver_and_components_together() {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    register(&mut db);
    let mut registry = SimRegistry::new(db.clone()).unwrap();
    register_sim(&mut registry).unwrap();
    let value = json!({"$schema":"ara://schema/scene/1","id":"sc_physics1","records":{
        "e_floor":{"PhysicsPose2d":{"position":[0,-0.5]},"RigidBody2d":{},"Collider2d":{"halfExtents":[10,0.5]}},
        "e_ball":{"PhysicsPose2d":{"position":[0,3]},"RigidBody2d":{"motion":"dynamic"},"Collider2d":{"kind":"circle"}},
        "e_hero":{"PhysicsPose2d":{"position":[-3,2]},"CharacterController2d":{},"CharacterInput2d":{"desiredVelocity":[1,0]}}}});
    let read = ara_doc::read(
        &serde_json::to_string(&value).unwrap(),
        "physics.scene",
        &db,
        None,
    );
    assert!(!read.diags.iter().any(|d| d.is_error()), "{:?}", read.diags);
    let edit = EditWorld::new(registry, read.doc.unwrap()).unwrap();
    let mut session = Session::local(edit, TickConfig::default(), 1, Default::default()).unwrap();
    install(&mut session, Physics2dConfig::default()).unwrap();
    assert!(install(&mut session, Physics2dConfig::default()).is_err());
    for _ in 0..30 {
        session.step().unwrap();
    }
    assert!(
        session
            .world()
            .resource::<Physics2dStatus>()
            .error
            .is_none()
    );
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

fn defaults<T: Schema + Serialize + serde::de::DeserializeOwned>(db: &SchemaDb, value: T) {
    let data = serde_json::to_value(value).unwrap();
    let mut diagnostics = Vec::new();
    Validator::new(db, &mut diagnostics).value(
        &ara_schema::ValueType::Named(T::describe().name),
        None,
        &data,
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let schema: T = serde_json::from_value(serde_json::Value::Object(
        T::describe()
            .fields()
            .iter()
            .map(|f| (f.name.to_owned(), db.field_default(f)))
            .collect(),
    ))
    .unwrap();
    assert_eq!(
        data,
        serde_json::to_value(schema).unwrap(),
        "{}",
        T::describe().name
    );
}
#[test]
fn schemas_verify_and_defaults_match_serde() {
    let mut db = SchemaDb::new();
    register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    defaults(&db, PhysicsPose2d::default());
    defaults(&db, Velocity2d::default());
    defaults(&db, RigidBody2d::default());
    defaults(&db, Collider2d::default());
    defaults(&db, CharacterController2d::default());
    defaults(&db, CharacterInput2d::default());
    defaults(&db, CharacterMotion2d::default());
}

#[test]
fn respawns_and_new_ground_are_visible_to_characters_on_the_same_tick() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([5., 0.5]),
    );
    let hero = world
        .spawn((
            Identity("e_hero".into()),
            CharacterController2d::default(),
            PhysicsPose2d {
                position: [0., 0.92],
                rotation: 0.,
            },
        ))
        .id();
    physics(&mut world);
    tick(&mut world, 10);
    assert!(world.get::<CharacterMotion2d>(hero).unwrap().grounded);
    spawn(
        &mut world,
        "e_island",
        MotionType2d::Static,
        [40., -0.5],
        slab([2., 0.5]),
    );
    world.get_mut::<PhysicsPose2d>(hero).unwrap().position = [40., 0.92];
    tick(&mut world, 1);
    let pose = world.get::<PhysicsPose2d>(hero).unwrap().position;
    assert!(
        (pose[0] - 40.).abs() < 1e-4,
        "teleported, not swept: {pose:?}"
    );
    assert!(
        world.get::<CharacterMotion2d>(hero).unwrap().grounded,
        "new ground seen"
    );
}

#[test]
fn rotated_resting_bodies_fall_asleep_through_the_ecs_round_trip() {
    let mut world = World::new();
    spawn(
        &mut world,
        "e_floor",
        MotionType2d::Static,
        [0., -0.5],
        slab([30., 0.5]),
    );
    // Many angles: atan2/sin_cos round trips are not bit-exact for all of them.
    for i in 0..32 {
        let at = [i as f32 * 1.5 - 24., 1.];
        let rotation = 0.05 + i as f32 * 0.097;
        let e = spawn(
            &mut world,
            &format!("e_tilt{i:02}"),
            MotionType2d::Dynamic,
            at,
            Collider2d::default(),
        );
        world.entity_mut(e).insert((
            RigidBody2d {
                motion: MotionType2d::Dynamic,
                lock_rotation: true,
                ..Default::default()
            },
            PhysicsPose2d {
                position: at,
                rotation,
            },
        ));
    }
    world.insert_resource(Physics2d::new(Physics2dConfig::default()).unwrap());
    tick(&mut world, 400);
    let physics = world.resource::<Physics2d>();
    let awake: Vec<_> = (0..32)
        .filter(|i| physics.sleeping(&format!("e_tilt{i:02}")) != Some(true))
        .collect();
    assert!(
        awake.is_empty(),
        "kept awake by the ECS round trip: {awake:?}"
    );
}

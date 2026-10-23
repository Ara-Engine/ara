#![allow(clippy::unwrap_used)]
use ara_components::Transform;
use ara_doc::{AssetId, Document, Edit, Workspace};
use ara_input::{Action, ActionKind, ActionMap, ActionValue, Injection, PlayerId};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{Schema, SchemaDb};
use ara_sim::{
    BoundaryCommand, Clock, EditWorld, FixedTick, Identity, PlayControl, SceneMetadata, Session,
    SimContext, SimError, SimRegistry, StateHash, StepInput, TickConfig, Watch,
};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Restorable service cursor; gameplay remains in components.
#[derive(Resource, Schema, Serialize, Deserialize)]
struct Cursor {
    /// Last boundary serviced.
    value: u64,
}

/// An explicitly ordered system's state.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
struct Counter {
    /// Accumulated tick value.
    value: u64,
}

/// A typed asynchronous result/intent.
#[derive(Schema, Deserialize)]
#[ara(params)]
struct SetPosition {
    /// Requested x coordinate.
    x: f32,
}

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    assert!(db.add_json_types(&json!({"$schema":"ara://schema/types/1", "types": {"Stats": {"kind":"component", "doc":"Runtime authored fields.", "fields": {
        "hp":{"type":"i32", "doc":"Hit points.", "default":100},
        "title":{"type":"string", "doc":"Display title."},
        "items":{"type":"list<u64>", "doc":"Stable item identities."},
        "direction":{"type":"vec3", "doc":"A vector."},
        "enabled":{"type":"bool", "doc":"Active flag."}
    }}}})).is_empty());
    db
}

fn scene(db: &SchemaDb) -> Document {
    let read = ara_doc::read(
        r#"{
        "$schema":"ara://schema/scene/1", "id":"sc_12345678", "records": {
          "e_0001":{"name":"Player", "order":"a0", "Transform":{}, "Stats":{"title":"hello", "items":[1,9]}},
          "e_0002":{"parent":"e_0001", "order":"a1", "Transform":{"position":[0,1,0]}}
        }
    }"#,
        "test.scene",
        db,
        None,
    );
    assert!(read.diags.iter().all(|d| !d.is_error()), "{:?}", read.diags);
    read.doc.unwrap()
}

fn actions() -> ActionMap {
    let mut map = ActionMap::default();
    map.declare(Action {
        name: "Move".into(),
        kind: ActionKind::Axis2,
    })
    .unwrap();
    map.declare(Action {
        name: "Jump".into(),
        kind: ActionKind::Button,
    })
    .unwrap();
    map
}

fn movement(
    mut context: ResMut<SimContext>,
    mut query: Query<(&Identity, &mut Transform)>,
    mut cursor: ResMut<Cursor>,
) {
    let movement = context.action(PlayerId(1), "Move");
    let jump = context.action(PlayerId(1), "Jump");
    for (id, mut t) in &mut query {
        if let Some(ActionValue::Axis2([x, z])) = movement {
            t.position[0] += x;
            t.position[2] += z;
        }
        if jump == Some(ActionValue::Button(true)) {
            t.position[1] += 1.0;
        }
        t.position[1] += context.rng("movement", &id.0).unit_f32() * 0.01;
    }
    cursor.value = context.tick();
}

fn control(seed: u64) -> PlayControl {
    let db = schema();
    let doc = scene(&db);
    let mut registry = SimRegistry::new(db).unwrap();
    registry.register_resource::<Cursor>().unwrap();
    registry.register_component::<Counter>().unwrap();
    registry
        .register_command::<SetPosition>("setPosition", |world, command, params| {
            let mut query = world.query::<(&Identity, &mut Transform)>();
            let mut found = false;
            for (id, mut t) in query.iter_mut(world) {
                if command.target.as_deref() == Some(id.0.as_str()) {
                    t.position[0] = params.x;
                    found = true;
                }
            }
            if !found {
                return Err(SimError("missing command target".into()));
            }
            Ok(())
        })
        .unwrap();
    let edit = EditWorld::new(registry, doc).unwrap();
    let mut session = Session::local(edit, TickConfig::default(), seed, actions()).unwrap();
    session.world_mut().insert_resource(Cursor { value: 0 });
    session.add_system("movement", movement).unwrap();
    let mut control = PlayControl::new(session, StateHash::of_bytes(b"test build"));
    control.start();
    control
}

fn move_input(offset: u32, hold_ticks: u32) -> StepInput {
    StepInput {
        after_ticks: offset,
        input: Injection {
            player: PlayerId(1),
            action: "Move".into(),
            value: ActionValue::Axis2([1., 0.]),
            hold_ticks,
        },
    }
}

#[test]
fn projection_round_trip_and_dynamic_storage() {
    let db = schema();
    let source = scene(&db);
    let registry = SimRegistry::new(db.clone()).unwrap();
    let layout = registry.dynamic_layout("Stats").unwrap();
    assert!(
        layout
            .fields()
            .windows(2)
            .all(|f| f[0].offset < f[1].offset)
    );
    let world = EditWorld::new(registry, source.clone()).unwrap();
    let parent = world.entity("e_0001").unwrap();
    let child = world.entity("e_0002").unwrap();
    assert_eq!(
        world.world().get::<ChildOf>(child).unwrap().parent(),
        parent
    );
    assert_eq!(world.children(Some("e_0001")), ["e_0002"]);
    assert_eq!(
        world.component("e_0001", "Stats").unwrap().unwrap()["hp"],
        100
    );
    assert_eq!(
        world.component("e_0001", "Stats").unwrap().unwrap()["items"],
        json!([1, 9])
    );
    assert_eq!(
        ara_doc::write(&world.to_document().unwrap(), &db),
        ara_doc::write(&source, &db)
    );
}

#[test]
fn committed_ops_patch_existing_handles_and_inverse_restores() {
    let db = schema();
    let source = scene(&db);
    let id = source.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("test.scene", source.clone());
    let mut store = Store::new(db.clone(), workspace);
    let mut edit = EditWorld::new(SimRegistry::new(db.clone()).unwrap(), source.clone()).unwrap();
    let old = edit.entity("e_0001").unwrap();
    store
        .transact(
            TxnMeta::by(Author::Human),
            vec![
                Edit::Set {
                    doc: id.clone(),
                    id: ara_doc::RecordId::parse("e_0001").unwrap(),
                    path: "Transform.position.x".parse().unwrap(),
                    value: Some(json!(3)),
                },
                Edit::Set {
                    doc: id.clone(),
                    id: ara_doc::RecordId::parse("e_0001").unwrap(),
                    path: "Stats.title".parse().unwrap(),
                    value: Some(json!("changed")),
                },
                Edit::Move {
                    doc: id.clone(),
                    id: ara_doc::RecordId::parse("e_0002").unwrap(),
                    parent: None,
                    order: Some(ara_doc::OrderKey::parse("a2").unwrap()),
                },
                Edit::Create {
                    doc: id.clone(),
                    id: ara_doc::RecordId::parse("e_0003").unwrap(),
                    record: json!({"Transform":{}}).as_object().unwrap().clone(),
                },
            ],
        )
        .unwrap();
    let txn = store.history().last().unwrap();
    edit.apply_transaction(txn).unwrap();
    assert_eq!(edit.entity("e_0001"), Some(old));
    assert_eq!(edit.world().get::<Transform>(old).unwrap().position[0], 3.);
    assert_eq!(
        edit.component("e_0001", "Stats").unwrap().unwrap()["title"],
        "changed"
    );
    assert!(
        edit.world()
            .get::<ChildOf>(edit.entity("e_0002").unwrap())
            .is_none()
    );
    assert!(edit.entity("e_0003").is_some());
    edit.apply_ops(
        &txn.ops
            .iter()
            .rev()
            .map(|op| op.inverse())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(
        ara_doc::write(&edit.to_document().unwrap(), &db),
        ara_doc::write(&source, &db)
    );
    assert_eq!(edit.entity("e_0001"), Some(old));
    assert!(edit.apply_transaction(txn).is_ok());
    assert!(edit.apply_transaction(txn).is_err());
}

#[test]
fn seed_inputs_snapshot_restore_and_replay_are_identical() {
    let mut a = control(7);
    let mut b = control(7);
    let mut other = control(8);
    a.step(10, &[move_input(0, 8)]).unwrap();
    b.step(10, &[move_input(0, 8)]).unwrap();
    other.step(10, &[move_input(0, 8)]).unwrap();
    assert_eq!(
        a.session().state_hash().unwrap(),
        b.session().state_hash().unwrap()
    );
    assert_ne!(
        a.session().state_hash().unwrap(),
        other.session().state_hash().unwrap()
    );
    let checkpoint = a.snapshot().unwrap();
    let checkpoint = serde_json::from_str(&serde_json::to_string(&checkpoint).unwrap()).unwrap();
    a.record().unwrap();
    a.step(30, &[move_input(2, 15)]).unwrap();
    let expected = a.session().state_hash().unwrap();
    let replay = a.finish_recording().unwrap();
    a.restore(&checkpoint).unwrap();
    a.step(30, &[move_input(2, 15)]).unwrap();
    assert_eq!(a.session().state_hash().unwrap(), expected);
    let replay = ara_sim::Replay::from_bytes(&replay.to_bytes().unwrap()).unwrap();
    assert_eq!(b.replay(&replay).unwrap(), expected);
    let mut corrupt = replay.clone();
    corrupt.ticks[2].hash = Some(StateHash::default());
    assert!(b.replay(&corrupt).unwrap_err().0.contains("tick 13"));
}

#[test]
fn pause_timescale_watch_until_and_input_expiration() {
    let mut play = control(1);
    play.watch(
        vec![
            Watch::parse("e_0001.Transform.position.x").unwrap(),
            Watch::parse("e_0001.name").unwrap(),
        ],
        2,
    )
    .unwrap();
    play.pause().unwrap();
    assert_eq!(play.advance(Duration::from_secs(1)).unwrap().ticks, 0);
    let report = play.step(5, &[move_input(0, 2)]).unwrap();
    assert_eq!(report.samples.len(), 4);
    assert_eq!(report.samples[0].value, Some(json!(2.0)));
    assert_eq!(report.samples[1].value, Some(json!("Player")));
    let report = play.until(100, |s| s.tick() == 8).unwrap();
    assert_eq!(report.ticks, 3);
    assert!(report.reached);
    assert!(!play.until(3, |_| false).unwrap().reached);
    play.resume().unwrap();
    play.timescale(0.5).unwrap();
    assert_eq!(
        play.advance(Duration::from_secs_f64(1. / 60.))
            .unwrap()
            .ticks,
        0
    );
    assert_eq!(
        play.advance(Duration::from_secs_f64(1. / 60.))
            .unwrap()
            .ticks,
        1
    );
    play.stop();
    assert!(play.step(1, &[]).is_err());
}

#[test]
fn boundary_commands_are_ordered_deduplicated_and_recorded() {
    let mut play = control(4);
    let target = play.session().projection().entity("e_0001").unwrap();
    let command = |sequence, x| BoundaryCommand {
        kind: "setPosition".into(),
        player: PlayerId(1),
        sequence,
        target: Some("e_0001".into()),
        payload: json!({"x":x}),
    };
    play.record().unwrap();
    play.session_mut().submit(1, command(2, 20.)).unwrap();
    play.session_mut().submit(1, command(1, 10.)).unwrap();
    play.session_mut().submit(1, command(2, 99.)).unwrap();
    assert_eq!(
        play.session()
            .world()
            .get::<Transform>(target)
            .unwrap()
            .position[0],
        0.
    );
    play.step(1, &[]).unwrap();
    assert_eq!(
        play.session()
            .world()
            .get::<Transform>(target)
            .unwrap()
            .position[0],
        20.
    );
    let hash = play.session().state_hash().unwrap();
    let replay = play.finish_recording().unwrap();
    assert_eq!(replay.ticks[0].commands.len(), 2);
    assert_eq!(play.replay(&replay).unwrap(), hash);
}

#[test]
fn snapshot_restores_resources_lifecycle_dynamic_fields_and_pending_input() {
    let mut play = control(9);
    play.session_mut()
        .inject(3, vec![move_input(0, 3).input])
        .unwrap();
    let snapshot = play.snapshot().unwrap();
    let entity = play
        .session_mut()
        .world_mut()
        .spawn((
            Identity("e_runtime".into()),
            Counter { value: 42 },
            SceneMetadata::default(),
        ))
        .id();
    play.session_mut()
        .world_mut()
        .resource_mut::<Cursor>()
        .value = 99;
    play.step(8, &[]).unwrap();
    play.restore(&snapshot).unwrap();
    assert!(play.session().world().get_entity(entity).is_err());
    assert_eq!(play.session().world().resource::<Cursor>().value, 0);
    play.step(8, &[]).unwrap();
    let hash = play.session().state_hash().unwrap();
    play.restore(&snapshot).unwrap();
    play.step(8, &[]).unwrap();
    assert_eq!(hash, play.session().state_hash().unwrap());
}

#[test]
fn snapshots_reject_unknown_future_affecting_state() {
    #[derive(Component)]
    struct Unknown;
    #[derive(Resource)]
    struct UnknownResource;
    let mut play = control(1);
    let e = play.session().projection().entity("e_0001").unwrap();
    play.session_mut().world_mut().entity_mut(e).insert(Unknown);
    assert!(
        play.snapshot()
            .unwrap_err()
            .0
            .contains("unregistered simulation component")
    );
    play.session_mut()
        .world_mut()
        .entity_mut(e)
        .remove::<Unknown>();
    play.session_mut()
        .world_mut()
        .insert_resource(UnknownResource);
    assert!(
        play.snapshot()
            .unwrap_err()
            .0
            .contains("unregistered simulation resource")
    );
    play.session_mut()
        .world_mut()
        .remove_resource::<UnknownResource>();
    let resource = play.session().world().resource_entity::<Cursor>().unwrap();
    play.session_mut()
        .world_mut()
        .entity_mut(resource)
        .insert(Unknown);
    assert!(play.snapshot().unwrap_err().0.contains("resource entities"));
    play.session_mut()
        .world_mut()
        .entity_mut(resource)
        .remove::<Unknown>();
    play.session_mut()
        .world_mut()
        .insert_non_send(std::rc::Rc::new(42u32));
    assert!(
        play.snapshot()
            .unwrap_err()
            .0
            .contains("non-send simulation state")
    );
}

#[test]
fn independent_worlds_and_explicit_system_order() {
    let mut a = control(1);
    let b = control(1);
    let entity = a.session().projection().entity("e_0001").unwrap();
    a.session_mut()
        .world_mut()
        .entity_mut(entity)
        .insert(Counter { value: 0 });
    a.session_mut()
        .add_system("first", |mut q: Query<&mut Counter>| {
            for mut c in &mut q {
                c.value += 1;
            }
        })
        .unwrap();
    a.session_mut()
        .add_system("second", |mut q: Query<&mut Counter>| {
            for mut c in &mut q {
                c.value *= 2;
            }
        })
        .unwrap();
    a.step(2, &[]).unwrap();
    assert_eq!(a.session().world().get::<Counter>(entity).unwrap().value, 6);
    assert_eq!(b.session().tick(), 0);
}

#[test]
fn catch_up_is_bounded_and_clock_is_injected() {
    struct Manual(Duration);
    impl Clock for Manual {
        fn now(&self) -> Duration {
            self.0
        }
    }
    let mut fixed = FixedTick::new(TickConfig {
        hz: 60,
        max_catch_up: 4,
    })
    .unwrap();
    assert_eq!(fixed.poll(&Manual(Duration::ZERO)).unwrap().ticks, 0);
    let frame = fixed.poll(&Manual(Duration::from_millis(1010))).unwrap();
    assert_eq!(frame.ticks, 4);
    assert_eq!(frame.dropped_ticks, 56);
    assert!((frame.alpha - 0.6).abs() < 0.00001);
    assert!(fixed.poll(&Manual(Duration::ZERO)).is_err());
    assert!(
        FixedTick::new(TickConfig {
            hz: 0,
            max_catch_up: 4
        })
        .is_err()
    );
    assert!(fixed.set_timescale(f64::NAN).is_err());
}

#[test]
fn prefab_projection_uses_stable_addresses() {
    let db = schema();
    let prefab = ara_doc::read(r#"{"$schema":"ara://schema/prefab/1", "id":"pf_12345678", "records":{"e_0001":{"Transform":{}},"e_0002":{"parent":"e_0001","Stats":{}}}}"#, "thing.prefab", &db, None).doc.unwrap();
    let scene = ara_doc::read(r#"{"$schema":"ara://schema/scene/1", "id":"sc_12345678", "records":{"e_0003":{"prefab":"@thing.prefab#pf_12345678"}}}"#, "scene.scene", &db, None).doc.unwrap();
    let mut workspace = Workspace::default();
    workspace.insert("thing.prefab", prefab);
    workspace.insert("scene.scene", scene);
    let edit = EditWorld::from_workspace(
        SimRegistry::new(db).unwrap(),
        workspace,
        AssetId::parse("sc_12345678").unwrap(),
    )
    .unwrap();
    assert!(edit.entity("e_0003/e_0002").is_some());
}

#[test]
fn dynamic_runtime_values_restore_and_reject_storage_overflow() {
    let mut play = control(12);
    play.session_mut()
        .write_runtime_component(
            "e_0001",
            "Stats",
            &json!({"hp":2.0,"items":[3,5,8],"title":"runtime","direction":[1,2,3],"enabled":true}),
        )
        .unwrap();
    let checkpoint = play.snapshot().unwrap();
    let expected = play.session().state_hash().unwrap();
    play.session_mut()
        .write_runtime_component("e_0001", "Stats", &json!({"title":"new"}))
        .unwrap();
    assert!(
        play.session_mut()
            .write_runtime_component("e_0001", "Stats", &json!({"direction":[1e100,0,0]}))
            .is_err()
    );
    play.restore(&checkpoint).unwrap();
    assert_eq!(play.session().state_hash().unwrap(), expected);
    assert_eq!(
        play.session()
            .component("e_0001", "Stats")
            .unwrap()
            .unwrap()["hp"],
        2
    );
    assert_eq!(
        play.session()
            .component("e_0001", "Stats")
            .unwrap()
            .unwrap()["title"],
        "runtime"
    );
}

#[test]
fn malformed_command_is_rejected_before_a_boundary_and_handler_failure_is_recoverable() {
    let mut play = control(2);
    let checkpoint = play.snapshot().unwrap();
    let mut command = BoundaryCommand {
        kind: "setPosition".into(),
        player: PlayerId(9),
        sequence: 1,
        target: Some("e_9999".into()),
        payload: json!({"x":"bad"}),
    };
    assert!(play.session_mut().submit(1, command.clone()).is_err());
    assert_eq!(play.session().tick(), 0);
    command.payload = json!({"x":2});
    play.session_mut().submit(1, command).unwrap();
    assert!(play.step(1, &[]).is_err());
    assert!(play.step(1, &[]).unwrap_err().0.contains("faulted"));
    play.restore(&checkpoint).unwrap();
    play.step(1, &[]).unwrap();
}

#[test]
fn replay_includes_tick_boundary_authoring_ops() {
    let mut play = control(6);
    let db = schema();
    let source = scene(&db);
    let id = source.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("test.scene", source);
    let mut store = Store::new(db, workspace);
    store
        .transact(
            TxnMeta::by(Author::Human),
            vec![Edit::Set {
                doc: id,
                id: ara_doc::RecordId::parse("e_0001").unwrap(),
                path: "Stats.hp".parse().unwrap(),
                value: Some(json!(27)),
            }],
        )
        .unwrap();
    play.session_mut()
        .patch(2, store.history().last().unwrap())
        .unwrap();
    play.record().unwrap();
    assert_eq!(
        play.session()
            .component("e_0001", "Stats")
            .unwrap()
            .unwrap()["hp"],
        100
    );
    play.step(5, &[]).unwrap();
    let expected = play.session().state_hash().unwrap();
    let replay = play.finish_recording().unwrap();
    assert_eq!(replay.ticks[1].ops.len(), 1);
    let mut granular = control(6);
    granular.restore(&replay.initial).unwrap();
    for record in &replay.ticks {
        granular.session_mut().replay_tick(record).unwrap();
    }
    assert_eq!(granular.session().state_hash().unwrap(), expected);
    assert_eq!(play.replay(&replay).unwrap(), expected);
    assert_eq!(
        play.session()
            .component("e_0001", "Stats")
            .unwrap()
            .unwrap()["hp"],
        27
    );
}

#[test]
fn local_typed_dispatch_only_encodes_when_history_is_requested() {
    use serde::ser::SerializeStruct;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ENCODED: AtomicUsize = AtomicUsize::new(0);
    /// Direct Local command data.
    #[derive(Schema, Deserialize)]
    #[ara(params)]
    struct DirectSet {
        /// Target x coordinate.
        x: f32,
    }
    impl Serialize for DirectSet {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            ENCODED.fetch_add(1, Ordering::Relaxed);
            let mut state = serializer.serialize_struct("DirectSet", 1)?;
            state.serialize_field("x", &self.x)?;
            state.end()
        }
    }
    impl ara_sim::SimCommand for DirectSet {
        const NAME: &'static str = "directSet";
        fn execute(
            self,
            world: &mut World,
            context: &ara_sim::CommandContext,
        ) -> Result<(), SimError> {
            let mut query = world.query::<(&Identity, &mut Transform)>();
            for (id, mut transform) in query.iter_mut(world) {
                if context.target.as_deref() == Some(id.0.as_str()) {
                    transform.position[0] = self.x;
                }
            }
            Ok(())
        }
    }
    let db = schema();
    let doc = scene(&db);
    let mut registry = SimRegistry::new(db).unwrap();
    registry.register_sim_command::<DirectSet>().unwrap();
    let mut session = Session::local(
        EditWorld::new(registry, doc).unwrap(),
        TickConfig::default(),
        1,
        actions(),
    )
    .unwrap();
    let context = |sequence| ara_sim::CommandContext {
        player: PlayerId(1),
        sequence,
        target: Some("e_0001".into()),
    };
    session
        .submit_typed(1, context(1), DirectSet { x: 4. })
        .unwrap();
    session.step().unwrap();
    assert_eq!(ENCODED.load(Ordering::Relaxed), 0);
    assert_eq!(
        session.component("e_0001", "Transform").unwrap().unwrap()["position"][0],
        4.
    );
    let mut play = PlayControl::new(session, StateHash::of_bytes(b"direct build"));
    play.start();
    play.session_mut()
        .submit_typed(2, context(2), DirectSet { x: 7. })
        .unwrap();
    let checkpoint = play.snapshot().unwrap();
    assert_eq!(ENCODED.load(Ordering::Relaxed), 1);
    play.step(1, &[]).unwrap();
    play.restore(&checkpoint).unwrap();
    play.step(1, &[]).unwrap();
    play.record().unwrap();
    play.session_mut()
        .submit_typed(3, context(3), DirectSet { x: 11. })
        .unwrap();
    play.step(1, &[]).unwrap();
    let hash = play.session().state_hash().unwrap();
    let replay = play.finish_recording().unwrap();
    assert_eq!(replay.ticks[0].commands.len(), 1);
    assert_eq!(play.replay(&replay).unwrap(), hash);
}

#[test]
fn deferred_lifecycle_changes_are_visible_only_at_tick_boundaries() {
    let mut play = control(19);
    play.session_mut()
        .add_system(
            "spawn",
            |mut commands: Commands, mut context: ResMut<SimContext>| {
                if context.tick() == 1 {
                    commands.spawn((context.allocate_identity(), Counter { value: 42 }));
                }
            },
        )
        .unwrap();
    play.session_mut()
        .add_system("observe", |mut query: Query<&mut Counter>| {
            for mut counter in &mut query {
                counter.value += 1;
            }
        })
        .unwrap();
    play.step(1, &[]).unwrap();
    let entity = play.session().entity("e_rt0000000000000000").unwrap();
    assert_eq!(
        play.session().world().get::<Counter>(entity).unwrap().value,
        42
    );
    play.step(1, &[]).unwrap();
    assert_eq!(
        play.session().world().get::<Counter>(entity).unwrap().value,
        43
    );
}

#[test]
fn snapshots_preserve_signed_zero_and_hashes_distinguish_it() {
    let mut play = control(23);
    let entity = play.session().projection().entity("e_0001").unwrap();
    play.session_mut()
        .world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .position[0] = -0.0;
    let negative = play.session().state_hash().unwrap();
    let checkpoint = play.snapshot().unwrap();
    let bytes = checkpoint.to_bytes().unwrap();
    let checkpoint = ara_sim::Snapshot::from_bytes(&bytes).unwrap();
    let mut trailing = bytes;
    trailing.push(0);
    assert!(ara_sim::Snapshot::from_bytes(&trailing).is_err());
    play.session_mut()
        .world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .position[0] = 0.0;
    let positive = play.session().state_hash().unwrap();
    play.restore(&checkpoint).unwrap();
    assert_eq!(
        play.session()
            .world()
            .get::<Transform>(entity)
            .unwrap()
            .position[0]
            .to_bits(),
        (-0.0f32).to_bits()
    );
    assert_ne!(negative, positive);
}

#[test]
fn restore_preserves_explicit_source_defaults_used_by_queued_op_preconditions() {
    let mut play = control(31);
    let db = schema();
    let source = scene(&db);
    let id = source.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("test.scene", source);
    let mut store = Store::new(db, workspace);
    let edit = |hp| Edit::Set {
        doc: id.clone(),
        id: ara_doc::RecordId::parse("e_0001").unwrap(),
        path: "Stats.hp".parse().unwrap(),
        value: Some(json!(hp)),
    };
    store
        .transact(TxnMeta::by(Author::Human), vec![edit(100)])
        .unwrap();
    play.session_mut()
        .patch(1, store.history().last().unwrap())
        .unwrap();
    play.step(1, &[]).unwrap();
    store
        .transact(TxnMeta::by(Author::Human), vec![edit(27)])
        .unwrap();
    play.session_mut()
        .patch(3, store.history().last().unwrap())
        .unwrap();
    let checkpoint =
        ara_sim::Snapshot::from_bytes(&play.snapshot().unwrap().to_bytes().unwrap()).unwrap();
    play.step(2, &[]).unwrap();
    let expected = play.session().state_hash().unwrap();
    play.restore(&checkpoint).unwrap();
    play.step(2, &[]).unwrap();
    assert_eq!(play.session().state_hash().unwrap(), expected);
}

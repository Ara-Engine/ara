#![allow(clippy::unwrap_used)]
//! External state providers and parallel system groups.
use ara_input::ActionMap;
use ara_schema::SchemaDb;
use ara_sim::{
    EditWorld, Identity, PlayControl, Session, SimError, SimRegistry, StateHash, StateProvider,
    TickConfig, TrustedState,
};
use bevy_ecs::prelude::*;
use serde_json::{Value, json};
use std::any::TypeId;
use std::sync::Arc;

/// Non-schema runtime value owned by the test provider (like a nav agent state).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct Odometer(u64);

/// Native-like cache that only the provider may rebuild (like a physics world).
#[derive(Resource, Default)]
struct NativeCache {
    rebuilt: u32,
    exact: u32,
}

struct OdometerProvider;
impl StateProvider for OdometerProvider {
    fn name(&self) -> &'static str {
        "odometer"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![TypeId::of::<Odometer>(), TypeId::of::<NativeCache>()]
    }
    fn capture(&self, world: &World) -> Result<Value, SimError> {
        let mut rows: Vec<(String, u64)> = world
            .iter_entities()
            .filter_map(|e| Some((e.get::<Identity>()?.0.clone(), e.get::<Odometer>()?.0)))
            .collect();
        rows.sort();
        Ok(json!(rows))
    }
    fn restore(&self, world: &mut World, state: &Value) -> Result<(), SimError> {
        let rows: Vec<(String, u64)> = serde_json::from_value(state.clone())?;
        let mut query = world.query::<(&Identity, &mut Odometer)>();
        for (id, mut odometer) in query.iter_mut(world) {
            odometer.0 = rows
                .iter()
                .find(|(row, _)| *row == id.0)
                .map_or(0, |(_, v)| *v);
        }
        world.resource_mut::<NativeCache>().rebuilt += 1;
        Ok(())
    }
    fn checkpoint(&self, _: &mut World) -> Result<Option<TrustedState>, SimError> {
        Ok(Some(Arc::new(7u32)))
    }
    fn restore_checkpoint(
        &self,
        world: &mut World,
        state: &Value,
        trusted: &TrustedState,
    ) -> Result<(), SimError> {
        assert_eq!(trusted.downcast_ref::<u32>(), Some(&7));
        self.restore(world, state)?;
        world.resource_mut::<NativeCache>().exact += 1;
        Ok(())
    }
}

fn drive(mut query: Query<&mut Odometer>) {
    for mut odometer in &mut query {
        odometer.0 += 1;
    }
}
fn read_a(_: Query<&Identity>) {}
fn read_b(_: Query<&Identity>) {}
fn write_identity(_: Query<&mut Identity>) {}

fn control() -> PlayControl {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let doc = ara_doc::read(
        r#"{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{"e_0001":{"Transform":{}}}}"#,
        "test.scene",
        &db,
        None,
    )
    .doc
    .unwrap();
    let edit = EditWorld::new(SimRegistry::new(db).unwrap(), doc).unwrap();
    let mut session =
        Session::local(edit, TickConfig::default(), 1, ActionMap::default()).unwrap();
    let entity = session.entity("e_0001").unwrap();
    session.world_mut().entity_mut(entity).insert(Odometer(0));
    session.world_mut().init_resource::<NativeCache>();
    session.add_system("drive", drive).unwrap();
    let mut play = PlayControl::new(session, StateHash::of_bytes(b"providers"));
    play.start();
    play
}

#[test]
fn provider_state_is_snapshotted_hashed_and_restored() {
    let mut play = control();
    assert!(
        play.snapshot().unwrap_err().0.contains("unregistered"),
        "owned types are rejected until a provider claims them"
    );
    play.session_mut().add_provider(OdometerProvider).unwrap();
    assert!(play.session_mut().add_provider(OdometerProvider).is_err());
    play.step(3, &[]).unwrap();
    let portable = ara_sim::Snapshot::from_bytes(&play.snapshot().unwrap().to_bytes().unwrap())
        .unwrap();
    let exact = play.checkpoint().unwrap();
    assert!(exact.is_trusted() && !portable.is_trusted());
    let hash = play.session().state_hash().unwrap();
    play.step(5, &[]).unwrap();
    assert_ne!(play.session().state_hash().unwrap(), hash, "provider state is hashed");
    play.restore(&portable).unwrap();
    assert_eq!(play.session().state_hash().unwrap(), hash);
    play.step(5, &[]).unwrap();
    play.restore(&exact).unwrap();
    assert_eq!(play.session().state_hash().unwrap(), hash);
    let cache = play.session().world().resource::<NativeCache>();
    assert_eq!((cache.rebuilt, cache.exact), (2, 1));
}

#[test]
fn parallel_groups_order_with_neighbours_and_reject_conflicts() {
    let mut play = control();
    let session = play.session_mut();
    session
        .add_parallel_systems(vec![
            ("readA", Box::new(IntoSystem::into_system(read_a))),
            ("readB", Box::new(IntoSystem::into_system(read_b))),
        ])
        .unwrap();
    session.add_system("after", drive).unwrap();
    assert!(
        session
            .add_parallel_systems(vec![
                ("w1", Box::new(IntoSystem::into_system(write_identity))),
                ("r1", Box::new(IntoSystem::into_system(read_a))),
            ])
            .is_err()
    );
}

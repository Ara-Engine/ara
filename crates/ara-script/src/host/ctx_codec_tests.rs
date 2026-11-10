//! The gameplay fast write must agree with the journaled codec path byte for byte.
use super::*;
use crate::ecs::cmdbuf::ScriptCatalog;
use crate::host::registry::{Owner, ScriptComponent};
use bevy_ecs::world::World;
use serde_json::json;
use std::sync::Arc;

fn fixture() -> (Registry, World, Entity) {
    let schema = ara_runtime::schema(&[]).expect("engine schema");
    let mut codecs = ara_sim::SimRegistry::new(schema.clone()).expect("codecs");
    ara_runtime::register_components(&mut codecs).expect("engine codecs");
    let mut registry = Registry::new(schema);
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let mut catalog = ScriptCatalog::default();
    for name in ["Light", "AudioSource", "AudioGraphSource"] {
        let id = codecs.install_component(&mut world, name).expect("install");
        codecs
            .write_component(&mut world, entity, name, id, &json!({}))
            .expect("seed");
        registry.components.push(ScriptComponent {
            name: name.into(),
            id,
            layout: crate::csgen::binding_layout(&registry.schema, name),
            owner: Owner::Engine,
            native: false,
            json: true,
        });
        catalog.components.push((id, 0));
        catalog.names.push(Some(name.into()));
    }
    let codecs = Arc::new(codecs);
    registry.json = Some(codecs.clone());
    catalog.json = Some(codecs);
    world.insert_resource(catalog);
    (registry, world, entity)
}

fn write(
    registry: &mut Registry,
    world: &mut World,
    entity: Entity,
    index: u32,
    payload: &[u8],
    journal: Option<&mut Journal>,
) -> i32 {
    let context = HostCtx::new(Box::new(|_, _| {}));
    let mut scope = Scope {
        world: world.as_unsafe_world_cell(),
        registry,
        journal: journal.map(std::ptr::from_mut),
        thread: std::thread::current().id(),
        register: false,
    };
    context.with_scope(&mut scope, || {
        // SAFETY: the live scope exclusively owns this world; payload is a readable span.
        unsafe {
            host_component_json(
                context.handle(),
                entity.to_bits(),
                index,
                COMPONENT_WRITE,
                payload.as_ptr(),
                payload.len() as u32,
                std::ptr::null_mut(),
                0,
            )
        }
    })
}

fn read(world: &World, entity: Entity, index: u32) -> Option<Vec<u8>> {
    world.resource::<ScriptCatalog>().read(world, entity, index)
}

#[test]
fn gameplay_and_journaled_json_writes_agree_on_values_defaults_errors_and_float_bits() {
    let (mut registry, mut world, entity) = fixture();
    let reference = world.spawn_empty().id();
    for (i, component) in registry.components.iter().enumerate() {
        let initial = read(&world, entity, i as u32).expect("initial");
        world.resource_scope(|world, catalog: bevy_ecs::prelude::Mut<ScriptCatalog>| {
            assert!(catalog.insert(world, reference, i as u32, &initial));
        });
        assert!(world.entity(reference).contains_id(component.id));
    }
    let cases: &[(u32,&[u8],i32)] = &[
        (0,br#"{"intensity":0.1,"color":[0.1,-0.0,1.401298464324817e-45,1]}"#,1),
        (0,br#"{"intensity":1,"intensity":2}"#,1),
        (0,br#"{"intensity":-0.0}"#,1),
        (0,br#"{}"#,1),
        (0,br#"{"intensity":-1}"#,0),
        (0,br#"{"intensity":null}"#,0),
        (0,br#"{"unknown":3}"#,0),
        (0,br#"{"intensity":1e999}"#,0),
        (0,br#"{"intensity":1e39}"#,0),
        (0,b"{",0),
        (1,br#"{"bus":"effects/\"spatial\"","gain":0.1,"position":[-0.0,0.1,1]}"#,1),
        (1,br#"{"gain":17}"#,0),
        (2,br#"{"parameters":{"frequency":0.1,"frequency":0.2,"escaped\"":-0.0},"triggers":{"pulse":7}}"#,1),
        (2,br#"{"parameters":{"frequency":"bad"}}"#,0),
    ];
    for &(index, payload, expected) in cases {
        let before = read(&world, reference, index).expect("preimage");
        let mut journal = Journal::default();
        assert_eq!(
            write(&mut registry, &mut world, entity, index, payload, None),
            expected,
            "{payload:?}"
        );
        assert_eq!(
            write(
                &mut registry,
                &mut world,
                reference,
                index,
                payload,
                Some(&mut journal)
            ),
            expected,
            "{payload:?}"
        );
        assert_eq!(
            read(&world, entity, index),
            read(&world, reference, index),
            "{payload:?}"
        );
        if expected == 1 {
            assert_eq!(journal.before[0].2, before);
        } else {
            assert_eq!(read(&world, reference, index).expect("unchanged"), before);
        }
        if index == 0 && payload == br#"{"intensity":-0.0}"# {
            assert_eq!(
                world
                    .get::<ara_runtime::components::Light>(entity)
                    .expect("typed light")
                    .intensity
                    .to_bits(),
                (-0.0f32).to_bits()
            );
        }
    }
}

#[test]
fn gameplay_json_write_never_inserts_missing_components_or_revives_entities() {
    let (mut registry, mut world, entity) = fixture();
    let missing = world.spawn_empty().id();
    let dead = world.spawn_empty().id();
    world.despawn(dead);
    for target in [missing, dead] {
        assert_eq!(write(&mut registry, &mut world, target, 0, b"{}", None), 0);
        assert!(read(&world, target, 0).is_none());
    }
    assert_eq!(write(&mut registry, &mut world, entity, 99, b"{}", None), 0);
}

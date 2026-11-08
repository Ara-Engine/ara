//! Span queries borrow native table runs and retain row identity and structural ordering.
mod support;

use std::collections::{HashMap, HashSet};

use ara_physics::Velocity;
use ara_script::{EngineBindings, ModuleSpec, ScriptConfig, ScriptHost, bind_component};
use bevy_ecs::prelude::Component;
use serde_json::json;

#[derive(Component)]
struct Dense;

#[derive(Component)]
#[component(storage = "SparseSet")]
struct Sparse;

/// A native Transform prefix stored sparsely, which produces single-row batch views.
#[derive(Component, Clone, Copy, Debug, Default)]
#[component(storage = "SparseSet")]
#[repr(C)]
struct SparseTransform {
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

const GAME: &str = r#"
/// <summary>Authoritative row identity and visit count.</summary>
[Component] public partial struct Visit
{
    /// <summary>Last borrowed entity handle.</summary>
    public ulong Seen;
    /// <summary>Number of fixed ticks processed.</summary>
    public int Count;
    /// <summary>Size of the most recent native run.</summary>
    public int BatchLength;
}
/// <summary>Rows selected for movement.</summary>
[Component] public partial struct Selected { }
/// <summary>Rows excluded from movement.</summary>
[Component] public partial struct Blocked { }
/// <summary>Playback order written through the structural buffer.</summary>
[Component] public partial struct OrderStamp
{
    /// <summary>Last command's sequence number.</summary>
    public int Value;
}
/// <summary>Serial command sequence state, stored in the ECS.</summary>
[Resource] public partial struct Sequence
{
    /// <summary>Next command sequence number.</summary>
    public int Next;
}
/// <summary>Views over both engine and module native columns.</summary>
public static partial class Movement
{
    [Query, With<Selected>, Without<Blocked>]
    static void Run(Span<Transform> transforms, ReadOnlySpan<Velocity> velocities,
        Span<Visit> visits, ReadOnlySpan<Entity> entities, in Tick tick)
    {
        for (var i = 0; i < transforms.Length; i++)
        {
            transforms[i].Position += velocities[i].Linear * tick.Dt;
            visits[i].Seen = entities[i].Bits;
            visits[i].Count++;
            visits[i].BatchLength = transforms.Length;
        }
    }
}

/// <summary>Resource access and structural commands keep batches serial.</summary>
public static partial class Playback
{
    [Query(Order = 10), With<Selected>, Without<Blocked>]
    static void Run(ReadOnlySpan<Visit> visits, ReadOnlySpan<Entity> entities,
        ref Sequence sequence, ref Commands commands)
    {
        for (var i = 0; i < visits.Length; i++)
        {
            commands.Add(entities[i], new OrderStamp { Value = -1 });
            commands.Add(entities[i], new OrderStamp { Value = sequence.Next++ });
        }
    }
}
"#;

#[test]
fn spans_preserve_native_rows_ticks_filters_and_command_order_across_archetypes() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start("batch-queries", &[("Game", GAME, &[])]);
    let mut entities = Vec::new();
    for index in 0..8192 {
        let selected = index % 5 != 0;
        let blocked = index % 7 == 0;
        let mut components = vec![("Visit", json!({"seen": 0, "count": 0, "batchLength": 0}))];
        if selected {
            components.push(("Selected", json!({})));
        }
        if blocked {
            components.push(("Blocked", json!({})));
        }
        let entity = live.spawn(&components);
        let mut entity_mut = live.world.entity_mut(entity);
        entity_mut.insert((
            support::EcsTransform::default(),
            Velocity {
                linear: [2.0, 3.0, 4.0],
                angular: [-0.0, 5.0, 6.0],
            },
        ));
        if index % 2 == 0 {
            entity_mut.insert(Dense);
        }
        if index % 3 == 0 {
            entity_mut.insert(Sparse);
        }
        entities.push((entity, selected && !blocked));
    }
    let ticks = entities
        .iter()
        .map(|(entity, _)| {
            let entity = live.world.entity(*entity);
            (
                entity
                    .get_change_ticks::<support::EcsTransform>()
                    .expect("transform ticks"),
                entity
                    .get_change_ticks::<Velocity>()
                    .expect("velocity ticks"),
            )
        })
        .collect::<Vec<_>>();
    let selected = entities.iter().filter(|(_, include)| *include).count();
    let included = entities
        .iter()
        .filter_map(|(entity, include)| include.then_some(*entity))
        .collect::<HashSet<_>>();
    // Distinguish the first writer's tick from the tick used while spawning rows.
    live.world.increment_change_tick();
    for frame in 1..=3 {
        let serial_order = live
            .world
            .storages()
            .tables
            .iter()
            .flat_map(|table| table.entities().iter().copied())
            .filter(|entity| included.contains(entity))
            .enumerate()
            .map(|(index, entity)| (entity, index))
            .collect::<HashMap<_, _>>();
        live.run();
        assert!(
            live.diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.is_error()),
            "{:?}",
            live.diagnostics
        );
        let mut order = Vec::new();
        for ((entity, include), (transform_ticks, velocity_ticks)) in entities.iter().zip(&ticks) {
            let row = live.world.entity(*entity);
            let transform = row
                .get::<support::EcsTransform>()
                .expect("native transform");
            let final_transform_ticks = row
                .get_change_ticks::<support::EcsTransform>()
                .expect("final transform ticks");
            let final_velocity_ticks = row
                .get_change_ticks::<Velocity>()
                .expect("final velocity ticks");
            assert_eq!(final_transform_ticks.added, transform_ticks.added);
            assert_eq!(final_velocity_ticks.added, velocity_ticks.added);
            assert_eq!(
                final_velocity_ticks.changed, velocity_ticks.changed,
                "readonly span must not stamp changes"
            );
            let visit = live
                .host
                .read_component(&live.world, *entity, "Visit")
                .expect("visit");
            assert_eq!(
                visit["count"].as_u64(),
                Some(if *include { frame } else { 0 })
            );
            assert_eq!(
                visit["seen"].as_u64(),
                Some(if *include { entity.to_bits() } else { 0 })
            );
            if *include {
                let mut expected = [0.0f32; 3];
                for _ in 0..frame {
                    for (position, speed) in expected.iter_mut().zip([2.0f32, 3.0, 4.0]) {
                        *position += speed * (1.0f32 / 60.0);
                    }
                }
                assert_eq!(
                    transform.position.map(f32::to_bits),
                    expected.map(f32::to_bits)
                );
                assert_ne!(final_transform_ticks.changed, transform_ticks.changed);
                let stamp = live.int(*entity, "OrderStamp", "value");
                assert_eq!(
                    stamp,
                    ((frame - 1) as usize * selected + serial_order[entity]) as i64,
                    "serial dense span commands retain table/row traversal order and last-write order"
                );
                order.push(stamp);
            } else {
                assert_eq!(transform.position, [0.0; 3]);
                assert_eq!(final_transform_ticks.changed, transform_ticks.changed);
                assert!(
                    live.host
                        .read_component(&live.world, *entity, "OrderStamp")
                        .is_none()
                );
            }
        }
        order.sort_unstable();
        let start = (frame - 1) as usize * selected;
        assert_eq!(
            order,
            (start..start + selected)
                .map(|n| n as i64)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            live.host
                .read_resource(&live.world, "Sequence")
                .expect("sequence")["next"]
                .as_u64(),
            Some(frame * selected as u64)
        );
    }
    assert_eq!(live.finish(), 0);
}

#[test]
fn sparse_native_columns_supply_single_row_spans_with_matching_identities() {
    let _clr = support::clr_lock();
    let root = support::Live::write("batch-sparse-queries", &[("Game", GAME, &[])]);
    let mut world = bevy_ecs::world::World::new();
    let schema = ara_runtime::schema(&[]).expect("engine schema");
    let bindings = bind_component!(EngineBindings::new(), SparseTransform as "Transform" { position, rotation, scale });
    let mut config = ScriptConfig::jit(support::bin("Ara.Host"));
    config.compile_server = Some(support::bin("Ara.CompileServer").join("Ara.CompileServer.dll"));
    let host =
        ScriptHost::start(config, schema, &bindings, &mut world).expect("sparse native host");
    let mut live = support::Live {
        host,
        world,
        root,
        frame: 0,
        reports: Vec::new(),
        diagnostics: Vec::new(),
    };
    let module = ModuleSpec::from_dir(&live.root.join("Game")).expect("module spec");
    live.host
        .load_modules(&mut live.world, &[module])
        .expect("sparse span module");
    let rows = (0..32)
        .map(|index| {
            let entity = live.spawn(&[
                ("Visit", json!({"seen": 0, "count": 0, "batchLength": 0})),
                ("Selected", json!({})),
            ]);
            let mut row = live.world.entity_mut(entity);
            row.insert(Velocity {
                linear: [2.0, 3.0, 4.0],
                angular: [-0.0, 5.0, 6.0],
            });
            if index % 2 == 0 {
                row.insert(SparseTransform::default());
            }
            entity
        })
        .collect::<Vec<_>>();
    live.run();
    for (index, entity) in rows.into_iter().enumerate() {
        assert_eq!(
            live.int(entity, "Visit", "count"),
            i64::from(index % 2 == 0)
        );
        assert_eq!(
            live.int(entity, "Visit", "batchLength"),
            i64::from(index % 2 == 0)
        );
        let seen = live
            .host
            .read_component(&live.world, entity, "Visit")
            .expect("visit")["seen"]
            .as_u64();
        assert_eq!(
            seen,
            Some(if index % 2 == 0 { entity.to_bits() } else { 0 })
        );
        if let Some(transform) = live.world.get::<SparseTransform>(entity) {
            assert_eq!(
                transform.position.map(f32::to_bits),
                [2.0f32, 3.0, 4.0].map(|v| (v * (1.0f32 / 60.0)).to_bits())
            );
        }
    }
    assert!(
        live.diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0);
}

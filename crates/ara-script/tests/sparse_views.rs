//! Actual CLR systems over sparse native data and sparse filters sharing dense tables.
#[path = "sparse_views/fixture.rs"]
mod fixture;
mod support;

use ara_physics::{PhysicsId, PhysicsPose, VehicleTelemetry, Velocity};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use fixture::{DenseMarker, SparseMarker, SparseTransform};
use serde_json::json;

const GAME: &str = r#"
/// <summary>Moves sparse and dense native columns in the same callback.</summary>
public static partial class Move
{
    [Query(Order = 0)]
    static void Step(ref Transform transform, in Velocity velocity, ref PhysicsPose pose, in Tick tick)
    {
        transform.Position += velocity.Linear * tick.Dt;
        pose.Position = transform.Position;
    }
}
/// <summary>A sparse With filter must not include other entities in the same table.</summary>
public static partial class Included
{
    [Query(Order = 10), With<Transform>]
    static void Step(ref Velocity velocity) => velocity.Angular.X += 1;
}
/// <summary>A sparse Without filter must select the complementary dense rows.</summary>
public static partial class Excluded
{
    [Query(Order = 20), Without<Transform>]
    static void Step(ref Velocity velocity) => velocity.Angular.Y += 1;
}
/// <summary>Reading sparse storage must retain the earlier writer's change tick.</summary>
public static partial class Observe
{
    [Query(Order = 30)]
    static void Step(in Transform transform, ref VehicleTelemetry telemetry) => telemetry.EngineRpm = transform.Position.X;
}
/// <summary>Observed serial traversal order.</summary>
[Cue] public partial struct Visited
{
    /// <summary>Authoritative source row ID.</summary>
    public ulong Value;
}
/// <summary>Structural buffers apply in defined archetype/entity order.</summary>
public static partial class Structural
{
    [Query(Order = 40)]
    static void Step(Entity entity, in Transform transform, in PhysicsId id, in Velocity velocity, in Tick tick, ref Commands commands)
    {
        commands.Cue(new Visited { Value = id.Value });
        if (tick.Frame == 0)
        {
            var created = commands.Spawn();
            commands.Add(created, new PhysicsId { Value = id.Value + 1000000 });
        }
        if (tick.Frame == 1 && id.Value % 7 == 0) commands.Remove<Transform>(entity);
    }
}
/// <summary>Random access and undo remain valid for the same sparse native storage.</summary>
[Command("sparse.move")] public static partial class RandomAccess
{
    /// <summary>Edits the actual sparse prefix through World.Get.</summary>
    /// <param name="actor">The entity to move.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor)
    {
        World.Get<Transform>(actor).Position.X = 123;
        return CommandResult.Ok;
    }
}
"#;

fn spawn(world: &mut World, id: u64) -> Entity {
    let mut entity = world.spawn((
        PhysicsId { value: id },
        PhysicsPose::default(),
        Velocity {
            linear: [1.0, 2.0, 3.0],
            angular: [0.0, 0.0, -0.0],
        },
        VehicleTelemetry::default(),
    ));
    if id.is_multiple_of(2) {
        entity.insert(SparseTransform {
            position: [id as f32, 0.0, 0.0],
            ..Default::default()
        });
    }
    if id.is_multiple_of(3) {
        entity.insert(DenseMarker);
    }
    if id.is_multiple_of(5) {
        entity.insert(SparseMarker);
    }
    entity.id()
}

fn serial_order(world: &World) -> Vec<u64> {
    let transform = world
        .component_id::<SparseTransform>()
        .expect("sparse transform id");
    let id = world.component_id::<PhysicsId>().expect("physics id");
    world
        .archetypes()
        .iter()
        .filter(|a| a.contains(transform) && a.contains(id))
        .flat_map(|a| a.entities().iter())
        .map(|row| world.get::<PhysicsId>(row.id()).expect("source id").value)
        .collect()
}

#[test]
fn actual_sparse_native_prefix_queries_filter_write_and_apply_ordered_structural_commands() {
    let _clr = support::clr_lock();
    let mut live = fixture::start("sparse-native-views", GAME, false);
    let mut sources = (0..20_000)
        .map(|i| spawn(&mut live.world, i))
        .collect::<Vec<_>>();
    for frame in 0..3 {
        if frame == 1 {
            for &entity in sources.iter().step_by(13) {
                live.world.despawn(entity);
            }
            sources.retain(|&e| live.world.get_entity(e).is_ok());
            sources.extend((30_000..31_000).map(|i| spawn(&mut live.world, i)));
            for &entity in sources.iter().step_by(17) {
                live.world.entity_mut(entity).insert(SparseMarker);
            }
        }
        let order = serial_order(&live.world);
        let before = sources
            .iter()
            .map(|&entity| {
                let row = live.world.entity(entity);
                (
                    entity,
                    live.world.get::<SparseTransform>(entity).copied(),
                    *live.world.get::<Velocity>(entity).expect("velocity"),
                    *live.world.get::<PhysicsPose>(entity).expect("pose"),
                    row.get_change_ticks::<PhysicsId>()
                        .expect("readonly ID tick")
                        .changed,
                    row.get_change_ticks::<PhysicsPose>()
                        .expect("pose tick")
                        .changed,
                    row.get_change_ticks::<SparseTransform>()
                        .map(|ticks| ticks.added),
                )
            })
            .collect::<Vec<_>>();
        live.run();
        let visited = live
            .host
            .take_cues(&mut live.world)
            .into_iter()
            .map(|cue| cue.value["value"].as_u64().expect("cue ID"))
            .collect::<Vec<_>>();
        assert_eq!(
            visited, order,
            "serial callback and structural-buffer order is archetype ID then archetype entity order"
        );
        if frame == 0 {
            let mut created = live
                .world
                .query::<(Entity, &PhysicsId)>()
                .iter(&live.world)
                .filter(|(_, id)| id.value >= 1_000_000)
                .map(|(entity, id)| (entity.index().index(), id.value - 1_000_000))
                .collect::<Vec<_>>();
            created.sort_unstable_by_key(|row| row.0);
            assert_eq!(
                created.into_iter().map(|(_, id)| id).collect::<Vec<_>>(),
                order,
                "reserved spawns apply in the same serial order"
            );
        }
        for (entity, transform, velocity, pose, id_tick, pose_tick, sparse_added) in before {
            let row = live.world.entity(entity);
            let after = live.world.get::<Velocity>(entity).expect("velocity");
            assert_eq!(
                after.linear, velocity.linear,
                "readonly dense column is unchanged"
            );
            let mut angular = velocity.angular;
            angular[usize::from(transform.is_none())] += 1.0;
            assert_eq!(
                after.angular.map(f32::to_bits),
                angular.map(f32::to_bits),
                "With/Without select the exact sparse membership on shared tables"
            );
            assert_eq!(
                row.get_change_ticks::<PhysicsId>()
                    .expect("readonly ID")
                    .changed,
                id_tick
            );
            if let Some(mut transform) = transform {
                for axis in 0..3 {
                    transform.position[axis] += velocity.linear[axis] * (1.0f32 / 60.0);
                }
                assert_eq!(
                    live.world
                        .get::<PhysicsPose>(entity)
                        .expect("written dense pose")
                        .position
                        .map(f32::to_bits),
                    transform.position.map(f32::to_bits)
                );
                assert_eq!(
                    live.world
                        .get::<VehicleTelemetry>(entity)
                        .expect("sparse readonly result")
                        .engine_rpm
                        .to_bits(),
                    transform.position[0].to_bits()
                );
                if let Some(after) = live.world.get::<SparseTransform>(entity) {
                    assert_eq!(
                        after.position.map(f32::to_bits),
                        transform.position.map(f32::to_bits)
                    );
                    assert_eq!(
                        row.get_change_ticks::<SparseTransform>()
                            .expect("sparse writer tick")
                            .changed,
                        row.get_change_ticks::<PhysicsPose>()
                            .expect("same-system dense writer tick")
                            .changed,
                        "later readonly sparse callback must not restamp its column"
                    );
                    assert_eq!(
                        row.get_change_ticks::<SparseTransform>()
                            .expect("sparse added tick")
                            .added,
                        sparse_added.expect("component existed before callback")
                    );
                }
            } else {
                assert_eq!(
                    *live
                        .world
                        .get::<PhysicsPose>(entity)
                        .expect("unselected pose"),
                    pose
                );
                assert_eq!(
                    row.get_change_ticks::<PhysicsPose>()
                        .expect("unselected tick")
                        .changed,
                    pose_tick
                );
            }
        }
    }
    let actor = sources
        .into_iter()
        .find(|&e| live.world.get::<SparseTransform>(e).is_some())
        .expect("remaining sparse row");
    let before = *live
        .world
        .get::<SparseTransform>(actor)
        .expect("sparse random access");
    let record = live
        .host
        .execute_command(
            &mut live.world,
            "sparse.move",
            &json!({"actor":ara_script::value::entity_json(actor.to_bits())}),
            None,
        )
        .expect("sparse World.Get");
    assert_eq!(
        live.world
            .get::<SparseTransform>(actor)
            .expect("native sparse write")
            .position[0],
        123.0
    );
    live.host
        .undo(&mut live.world, &record)
        .expect("undo sparse native write");
    assert_eq!(
        *live
            .world
            .get::<SparseTransform>(actor)
            .expect("restored sparse native storage"),
        before
    );
    assert!(
        live.host.diag(ara_script::abi::DIAG_THREADS_SEEN) > 1,
        "non-serial sparse packets execute on multiple managed threads"
    );
    assert!(
        live.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0);
}

//! Checked engine views share the real native ECS storage and preserve codec wire data.
mod support;

use ara_physics::{CharacterController, PhysicsId, PhysicsPose, VehicleTelemetry, Velocity};
use serde_json::json;

const GAME: &str = r#"
/// <summary>Moves native physics columns as one generated batch.</summary>
public static partial class NativeMovement
{
    [Query]
    static void Run(ref Velocity velocity, ref PhysicsPose pose, ref VehicleTelemetry telemetry, in PhysicsId id, in Tick tick)
    {
        velocity.Linear.X += 1;
        pose.Position += velocity.Linear * tick.Dt;
        telemetry.Gear = (int)id.Value;
        if (id.Value == ulong.MaxValue)
        {
            velocity.Linear.X = float.NaN;
            velocity.Angular.Z = float.PositiveInfinity;
        }
        if (id.Value == ulong.MaxValue - 1) telemetry.WheelsInContact = 5;
    }
}
/// <summary>Mutates checked engine views as one undoable command.</summary>
[Command("physics.tune")] public static partial class Tune
{
    /// <summary>Rejection must restore the native values through their unchanged codec journal.</summary>
    /// <param name="actor">Entity to tune.</param>
    /// <param name="reject">Whether to discard all edits.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, bool reject)
    {
        World.Get<Velocity>(actor).Linear.Y = 99;
        if (World.Read<Velocity>(actor).Linear.Y != 99) return ctx.Reject("stale read");
        var pose = World.Read<PhysicsPose>(actor);
        pose.Position.X = 42;
        World.Set(actor, in pose);
        return reject ? ctx.Reject("rejected") : CommandResult.Ok;
    }
}
/// <summary>Structural insertion retains its existing JSON payload contract.</summary>
[Command("physics.replace")] public static partial class Replace
{
    /// <summary>Replaces the velocity at the normal structural sync point.</summary>
    /// <param name="actor">Entity to replace.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor)
    {
        ctx.Commands.Add(actor, new Velocity { Linear = new System.Numerics.Vector3(7, 8, 9) });
        return CommandResult.Ok;
    }
}
/// <summary>Codec validation still rejects invalid supplementary native values.</summary>
[Command("physics.invalid-controller")] public static partial class InvalidController
{
    /// <summary>Tests each component's reflected range validation.</summary>
    /// <param name="actor">Entity to validate.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor)
    {
        World.Get<Transform>(actor).Position.X = 123;
        World.Get<CharacterController>(actor).Radius = -1;
        return CommandResult.Ok;
    }
}
/// <summary>Telemetry command writes still use reflected validation.</summary>
[Command("physics.invalid-telemetry")] public static partial class InvalidTelemetry
{
    /// <summary>Rejects invalid grounded wheel counts.</summary>
    /// <param name="actor">Entity to validate.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor)
    {
        World.Get<Transform>(actor).Position.X = 123;
        World.Get<VehicleTelemetry>(actor).WheelsInContact = 5;
        return CommandResult.Ok;
    }
}
/// <summary>Unrepresentable gameplay values must fail before a command can change them.</summary>
[Command("physics.nonfinite")] public static partial class Nonfinite
{
    /// <summary>Earlier direct writes roll back when a later codec read fails.</summary>
    /// <param name="actor">Entity containing non-finite gameplay values.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor)
    {
        World.Get<Transform>(actor).Position.X = 123;
        World.Get<CharacterController>(actor).Radius = 0.7f;
        World.Get<Velocity>(actor).Linear.Y = 99;
        return ctx.Reject("nonfinite");
    }
}
/// <summary>Repairing invalid gameplay state must not create unrestorable undo bytes.</summary>
[Command("physics.repair")] public static partial class Repair
{
    /// <summary>Earlier valid codec and raw writes must roll back before rejecting repair.</summary>
    /// <param name="actor">Entity containing invalid telemetry.</param>
    /// <param name="other">Entity receiving earlier valid writes.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, Entity other)
    {
        World.Get<Transform>(other).Position.X = 200;
        World.Get<Velocity>(other).Linear.Y = 111;
        World.Get<VehicleTelemetry>(actor).WheelsInContact = 0;
        return CommandResult.Ok;
    }
}
/// <summary>Editing another field must preserve invalid gameplay state on rejection.</summary>
[Command("physics.touch")] public static partial class Touch
{
    /// <summary>Refuses an invalid preimage even when the edited field is valid.</summary>
    /// <param name="actor">Entity containing invalid telemetry.</param>
    /// <param name="other">Entity receiving earlier valid writes.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, Entity other)
    {
        World.Get<Transform>(other).Position.X = 200;
        World.Get<Velocity>(other).Linear.Y = 111;
        World.Get<VehicleTelemetry>(actor).EngineRpm = 1200;
        return CommandResult.Ok;
    }
}
/// <summary>Structural commands must save every inverse before changing the world.</summary>
[Command("physics.structural")] public static partial class Structural
{
    /// <summary>Queues valid earlier operations before changing the selected component.</summary>
    /// <param name="actor">Entity to replace, remove from, or despawn.</param>
    /// <param name="other">Entity whose prior valid writes must roll back on failure.</param>
    /// <param name="mode">Zero inserts, one removes, and two despawns.</param>
    /// <param name="range">Whether the selected component is telemetry.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, Entity other, int mode, bool range)
    {
        World.Get<Transform>(other).Position.X = 200;
        var spawned = ctx.Commands.Spawn();
        ctx.Commands.Add(spawned, new Transform());
        ctx.Commands.Add(other, new Velocity { Linear = new System.Numerics.Vector3(10, 11, 12) });
        if (mode == 2) ctx.Commands.Despawn(actor);
        else if (range)
        {
            if (mode == 0) ctx.Commands.Add(actor, new VehicleTelemetry());
            else ctx.Commands.Remove<VehicleTelemetry>(actor);
        }
        else
        {
            if (mode == 0) ctx.Commands.Add(actor, new Velocity { Linear = new System.Numerics.Vector3(7, 8, 9) });
            else ctx.Commands.Remove<Velocity>(actor);
        }
        return CommandResult.Ok;
    }
}
"#;

fn velocity_bits(value: Velocity) -> ([u32; 3], [u32; 3]) {
    (
        value.linear.map(f32::to_bits),
        value.angular.map(f32::to_bits),
    )
}

fn structural_preimages(
    live: &mut support::Live,
    nonfinite: bevy_ecs::entity::Entity,
    other: bevy_ecs::entity::Entity,
) {
    let invalid_range = live
        .world
        .spawn((
            PhysicsId {
                value: u64::MAX - 1,
            },
            PhysicsPose::default(),
            Velocity::default(),
            VehicleTelemetry::default(),
        ))
        .id();
    live.run();
    assert_eq!(
        live.world
            .get::<VehicleTelemetry>(invalid_range)
            .expect("query telemetry")
            .wheels_in_contact,
        5
    );
    for command in ["physics.repair", "physics.touch"] {
        let telemetry = *live
            .world
            .get::<VehicleTelemetry>(invalid_range)
            .expect("invalid telemetry");
        let other_velocity = *live.world.get::<Velocity>(other).expect("other velocity");
        let other_transform = *live
            .world
            .get::<support::EcsTransform>(other)
            .expect("other transform");
        let error = live
            .host
            .execute_command(
                &mut live.world,
                command,
                &json!({
                    "actor":ara_script::value::entity_json(invalid_range.to_bits()),
                    "other":ara_script::value::entity_json(other.to_bits()),
                }),
                None,
            )
            .expect_err("invalid preimages must not enter the write journal");
        assert!(
            error
                .data
                .as_ref()
                .is_some_and(|data| data.get("exception").is_some()),
            "{command} must run: {error:?}"
        );
        assert_eq!(
            *live
                .world
                .get::<VehicleTelemetry>(invalid_range)
                .expect("untouched invalid telemetry"),
            telemetry
        );
        assert_eq!(
            *live
                .world
                .get::<Velocity>(other)
                .expect("earlier codec write rolled back"),
            other_velocity
        );
        assert_eq!(
            *live
                .world
                .get::<support::EcsTransform>(other)
                .expect("earlier native write rolled back"),
            other_transform
        );
    }
    for (actor, range) in [(nonfinite, false), (invalid_range, true)] {
        let velocity = velocity_bits(
            *live
                .world
                .get::<Velocity>(actor)
                .expect("original velocity"),
        );
        let telemetry = *live
            .world
            .get::<VehicleTelemetry>(actor)
            .expect("original telemetry");
        for mode in 0..3 {
            let count = live.world.entities().len();
            let other_velocity = *live.world.get::<Velocity>(other).expect("other velocity");
            let other_transform = *live
                .world
                .get::<support::EcsTransform>(other)
                .expect("other transform");
            let error = live.host.execute_command(&mut live.world, "physics.structural", &json!({
                "actor":ara_script::value::entity_json(actor.to_bits()),
                "other":ara_script::value::entity_json(other.to_bits()), "mode":mode,"range":range,
            }), None).expect_err("unrestorable preimage rejects the whole structural batch");
            assert_eq!(error.code, ara_script::codes::COMMAND_REJECTED);
            assert_eq!(
                live.world.entities().len(),
                count,
                "reserved spawn was discarded"
            );
            assert_eq!(
                velocity_bits(
                    *live
                        .world
                        .get::<Velocity>(actor)
                        .expect("actor still has velocity")
                ),
                velocity
            );
            assert_eq!(
                *live
                    .world
                    .get::<VehicleTelemetry>(actor)
                    .expect("actor still has telemetry"),
                telemetry
            );
            assert_eq!(
                *live
                    .world
                    .get::<Velocity>(other)
                    .expect("earlier queued insert was not applied"),
                other_velocity
            );
            assert_eq!(
                *live
                    .world
                    .get::<support::EcsTransform>(other)
                    .expect("callback write rolled back"),
                other_transform
            );
        }
    }
    let mut valid = live
        .world
        .spawn((
            PhysicsId { value: 10_000 },
            Velocity::default(),
            VehicleTelemetry::default(),
        ))
        .id();
    for mode in 0..3 {
        let count = live.world.entities().len();
        let other_velocity = *live.world.get::<Velocity>(other).expect("other velocity");
        let other_transform = *live
            .world
            .get::<support::EcsTransform>(other)
            .expect("other transform");
        let record = live.host.execute_command(&mut live.world, "physics.structural", &json!({
            "actor":ara_script::value::entity_json(valid.to_bits()),
            "other":ara_script::value::entity_json(other.to_bits()), "mode":mode,"range":false,
        }), None).expect("valid structural state remains editable");
        match mode {
            0 => assert_eq!(
                live.world
                    .get::<Velocity>(valid)
                    .expect("replacement")
                    .linear,
                [7.0, 8.0, 9.0]
            ),
            1 => assert!(live.world.get::<Velocity>(valid).is_none()),
            _ => assert!(live.world.get_entity(valid).is_err()),
        }
        live.host
            .undo(&mut live.world, &record)
            .expect("undo valid structural command");
        if mode == 2 {
            valid = live
                .world
                .query::<(bevy_ecs::entity::Entity, &PhysicsId)>()
                .iter(&live.world)
                .find_map(|(entity, id)| (id.value == 10_000).then_some(entity))
                .expect("despawn inverse restored entity");
        }
        assert_eq!(
            *live
                .world
                .get::<Velocity>(valid)
                .expect("restored native velocity"),
            Velocity::default()
        );
        assert_eq!(
            *live
                .world
                .get::<VehicleTelemetry>(valid)
                .expect("restored native telemetry"),
            VehicleTelemetry::default()
        );
        assert_eq!(
            *live
                .world
                .get::<Velocity>(other)
                .expect("undone other insert"),
            other_velocity
        );
        assert_eq!(
            *live
                .world
                .get::<support::EcsTransform>(other)
                .expect("undone callback write"),
            other_transform
        );
        assert_eq!(
            live.world.entities().len(),
            count,
            "undo removed the successful spawn"
        );
    }
}

#[test]
fn physics_queries_and_random_access_share_native_storage_with_undoable_json_commands() {
    let _clr = support::clr_lock();
    let mut live = support::Live::start("native-physics-views", &[("Game", GAME, &[])]);
    let entities = (0..8192)
        .map(|i| {
            live.world
                .spawn((
                    PhysicsId { value: i + 1 },
                    PhysicsPose::default(),
                    Velocity {
                        linear: [2.0, 3.0, 4.0],
                        angular: [-0.0, 6.0, 7.0],
                    },
                    VehicleTelemetry::default(),
                    CharacterController::default(),
                    support::EcsTransform::default(),
                ))
                .id()
        })
        .collect::<Vec<_>>();
    live.run();
    for (i, &entity) in entities.iter().enumerate() {
        assert_eq!(
            live.world
                .get::<Velocity>(entity)
                .expect("native velocity")
                .linear,
            [3.0, 3.0, 4.0]
        );
        assert_eq!(
            live.world
                .get::<VehicleTelemetry>(entity)
                .expect("native telemetry")
                .gear,
            i as i32 + 1
        );
        assert_eq!(
            live.world
                .get::<PhysicsPose>(entity)
                .expect("native pose")
                .position,
            [
                3.0 * (1.0f32 / 60.0),
                3.0 * (1.0f32 / 60.0),
                4.0 * (1.0f32 / 60.0)
            ],
            "native quaternion-containing rows use their actual Rust stride"
        );
    }
    let entity = entities[0];
    let before_velocity = *live.world.get::<Velocity>(entity).expect("velocity");
    let before_pose = *live.world.get::<PhysicsPose>(entity).expect("pose");
    let actor = ara_script::value::entity_json(entity.to_bits());
    let rejected = live.host.execute_command(
        &mut live.world,
        "physics.tune",
        &json!({"actor":actor,"reject":true}),
        Some(entity),
    );
    assert!(rejected.is_err());
    assert_eq!(
        *live
            .world
            .get::<Velocity>(entity)
            .expect("restored velocity"),
        before_velocity
    );
    assert_eq!(
        *live
            .world
            .get::<PhysicsPose>(entity)
            .expect("restored pose"),
        before_pose
    );

    let record = live
        .host
        .execute_command(
            &mut live.world,
            "physics.tune",
            &json!({"actor":actor,"reject":false}),
            Some(entity),
        )
        .expect("command");
    assert_eq!(record.writes.len(), 2);
    for write in &record.writes {
        assert!(
            serde_json::from_slice::<serde_json::Value>(&write.before).is_ok(),
            "accessor preimages remain JSON"
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(&write.after).is_ok(),
            "accessor results remain JSON"
        );
    }
    assert_eq!(
        live.world
            .get::<Velocity>(entity)
            .expect("written velocity")
            .linear[1],
        99.0
    );
    live.host
        .undo(&mut live.world, &record)
        .expect("undo native writes");
    assert_eq!(
        *live.world.get::<Velocity>(entity).expect("undone velocity"),
        before_velocity
    );
    assert_eq!(
        live.world
            .get::<Velocity>(entity)
            .expect("velocity bits")
            .angular
            .map(f32::to_bits),
        before_velocity.angular.map(f32::to_bits),
        "codec journals preserve native IEEE signed zero"
    );
    assert_eq!(
        *live.world.get::<PhysicsPose>(entity).expect("undone pose"),
        before_pose
    );

    let replace = live
        .host
        .execute_command(
            &mut live.world,
            "physics.replace",
            &json!({"actor":actor}),
            Some(entity),
        )
        .expect("JSON structural insert");
    assert_eq!(
        live.world
            .get::<Velocity>(entity)
            .expect("replaced native velocity")
            .linear,
        [7.0, 8.0, 9.0]
    );
    live.host
        .undo(&mut live.world, &replace)
        .expect("undo JSON insert");
    assert_eq!(
        *live
            .world
            .get::<Velocity>(entity)
            .expect("undone replacement"),
        before_velocity
    );

    let before_controller = *live
        .world
        .get::<CharacterController>(entity)
        .expect("controller");
    let before_telemetry = *live
        .world
        .get::<VehicleTelemetry>(entity)
        .expect("telemetry");
    let before_transform = *live
        .world
        .get::<support::EcsTransform>(entity)
        .expect("transform");
    for command in ["physics.invalid-controller", "physics.invalid-telemetry"] {
        let invalid = live.host.execute_command(
            &mut live.world,
            command,
            &json!({"actor":actor}),
            Some(entity),
        );
        assert!(
            invalid.is_err(),
            "codec range validation rejects invalid native values"
        );
        assert_eq!(
            *live
                .world
                .get::<CharacterController>(entity)
                .expect("restored controller"),
            before_controller
        );
        assert_eq!(
            *live
                .world
                .get::<VehicleTelemetry>(entity)
                .expect("restored telemetry"),
            before_telemetry
        );
        assert_eq!(
            *live
                .world
                .get::<support::EcsTransform>(entity)
                .expect("restored transform"),
            before_transform
        );
    }

    let nonfinite = live
        .world
        .spawn((
            PhysicsId { value: u64::MAX },
            PhysicsPose::default(),
            VehicleTelemetry::default(),
            Velocity::default(),
            CharacterController::default(),
            support::EcsTransform::default(),
        ))
        .id();
    live.run();
    let before = *live
        .world
        .get::<Velocity>(nonfinite)
        .expect("nonfinite velocity");
    assert!(before.linear[0].is_nan());
    assert!(before.angular[2].is_infinite());
    let invalid = live.host.execute_command(
        &mut live.world,
        "physics.nonfinite",
        &json!({"actor":ara_script::value::entity_json(nonfinite.to_bits())}),
        Some(nonfinite),
    );
    assert!(
        invalid.is_err(),
        "codec decoding refuses non-finite native input"
    );
    let after = *live
        .world
        .get::<Velocity>(nonfinite)
        .expect("unchanged nonfinite velocity");
    assert_eq!(
        after.linear.map(f32::to_bits),
        before.linear.map(f32::to_bits)
    );
    assert_eq!(
        after.angular.map(f32::to_bits),
        before.angular.map(f32::to_bits)
    );
    assert_eq!(
        *live
            .world
            .get::<CharacterController>(nonfinite)
            .expect("unchanged controller"),
        CharacterController::default()
    );
    assert_eq!(
        *live
            .world
            .get::<support::EcsTransform>(nonfinite)
            .expect("rolled back transform"),
        support::EcsTransform::default()
    );
    structural_preimages(&mut live, nonfinite, entity);
    assert!(
        live.diagnostics
            .iter()
            .all(|d| d.severity != ara_diag::Severity::Error),
        "{:?}",
        live.diagnostics
    );
    assert_eq!(live.finish(), 0);
}

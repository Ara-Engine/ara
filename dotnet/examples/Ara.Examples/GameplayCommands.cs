using Ara;

namespace Ara.Examples;

/// <summary>Executes the compiled component reference recipes.</summary>
[Command("examples.components")]
public static partial class ExerciseComponents
{
    /// <summary>Reads and writes every built-in component through the gameplay API.</summary>
    /// <param name="target">The actor carrying the recipe components.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target)
    {
        ComponentExamples.RunAll(target, in ctx.Tick);
        return CommandResult.Ok;
    }
}

/// <summary>Exercises native and variable-value structural changes together.</summary>
[Command("examples.structural")]
public static partial class ExerciseStructural
{
    /// <summary>Creates an actor and replaces the target's light with procedural audio intent.</summary>
    /// <param name="target">The actor carrying the recipe components.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target)
    {
        var commands = ctx.Commands;
        GameplayCommands.SpawnActor(in commands);
        commands.Remove<Light>(target);
        commands.Add(target, new AudioGraphSource { Parameters = new() { ["frequency"] = 220f }, Triggers = [] });
        return CommandResult.Ok;
    }
}

/// <summary>Checks that rejected writes never become gameplay state.</summary>
[Command("examples.reject")]
public static partial class RejectChanges
{
    /// <summary>Writes a string, an asset and a native transform, then rejects the command.</summary>
    /// <param name="target">The actor carrying the recipe components.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target)
    {
        World.Get<AudioSource>(target).Bus = "rejected";
        World.Get<AudioSource>(target).Clip = "@audio/rejected.wav#as_rejected";
        World.Get<Transform>(target).Position = System.Numerics.Vector3.One;
        return ctx.Reject("example_rejection");
    }
}

/// <summary>Verifies fault cleanup discards invalid callback-local values.</summary>
[Command("examples.fault")]
public static partial class FaultChanges
{
    /// <summary>Leaves an unencodable value behind before throwing a gameplay fault.</summary>
    /// <param name="target">The actor carrying the recipe components.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target)
    {
        World.Get<Light>(target).Intensity = float.NaN;
        throw new InvalidOperationException("example_fault");
    }
}

/// <summary>Exercises a reflected actor despawn and its restorable component values.</summary>
[Command("examples.retire")]
public static partial class RetireActor
{
    /// <summary>Runs the structural retirement recipe on an existing actor.</summary>
    /// <param name="target">The actor carrying the recipe components.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target)
    {
        var commands = ctx.Commands;
        GameplayCommands.RetireActor(in commands, target);
        return CommandResult.Ok;
    }
}

/// <summary>Structural gameplay recipes, applied at the normal ECS sync point.</summary>
public static class GameplayCommands
{
    #region topics/csharp-commands
    /// <summary>Spawns a moving actor; structural changes land in push order.</summary>
    public static Entity SpawnActor(in Commands commands)
    {
        var actor = commands.Spawn();
        commands.Add(actor, new Transform { Scale = System.Numerics.Vector3.One });
        commands.Add(actor, new RigidBody { Motion = MotionType.Dynamic, Mass = 2 });
        commands.Add(actor, new Collider {
            Kind = ColliderKind.Box, HalfExtents = new System.Numerics.Vector3(0.5f),
            Radius = 0.5f, HalfHeight = 0.5f, Layer = 1, Friction = 0.6f
        });
        commands.Add(actor, new Velocity { Linear = System.Numerics.Vector3.UnitX });
        return actor;
    }

    /// <summary>Removes motion and despawns at the following sync point.</summary>
    public static void RetireActor(in Commands commands, Entity actor)
    {
        if (World.Has<Velocity>(actor)) commands.Remove<Velocity>(actor);
        commands.Despawn(actor);
    }
    #endregion
}

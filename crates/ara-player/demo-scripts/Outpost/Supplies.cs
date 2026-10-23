namespace Outpost;

/// <summary>Drops a supply crate 6 m above an entity; gravity brings it down.</summary>
[Command("game.DropSupplies"), Expose(ExecuteOn.Authority, Caller.Authority)]
[Example("{\"near\": \"0v0\"}")]
public static partial class DropSupplies
{
    /// <summary>Spawns the crate beside <paramref name="near"/>, above it.</summary>
    /// <param name="ctx">The command context.</param>
    /// <param name="near">The entity to drop it next to.</param>
    /// <param name="at">That entity's transform.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity near, ref Transform at)
    {
        var commands = ctx.Commands;
        var crate = commands.Spawn();
        commands.Add(crate, new Transform
        {
            Position = at.Position + new Vector3(1.5f, 6f, 0f),
            Scale = new Vector3(1f, 1f, 1f),
        });
        commands.Add(crate, new Mass { Value = 1f });
        commands.Add(crate, new Motion());
        return CommandResult.Ok;
    }
}

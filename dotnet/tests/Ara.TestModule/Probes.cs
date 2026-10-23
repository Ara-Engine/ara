namespace Ara.TestModule;

/// <summary>Counts ticks; the host tests check it advances exactly once per tick per entity.</summary>
[Component]
public partial struct Counter
{
    /// <summary>Ticks seen.</summary>
    public int Value;

    /// <summary>Sum of the entity's index bits seen so far (checks the entity column lines up).</summary>
    public ulong EntitySum;
}

/// <summary>Arms a fault: systems and behaviours throw on entities whose fuse is lit.</summary>
[Component]
public partial struct Fuse
{
    /// <summary>When true, `Explode` throws.</summary>
    public bool Lit;
}

/// <summary>A value that reload tests edit (L1 changes the constant written here).</summary>
[Component]
public partial struct Marker
{
    /// <summary>Written every tick by `Mark`.</summary>
    public int Value;
}

/// <summary>Counter systems.</summary>
public static partial class Counting
{
    [Query]
    static void Count(ref Counter c, Entity e)
    {
        c.Value += 1;
        c.EntitySum += e.Index;
    }

    [Query(Order = 5)]
    static void Explode(in Fuse f, ref Counter c)
    {
        if (f.Lit) throw new InvalidOperationException("fuse lit");
    }

    [Query]
    static void Mark(ref Marker m)
    {
        m.Value = 1;
    }
}

/// <summary>Throws in OnTick once its fuse is lit; counts ticks otherwise.</summary>
public sealed partial class Bomb : Behaviour
{
    /// <summary>Ticks this bomb has seen.</summary>
    [Property] public partial int Ticks { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick t)
    {
        if (Has<Fuse>() && Get<Fuse>().Lit) throw new InvalidOperationException("bomb went off");
        Ticks += 1;
    }
}

/// <summary>Always throws (fault containment for commands).</summary>
[Command("test.Fail")]
public static partial class Fail
{
    /// <summary>Writes then throws: the write must be rolled back.</summary>
    /// <param name="ctx">The command context.</param>
    /// <param name="target">An entity with a counter.</param>
    /// <param name="c">Its counter.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity target, ref Counter c)
    {
        c.Value = -1;
        throw new InvalidOperationException("command failed on purpose");
    }
}

/// <summary>Spawns `count` entities with counters and despawns `victim`.</summary>
[Command("test.Shuffle")]
public static partial class Shuffle
{
    /// <summary>Structural changes for the undo tests.</summary>
    /// <param name="ctx">The command context.</param>
    /// <param name="victim">An entity to despawn.</param>
    /// <param name="count">How many entities to spawn.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity victim, [Range(0, 64)] int count)
    {
        for (int i = 0; i < count; i++)
        {
            var e = ctx.Commands.Spawn();
            ctx.Commands.Add(e, new Counter { Value = 100 + i });
        }
        ctx.Commands.Despawn(victim);
        return CommandResult.Ok;
    }
}

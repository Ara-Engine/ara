[assembly: Simulation]

namespace Outpost;

/// <summary>Health and healing supplies of a survivor.</summary>
[Component]
public partial struct Vitality
{
    /// <summary>Hit points; 0 is downed.</summary>
    [Range(0, 100), Replicate(Audience.Relevant)] public ushort Hp;

    /// <summary>Medkits carried.</summary>
    [Range(0, 9), Replicate(Audience.Owner)] public byte Medkits;
}

/// <summary>Played when a survivor uses a medkit.</summary>
[Cue]
public partial struct MedkitUsed
{
    /// <summary>Who healed.</summary>
    public Entity Actor;
}

/// <summary>Uses one medkit: +25 hit points, up to 100.</summary>
[Command("game.UseMedkit"), Expose(ExecuteOn.Authority, Caller.Owner, RatePerSecond = 2)]
[Example("{\"actor\": \"0v0\"}")]
public static partial class UseMedkit
{
    /// <summary>Heals <paramref name="actor"/> if it is hurt and has a medkit.</summary>
    /// <param name="ctx">The command context.</param>
    /// <param name="actor">The survivor using the medkit.</param>
    /// <param name="v">The survivor's vitality.</param>
    public static CommandResult Execute(ref CommandContext ctx, Entity actor, ref Vitality v)
    {
        if (v.Hp == 100 || v.Medkits == 0) return ctx.Reject("cannot_heal");
        v.Medkits--;
        v.Hp = (ushort)Math.Min(100, v.Hp + 25);
        ctx.Cue(new MedkitUsed { Actor = actor });
        return CommandResult.Ok;
    }
}

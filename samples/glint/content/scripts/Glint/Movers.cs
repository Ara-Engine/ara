namespace Glint;

/// <summary>Kinematic course elements, placed purely from the tick number so a replay is exact.</summary>
public static partial class Movers
{
    /// <summary>Slides each moving platform along its path.</summary>
    [Query(Order = 0)]
    static void Slide(in Mover mover, ref Transform transform, in Tick tick)
    {
        if (mover.PeriodTicks <= 0) return;
        var phase = (tick.Frame + (ulong)Math.Max(0, mover.PhaseTicks)) % (ulong)mover.PeriodTicks;
        var s = MathF.Sin(phase * MathF.Tau / mover.PeriodTicks);
        transform.Position = mover.Origin + mover.Travel * s;
    }

    /// <summary>Turns each spinner.</summary>
    [Query(Order = 0)]
    static void Spin(in Spinner spinner, ref Transform transform, in Tick tick)
    {
        transform.Rotation.Y = spinner.StartYaw + spinner.DegreesPerSecond * (float)tick.Elapsed;
    }

    /// <summary>Bobs each pickup about its home, each a little out of step with the next.</summary>
    [Query(Order = 0)]
    static void Bob(in Pickup pickup, ref Transform transform, in Tick tick)
    {
        var phase = (float)tick.Elapsed * 2.4f + pickup.Home.X * 0.35f + pickup.Home.Z * 0.2f;
        transform.Position = pickup.Home + new Vector3(0, 0.16f * MathF.Sin(phase), 0);
    }
}

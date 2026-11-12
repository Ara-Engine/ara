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
}

using Ara;

namespace Game;

/// <summary>Presentation-side motion driven from simulation time.</summary>
public static class Visuals
{
    /// <summary>Pulses each entity's scale with a per-entity phase.</summary>
    public static void Pulse(PlaceChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:pulse
        Span<Transform> tr = c.Transform;
        ReadOnlySpan<Entity> ids = c.Entities;
        float phase = t.Elapsed * Tuning.PulseRate;
        for (int i = 0; i < tr.Length; i++)
            tr[i].Scale = 1f + Tuning.PulseAmount * MathF.Sin(phase + (ids[i].Bits & 63) * 0.1f);
    }
}

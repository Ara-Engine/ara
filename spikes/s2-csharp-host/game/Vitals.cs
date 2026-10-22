using Ara;

namespace Game;

/// <summary>Health regeneration.</summary>
public static class Vitals
{
    /// <summary>Regenerates health; full entities drop back to a fraction so the column keeps changing.</summary>
    public static void Regen(VitalsChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:regen
        Span<Health> hp = c.Health;
        for (int i = 0; i < hp.Length; i++)
        {
            ref Health h = ref hp[i];
            h.Value += h.Regen * t.Dt;
            if (h.Value >= h.Max) h.Value = h.Max * Tuning.RegenResetFraction;
        }
    }
}

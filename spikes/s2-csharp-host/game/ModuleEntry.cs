using Ara;

namespace Game;

/// <summary>
/// The module's system list. Registration order is the deterministic order for systems whose
/// accesses conflict; non-conflicting systems may run in parallel. (In M1 a source generator
/// writes this from <c>[System]</c> attributes.)
/// </summary>
public static class ModuleEntry
{
    /// <summary>Called by the host after each load, and after any L1 edit to this method.</summary>
    public static void Register(SystemRegistry r)
    {
        r.Add("integrate", Movement.Integrate);
        r.Add("gravity", Forces.Gravity);
        r.Add("swirl", Steering.Swirl);
        r.Add("drag", Forces.Drag);
        r.Add("bounds", Movement.Bounds);
        r.Add("regen", Vitals.Regen);
        r.Add("pulse", Visuals.Pulse);
    }
}

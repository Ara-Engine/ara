namespace Game;

/// <summary>Gameplay constants. Constants are folded into callers, so changing one is an L3 edit.</summary>
public static class Tuning
{
    /// <summary>Gravity in m/s².</summary>
    public const float Gravity = 9.81f;
    /// <summary>Fraction of vertical speed kept on a ground bounce.</summary>
    public const float Restitution = 0.5f;
    /// <summary>Linear velocity kept per 1/60 s.</summary>
    public const float LinearDamping = 0.999f;
    /// <summary>Angular velocity kept per 1/60 s.</summary>
    public const float AngularDamping = 0.995f;
    /// <summary>Half the side of the square play area, in metres.</summary>
    public const float HalfExtent = 500f;
    /// <summary>Strength of the swirl around the origin.</summary>
    public const float SwirlStrength = 40f;
    /// <summary>Health fraction an entity drops to after reaching max.</summary>
    public const float RegenResetFraction = 0.5f;
    /// <summary>Scale pulse frequency in radians per second.</summary>
    public const float PulseRate = 2f;
    /// <summary>Scale pulse amplitude.</summary>
    public const float PulseAmount = 0.1f;
}

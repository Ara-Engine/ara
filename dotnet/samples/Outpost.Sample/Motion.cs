namespace Outpost;

/// <summary>Linear velocity of a moving body.</summary>
[Component]
public partial struct SampleVelocity
{
    /// <summary>Metres per second.</summary>
    [Unit(Units.MetersPerSecond)] public Vector3 Linear;
}

/// <summary>Mass of a body; 0 means it ignores gravity.</summary>
[Component]
public partial struct Mass
{
    /// <summary>Kilograms.</summary>
    [Unit(Units.Kilograms), Min(0)] public float Value;
}

/// <summary>World tuning shared by every system.</summary>
[Resource]
public partial struct GameRules
{
    /// <summary>Downward acceleration.</summary>
    [Unit(Units.MetersPerSecondSquared), Default(9.81)] public float Gravity;
}

/// <summary>Pulls bodies with mass down.</summary>
public static partial class Gravity
{
    [Query]
    static void Run(ref SampleVelocity v, in Mass m, in Tick t, in GameRules rules)
    {
        if (m.Value > 0f) v.Linear.Y -= rules.Gravity * t.Dt;
    }
}

/// <summary>Moves transforms by their velocity.</summary>
public static partial class Movement
{
    [Query(Order = 10)]
    static void Run(ref Transform tr, in SampleVelocity v, in Tick t) => tr.Position += v.Linear * t.Dt;
}

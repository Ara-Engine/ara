namespace Outpost;

/// <summary>Linear velocity of a scripted body (physics bodies use the engine's Velocity).</summary>
[Component]
public partial struct Motion
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

    /// <summary>Lowest centre height of scripted bodies (unit boxes rest on the ground).</summary>
    [Unit(Units.Meters), Default(0.5)] public float Floor;
}

/// <summary>Pulls bodies with mass down.</summary>
public static partial class Gravity
{
    [Query]
    static void Run(ref Motion m, in Mass mass, in Tick t, in GameRules rules)
    {
        if (mass.Value > 0f) m.Linear.Y -= rules.Gravity * t.Dt;
    }
}

/// <summary>Moves transforms by their velocity; bodies stop on the floor.</summary>
public static partial class Movement
{
    [Query(Order = 10)]
    static void Run(ref Transform tr, ref Motion m, in Tick t, in GameRules rules)
    {
        tr.Position += m.Linear * t.Dt;
        if (tr.Position.Y < rules.Floor)
        {
            tr.Position.Y = rules.Floor;
            m.Linear = Vector3.Zero;
        }
    }
}

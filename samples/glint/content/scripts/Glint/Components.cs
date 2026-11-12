[assembly: Simulation]

namespace Glint;

/// <summary>A walkable top face. The ball counts as grounded while it rests on one.</summary>
[Component]
public partial struct Surface
{
    /// <summary>Half extents of the box in metres; its centre is the entity's position.</summary>
    [Unit(Units.Meters)] public Vector3 Half;
}

/// <summary>A platform that slides back and forth along a straight path on the tick clock.</summary>
[Component]
public partial struct Mover
{
    /// <summary>Centre of the path in world space.</summary>
    [Unit(Units.Meters)] public Vector3 Origin;
    /// <summary>Offset from the centre at the sine's peak: the platform travels ± this.</summary>
    [Unit(Units.Meters)] public Vector3 Travel;
    /// <summary>Fixed ticks for one full there-and-back.</summary>
    [Default(240)] public int PeriodTicks;
    /// <summary>Ticks to add to the clock, so platforms in a row do not move together.</summary>
    public int PhaseTicks;
}

/// <summary>A bar that turns about the vertical axis at a constant rate.</summary>
[Component]
public partial struct Spinner
{
    /// <summary>Turn rate in degrees per second.</summary>
    [Default(90)] public float DegreesPerSecond;
    /// <summary>Yaw at tick zero, in degrees.</summary>
    public float StartYaw;
}

/// <summary>A pad that records the ball's respawn point when the ball reaches it.</summary>
[Component]
public partial struct Checkpoint
{
    /// <summary>Order along the course, from 1.</summary>
    [Default(1)] public int Index;
    /// <summary>Horizontal reach in metres.</summary>
    [Default(2.5f), Unit(Units.Meters)] public float Radius;
    /// <summary>Whether the ball has touched it this run.</summary>
    [Replicate] public bool Reached;
}

/// <summary>The finish ring.</summary>
[Component]
public partial struct Goal
{
    /// <summary>Horizontal reach in metres.</summary>
    [Default(2.5f), Unit(Units.Meters)] public float Radius;
}

/// <summary>The rolling ball's control state. Gameplay state stays in this component.</summary>
[Component]
public partial struct Ball
{
    /// <summary>Authenticated input slot, filled from the engine's player controller.</summary>
    public ulong Player;
    /// <summary>Where the ball comes back after a fall: the start or the latest checkpoint.</summary>
    [Unit(Units.Meters)] public Vector3 Spawn;
    /// <summary>Highest checkpoint index reached.</summary>
    [Replicate] public int Checkpoint;
    /// <summary>Times the ball fell off the course this run.</summary>
    [Replicate] public int Falls;
    /// <summary>Whether the ball rests on a surface.</summary>
    [Replicate] public bool Grounded;
    /// <summary>Ticks left in which a jump still counts after rolling off an edge.</summary>
    public int CoyoteTicks;
    /// <summary>Jump button on the previous tick, for press detection and replay.</summary>
    public bool JumpHeld;
    /// <summary>Restart button on the previous tick.</summary>
    public bool RestartHeld;
}

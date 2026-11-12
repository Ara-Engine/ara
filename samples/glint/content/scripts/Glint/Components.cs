[assembly: Simulation]

namespace Glint;

/// <summary>A walkable top face: an oriented box. Its centre and orientation are the entity's
/// <c>Transform</c> (Euler angles, applied as Rx * Ry * Rz); scripts have no contact queries, so
/// the ball asks this instead of the physics engine.</summary>
[Component]
public partial struct Surface
{
    /// <summary>Half extents of the box in metres.</summary>
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
    [Default(270)] public int PeriodTicks;
    /// <summary>Ticks to add to the clock, so platforms in a row do not move together.</summary>
    public int PhaseTicks;
}

/// <summary>Something that turns about the vertical axis at a constant rate (bars, pickups).</summary>
[Component]
public partial struct Spinner
{
    /// <summary>Turn rate in degrees per second.</summary>
    [Default(90)] public float DegreesPerSecond;
    /// <summary>Yaw at tick zero, in degrees.</summary>
    public float StartYaw;
}

/// <summary>A gate that records the ball's respawn point when the ball reaches it.</summary>
[Component]
public partial struct Checkpoint
{
    /// <summary>Order along the course, from 1.</summary>
    [Default(1)] public int Index;
    /// <summary>Reach in metres (a sphere around the gate's spawn point).</summary>
    [Default(3f), Unit(Units.Meters)] public float Radius;
    /// <summary>Whether the ball has touched it this run.</summary>
    [Replicate] public bool Reached;
    /// <summary>Tick it was reached: a monotonic counter the presentation turns into a flash and a sound.</summary>
    [Replicate] public ulong ReachedTick;
}

/// <summary>The finish: a sphere around the goal point.</summary>
[Component]
public partial struct Goal
{
    /// <summary>Reach in metres.</summary>
    [Default(4f), Unit(Units.Meters)] public float Radius;
}

/// <summary>A pickup orb.</summary>
[Component]
public partial struct Pickup
{
    /// <summary>Reach in metres.</summary>
    [Default(1.3f), Unit(Units.Meters)] public float Radius;
    /// <summary>Whether it has been collected this run.</summary>
    [Replicate] public bool Taken;
    /// <summary>Tick it was collected, for the presentation's flash and chime.</summary>
    [Replicate] public ulong TakenTick;
    /// <summary>Scale it returns to when the run restarts.</summary>
    public float Size;
    /// <summary>Where it floats; it bobs about this point.</summary>
    [Unit(Units.Meters)] public Vector3 Home;
}

/// <summary>A point on the course's centre line; the camera follows the polyline they make.</summary>
[Component]
public partial struct Waypoint
{
    /// <summary>Position along the route, from 0.</summary>
    public int Index;
}

/// <summary>The cannon: it captures a ball that rolls into its intake, holds it while it loads,
/// then fires it along a ballistic arc that was solved to land on the goal pad.</summary>
[Component]
public partial struct Cannon
{
    /// <summary>The intake point on the deck.</summary>
    [Unit(Units.Meters)] public Vector3 Breech;
    /// <summary>Where the ball leaves the barrel.</summary>
    [Unit(Units.Meters)] public Vector3 Muzzle;
    /// <summary>Launch velocity in m/s.</summary>
    [Unit(Units.MetersPerSecond)] public Vector3 Launch;
    /// <summary>0 idle, 1 loading, 2 firing and in flight.</summary>
    [Replicate] public int State;
    /// <summary>Ticks spent loading.</summary>
    public int Loaded;
    /// <summary>Where the ball was when it was captured.</summary>
    public Vector3 From;
    /// <summary>The ball's falls count at the last shot, to reset after a respawn.</summary>
    public int SeenFalls;
    /// <summary>Tick loading began (a counter for the load sound).</summary>
    [Replicate] public ulong LoadTick;
    /// <summary>Tick of the shot (a counter for the boom and the flash).</summary>
    [Replicate] public ulong FireTick;
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
    /// <summary>Held by the cannon: the controller leaves it alone.</summary>
    [Replicate] public bool Held;
    /// <summary>In the air after a shot: the controller steers it only a little.</summary>
    [Replicate] public bool Flying;
    /// <summary>Ticks left in which a jump still counts after rolling off an edge.</summary>
    public int CoyoteTicks;
    /// <summary>Jump button on the previous tick, for press detection and replay.</summary>
    public bool JumpHeld;
    /// <summary>Restart button on the previous tick.</summary>
    public bool RestartHeld;
    /// <summary>Tick of the last jump (a counter for the presentation).</summary>
    [Replicate] public ulong JumpTick;
    /// <summary>Tick of the last landing from a real fall of height (a counter for the thud).</summary>
    [Replicate] public ulong LandTick;
    /// <summary>Planar speed in m/s, for the camera and the sound.</summary>
    [Replicate] public float Speed;
    /// <summary>Height of the last surface the ball rested on, for the fall check.</summary>
    public float LastGroundY;
    /// <summary>Fastest downward speed since the ball left a surface, for the landing thud.</summary>
    public float FallSpeed;
}

[assembly: Simulation]

namespace Outpost;

/// <summary>A scavenger's authoritative status and private satchel. All state lives in ECS.</summary>
[Component]
public partial struct Survivor
{
    /// <summary>Authenticated input slot, filled by the director from PlayerControl.</summary>
    [Replicate] public ulong Player;
    /// <summary>Whether this slot has entered the run.</summary>
    [Replicate] public bool Active;
    /// <summary>Health; zero means downed and another scavenger can revive you.</summary>
    [Default(100), Range(0, 100), Replicate] public int Health;
    /// <summary>Ticks until rescue becomes unavailable while downed.</summary>
    [Replicate] public int BleedTicks;
    /// <summary>Circuit boards carried by this player only.</summary>
    [Replicate(Audience.Owner)] public int Circuits;
    /// <summary>Battery cells carried by this player only.</summary>
    [Replicate(Audience.Owner)] public int Cells;
    /// <summary>Radio parts carried by this player only.</summary>
    [Replicate(Audience.Owner)] public int Radios;
    /// <summary>Private inventory visibility.</summary>
    [Replicate(Audience.Owner)] public bool InventoryOpen;
    /// <summary>Prompt: 0 explore, 1 door, 2 salvage, 3 revive, 4 extraction, 5 EMP.</summary>
    [Replicate(Audience.Owner)] public int Prompt;
    /// <summary>Team extraction radius membership.</summary>
    [Replicate] public bool AtExtraction;
    /// <summary>Last successful interaction tick, for presentation feedback.</summary>
    [Replicate(Audience.Owner)] public ulong FeedbackTick;
    /// <summary>Interaction hold on the previous fixed tick, for edge detection and replay.</summary>
    public bool UseHeld;
    /// <summary>Inventory hold on the previous fixed tick.</summary>
    public bool InventoryHeld;
    /// <summary>Ticks spent contributing to a teammate's revive.</summary>
    public int ReviveTicks;
    /// <summary>Last drone damage tick, providing invulnerability between hits.</summary>
    public ulong HitTick;
}

/// <summary>A salvage cache. Ownership of the private reward stays on the collector.</summary>
[Component]
public partial struct SalvageCache
{
    /// <summary>Reward type: zero circuits, one cells, two radios.</summary>
    [Range(0, 2)] public int Kind;
    /// <summary>Whether any player has collected this cache.</summary>
    [Replicate] public bool Taken;
    /// <summary>Tick of pickup, for a replicated pickup flash.</summary>
    [Replicate] public ulong TakenTick;
}

/// <summary>A shared physical door, opened once for the whole team.</summary>
[Component]
public partial struct SharedDoor
{
    /// <summary>Whether the shared door has been unlocked.</summary>
    [Replicate] public bool Open;
    /// <summary>Closed leaf yaw in degrees.</summary>
    public float ClosedYaw;
    /// <summary>Current opening angle in degrees.</summary>
    [Replicate] public float Angle;
    /// <summary>Tick at which the door opened, for presentation feedback.</summary>
    [Replicate] public ulong OpenTick;
}

/// <summary>A navmesh security drone. Its path is owned by the native NavAgent.</summary>
[Component]
public partial struct SecurityDrone
{
    /// <summary>Patrol home position.</summary>
    public Vector3 Home;
    /// <summary>Whether a player is being chased.</summary>
    [Replicate] public bool Chasing;
    /// <summary>EMP stun expiration in simulation ticks.</summary>
    [Replicate] public ulong StunnedUntil;
}

/// <summary>The team's extraction beacon.</summary>
[Component]
public partial struct ExtractionBeacon
{
    /// <summary>Radius in metres that every active scavenger must enter.</summary>
    [Default(4), Unit(Units.Meters)] public float Radius;
}

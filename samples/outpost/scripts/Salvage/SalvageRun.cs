namespace Outpost;

/// <summary>A five-minute salvage run. Only this director changes mission state.</summary>
public sealed partial class SalvageRun : Behaviour
{
    /// <summary>Salvage caches required before extraction is enabled.</summary>
    [Property, Default(8), Replicate] public partial int Goal { get; set; }
    /// <summary>Team salvage progress; individual satchels remain private.</summary>
    [Property, Replicate] public partial int Salvaged { get; set; }
    /// <summary>Whether any scavenger has entered the run.</summary>
    [Property, Replicate] public partial bool Started { get; set; }
    /// <summary>Whether the team extracted successfully.</summary>
    [Property, Replicate] public partial bool Won { get; set; }
    /// <summary>Whether the team ran out of time or survivors.</summary>
    [Property, Replicate] public partial bool Lost { get; set; }
    /// <summary>Fixed ticks remaining until nightfall.</summary>
    [Property, Default(18000), Replicate] public partial int TicksLeft { get; set; }
    /// <summary>Elapsed fixed ticks, used by the result screen.</summary>
    [Property, Replicate] public partial int ElapsedTicks { get; set; }
    /// <summary>Consecutive ticks the whole team has held the extraction zone.</summary>
    [Property, Replicate] public partial int ExtractionTicks { get; set; }
    /// <summary>Number of scavengers participating in this run.</summary>
    [Property, Replicate] public partial int ActivePlayers { get; set; }
    /// <summary>Number of drones currently chasing, driving the tension music layer.</summary>
    [Property, Replicate] public partial int Alert { get; set; }
    /// <summary>Successful teammate rescues, shown in results.</summary>
    [Property, Replicate] public partial int Revives { get; set; }
    /// <summary>Remaining emergency respawns, shared by the team.</summary>
    [Property, Default(3), Replicate] public partial int Rescues { get; set; }
    /// <summary>Consecutive ticks with no standing survivor.</summary>
    [Property] public partial int WipeTicks { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Span<Entity> playerBuffer = stackalloc Entity[8];
        var players = World.Query<Survivor>(playerBuffer);
        SortPlayers(players);
        Span<Entity> cacheBuffer = stackalloc Entity[32];
        var caches = World.Query<SalvageCache>(cacheBuffer);
        Span<Entity> doorBuffer = stackalloc Entity[8];
        var doors = World.Query<SharedDoor>(doorBuffer);
        Span<Entity> droneBuffer = stackalloc Entity[8];
        var drones = World.Query<SecurityDrone>(droneBuffer);
        Span<Entity> beaconBuffer = stackalloc Entity[1];
        var beacons = World.Query<ExtractionBeacon>(beaconBuffer);
        if (beacons.Length != 1) return;
        var extraction = World.Read<Transform>(beacons[0]).Position;
        var radius = World.Read<ExtractionBeacon>(beacons[0]).Radius;
        ActivePlayers = 0;
        var standing = 0;
        var extracted = 0;
        foreach (var entity in players)
        {
            ref var player = ref World.Get<Survivor>(entity);
            player.Player = Input.Player(entity);
            var use = Input.Button(player.Player, "Interact");
            var inventory = Input.Button(player.Player, "Inventory");
            if (use || inventory || Input.Axis2(player.Player, "Move") != Vector2.Zero || Input.Button(player.Player, "Jump")) player.Active = true;
            if (!player.Active) continue;
            ActivePlayers++;
            if (inventory && !player.InventoryHeld) player.InventoryOpen = !player.InventoryOpen;
            player.InventoryHeld = inventory;
            if (Won || Lost) { player.UseHeld = use; continue; }
            if (player.Health == 0)
            {
                player.BleedTicks--;
                if (player.BleedTicks <= 0 && Rescues > 0)
                {
                    Rescues--;
                    player.Health = 60;
                    World.Get<Transform>(entity).Position = extraction + new Vector3((float)player.Player, 0, 0);
                }
                player.UseHeld = use;
                continue;
            }
            standing++;
            var position = World.Read<Transform>(entity).Position;
            player.AtExtraction = PlanarDistanceSquared(position, extraction) < radius * radius;
            if (player.AtExtraction) extracted++;
            Interactions(entity, ref player, players, caches, doors, drones, tick, use);
            player.UseHeld = use;
        }
        AnimateDoors(doors, tick.Dt);
        if (Won || Lost) return;
        Started |= ActivePlayers > 0;
        if (!Started) return;
        ElapsedTicks++;
        TicksLeft--;
        WipeTicks = standing == 0 ? WipeTicks + 1 : 0;
        if (TicksLeft <= 0 || WipeTicks >= 300) { Lost = true; return; }
        UpdateDrones(drones, players, tick);
        ExtractionTicks = Salvaged >= Goal && extracted == ActivePlayers && standing == ActivePlayers ? ExtractionTicks + 1 : 0;
        if (ExtractionTicks >= 180) Won = true;
    }

    /// <summary>Input slot defines contested interaction order independently of ECS allocation.</summary>
    static void SortPlayers(Span<Entity> players)
    {
        for (var i = 1; i < players.Length; i++)
            for (var j = i; j > 0 && Input.Player(players[j]) < Input.Player(players[j - 1]); j--)
                (players[j], players[j - 1]) = (players[j - 1], players[j]);
    }

    /// <summary>Horizontal distance allows a hovering pickup or drone to be used from the ground.</summary>
    static float PlanarDistanceSquared(Vector3 a, Vector3 b)
    {
        var x = a.X - b.X;
        var z = a.Z - b.Z;
        return x * x + z * z;
    }
}

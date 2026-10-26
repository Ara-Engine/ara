namespace Outpost;

/// <summary>The salvage director's validated shared interactions.</summary>
public sealed partial class SalvageRun
{
    /// <summary>Authoritative range checks; a client sends only its interaction button.</summary>
    void Interactions(Entity actor, ref Survivor player, Span<Entity> players, Span<Entity> caches,
        Span<Entity> doors, Span<Entity> drones, in Tick tick, bool use)
    {
        var position = World.Read<Transform>(actor).Position;
        var nearest = Entity.Null;
        var distance = 2.4f * 2.4f;
        var kind = 0;
        foreach (var entity in players)
        {
            ref readonly var teammate = ref World.Read<Survivor>(entity);
            if (!teammate.Active || teammate.Health > 0 || entity.Equals(actor)) continue;
            var d = PlanarDistanceSquared(position, World.Read<Transform>(entity).Position);
            if (d < distance) { distance = d; nearest = entity; kind = 3; }
        }
        foreach (var entity in caches)
        {
            if (World.Read<SalvageCache>(entity).Taken) continue;
            var d = PlanarDistanceSquared(position, World.Read<Transform>(entity).Position);
            if (d < distance) { distance = d; nearest = entity; kind = 2; }
        }
        foreach (var entity in doors)
        {
            if (World.Read<SharedDoor>(entity).Open) continue;
            var d = PlanarDistanceSquared(position, World.Read<Transform>(entity).Position);
            if (d < distance) { distance = d; nearest = entity; kind = 1; }
        }
        foreach (var entity in drones)
        {
            if (World.Read<SecurityDrone>(entity).StunnedUntil > tick.Frame) continue;
            var d = PlanarDistanceSquared(position, World.Read<Transform>(entity).Position);
            if (d < distance) { distance = d; nearest = entity; kind = 5; }
        }
        player.Prompt = kind == 0 && player.AtExtraction ? 4 : kind;
        if (kind == 3 && use)
        {
            player.ReviveTicks++;
            if (player.ReviveTicks >= 90)
            {
                ref var teammate = ref World.Get<Survivor>(nearest);
                teammate.Health = 60;
                teammate.BleedTicks = 0;
                player.ReviveTicks = 0;
                player.FeedbackTick = tick.Frame;
                Revives++;
            }
            return;
        }
        player.ReviveTicks = 0;
        if (!use || player.UseHeld || nearest.IsNull) return;
        switch (kind)
        {
            case 1:
                ref var door = ref World.Get<SharedDoor>(nearest);
                door.Open = true;
                door.OpenTick = tick.Frame;
                break;
            case 2:
                ref var cache = ref World.Get<SalvageCache>(nearest);
                cache.Taken = true;
                cache.TakenTick = tick.Frame;
                switch (cache.Kind)
                {
                    case 0: player.Circuits++; break;
                    case 1: player.Cells++; break;
                    case 2: player.Radios++; break;
                }
                World.Get<Transform>(nearest).Scale = Vector3.Zero;
                Salvaged++;
                break;
            case 5:
                World.Get<SecurityDrone>(nearest).StunnedUntil = tick.Frame + 300;
                Navigation.Stop(nearest);
                break;
        }
        player.FeedbackTick = tick.Frame;
    }

    /// <summary>The kinematic collider follows the shared leaf transform through physics.push.</summary>
    static void AnimateDoors(Span<Entity> doors, float dt)
    {
        foreach (var entity in doors)
        {
            ref var door = ref World.Get<SharedDoor>(entity);
            door.Angle = MathF.Min(100, door.Angle + (door.Open ? 100 * dt : 0));
            World.Get<Transform>(entity).Rotation.Y = door.ClosedYaw + door.Angle;
        }
    }
}

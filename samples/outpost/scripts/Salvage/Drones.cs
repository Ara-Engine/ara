namespace Outpost;

/// <summary>The salvage director's authoritative drone decisions.</summary>
public sealed partial class SalvageRun
{
    /// <summary>Drones chase through Recast paths, damage in range and can be EMP-stunned with E.</summary>
    void UpdateDrones(Span<Entity> drones, Span<Entity> players, in Tick tick)
    {
        Alert = 0;
        foreach (var entity in drones)
        {
            ref var drone = ref World.Get<SecurityDrone>(entity);
            if (drone.StunnedUntil > tick.Frame)
            {
                drone.Chasing = false;
                Navigation.Stop(entity);
                continue;
            }
            var origin = World.Read<Transform>(entity).Position;
            var target = Entity.Null;
            var distance = 18f * 18f;
            foreach (var actor in players)
            {
                ref readonly var player = ref World.Read<Survivor>(actor);
                if (!player.Active || player.Health <= 0) continue;
                var d = PlanarDistanceSquared(origin, World.Read<Transform>(actor).Position);
                if (d < distance) { target = actor; distance = d; }
            }
            drone.Chasing = !target.IsNull;
            if (drone.Chasing) Alert++;
            if (tick.Frame % 30 == 0)
            {
                var goal = target.IsNull ? drone.Home : World.Read<Transform>(target).Position;
                Navigation.GoTo(entity, goal);
            }
            if (target.IsNull || distance > 2.2f * 2.2f) continue;
            ref var victim = ref World.Get<Survivor>(target);
            if (tick.Frame < victim.HitTick + 60) continue;
            victim.HitTick = tick.Frame;
            victim.Health = Math.Max(0, victim.Health - 12);
            if (victim.Health == 0) victim.BleedTicks = 1200;
        }
    }
}

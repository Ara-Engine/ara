namespace Glint;

/// <summary>Queries and moves over the course's surfaces and the ball.</summary>
public static class Course
{
    /// <summary>
    /// Whether a ball of <paramref name="radius"/> at <paramref name="position"/> rests on a
    /// <see cref="Surface"/>; <paramref name="top"/> is that surface's height.
    /// </summary>
    public static bool Resting(Vector3 position, float radius, out float top) => Resting(position, radius, out top, out _);

    /// <summary>As <see cref="Resting(Vector3, float, out float)"/>, also naming the surface entity.</summary>
    public static bool Resting(Vector3 position, float radius, out float top, out Entity under)
    {
        Span<Entity> buffer = stackalloc Entity[64];
        foreach (var entity in World.Query<Surface>(buffer))
        {
            var centre = World.Read<Transform>(entity).Position;
            var half = World.Read<Surface>(entity).Half;
            var surface = centre.Y + half.Y;
            var gap = position.Y - radius - surface;
            if (gap < -0.1f || gap > 0.15f) continue;
            if (MathF.Abs(position.X - centre.X) > half.X + radius * 0.6f) continue;
            if (MathF.Abs(position.Z - centre.Z) > half.Z + radius * 0.6f) continue;
            top = surface;
            under = entity;
            return true;
        }
        top = 0;
        under = Entity.Null;
        return false;
    }

    /// <summary>
    /// The velocity of a <see cref="Mover"/> platform this tick, m/s. The physics engine does not
    /// carry a ball on a platform that scripts move by writing its transform, so the ball's
    /// controller adds this itself.
    /// </summary>
    public static Vector3 MoverVelocity(Entity entity, in Tick tick)
    {
        var mover = World.Read<Mover>(entity);
        if (mover.PeriodTicks <= 0) return Vector3.Zero;
        var phase = (tick.Frame + (ulong)Math.Max(0, mover.PhaseTicks)) % (ulong)mover.PeriodTicks;
        var omega = MathF.Tau / mover.PeriodTicks;
        return mover.Travel * (MathF.Cos(phase * omega) * omega / tick.Dt);
    }

    /// <summary>The top of the highest <see cref="Surface"/> at or below a point, if any lies under it.</summary>
    public static bool FloorBelow(Vector3 position, out float top)
    {
        Span<Entity> buffer = stackalloc Entity[64];
        var found = false;
        top = 0;
        foreach (var entity in World.Query<Surface>(buffer))
        {
            var centre = World.Read<Transform>(entity).Position;
            var half = World.Read<Surface>(entity).Half;
            var surface = centre.Y + half.Y;
            if (surface > position.Y + 0.05f || (found && surface <= top)) continue;
            if (MathF.Abs(position.X - centre.X) > half.X || MathF.Abs(position.Z - centre.Z) > half.Z) continue;
            top = surface;
            found = true;
        }
        return found;
    }

    /// <summary>Places a physics body at a point and stops it.</summary>
    public static void Teleport(Entity entity, Vector3 point)
    {
        World.Get<Transform>(entity).Position = point;
        World.Get<PhysicsPose>(entity).Position = point;
        ref var velocity = ref World.Get<Velocity>(entity);
        velocity.Linear = Vector3.Zero;
        velocity.Angular = Vector3.Zero;
    }

    /// <summary>Planar (XZ) distance squared.</summary>
    public static float Planar(Vector3 a, Vector3 b)
    {
        var x = a.X - b.X;
        var z = a.Z - b.Z;
        return x * x + z * z;
    }
}

namespace Glint;

/// <summary>Queries over the course's oriented surfaces, and moves of the ball.</summary>
public static class Course
{
    static Vector3 RotX(Vector3 v, float a) { var (s, c) = MathF.SinCos(a); return new(v.X, v.Y * c - v.Z * s, v.Y * s + v.Z * c); }
    static Vector3 RotY(Vector3 v, float a) { var (s, c) = MathF.SinCos(a); return new(v.X * c + v.Z * s, v.Y, -v.X * s + v.Z * c); }
    static Vector3 RotZ(Vector3 v, float a) { var (s, c) = MathF.SinCos(a); return new(v.X * c - v.Y * s, v.X * s + v.Y * c, v.Z); }
    const float Rad = MathF.PI / 180f;

    /// <summary>Gravity the cannon's arc was solved with, m/s squared (the physics default).</summary>
    public const float Gravity = 9.81f;

    /// <summary>Rotates <paramref name="v"/> by Euler degrees applied as Rx * Ry * Rz (the engine's order).</summary>
    public static Vector3 Rotate(Vector3 euler, Vector3 v) => RotX(RotY(RotZ(v, euler.Z * Rad), euler.Y * Rad), euler.X * Rad);

    /// <summary>The inverse of <see cref="Rotate"/>.</summary>
    public static Vector3 Unrotate(Vector3 euler, Vector3 v) => RotZ(RotY(RotX(v, -euler.X * Rad), -euler.Y * Rad), -euler.Z * Rad);

    /// <summary>
    /// Whether a ball of <paramref name="radius"/> at <paramref name="position"/> rests on a
    /// <see cref="Surface"/>; also the surface's up direction and entity.
    /// </summary>
    public static bool Resting(Vector3 position, float radius, out Vector3 normal, out Entity under)
    {
        Span<Entity> buffer = stackalloc Entity[96];
        foreach (var entity in World.Query<Surface>(buffer))
        {
            ref readonly var transform = ref World.Read<Transform>(entity);
            var half = World.Read<Surface>(entity).Half;
            var local = Unrotate(transform.Rotation, position - transform.Position);
            var gap = local.Y - half.Y - radius;
            if (gap < -0.14f || gap > 0.22f) continue;
            if (MathF.Abs(local.X) > half.X + radius * 0.5f || MathF.Abs(local.Z) > half.Z + radius * 0.5f) continue;
            normal = Rotate(transform.Rotation, Vector3.UnitY);
            under = entity;
            return true;
        }
        normal = Vector3.UnitY;
        under = Entity.Null;
        return false;
    }

    /// <summary>The height of the highest surface under a point, if any lies below it.</summary>
    public static bool FloorBelow(Vector3 position, out float top, out Vector3 normal)
    {
        Span<Entity> buffer = stackalloc Entity[96];
        var found = false;
        top = 0;
        normal = Vector3.UnitY;
        foreach (var entity in World.Query<Surface>(buffer))
        {
            ref readonly var transform = ref World.Read<Transform>(entity);
            var half = World.Read<Surface>(entity).Half;
            var local = Unrotate(transform.Rotation, position - transform.Position);
            if (MathF.Abs(local.X) > half.X || MathF.Abs(local.Z) > half.Z) continue;
            var n = Rotate(transform.Rotation, Vector3.UnitY);
            if (n.Y < 0.3f) continue;
            // Height of the top plane at this x, z.
            var y = transform.Position.Y + (half.Y - n.X * (position.X - transform.Position.X) - n.Z * (position.Z - transform.Position.Z)) / n.Y;
            if (y > position.Y + 0.05f || (found && y <= top)) continue;
            top = y;
            normal = n;
            found = true;
        }
        return found;
    }

    /// <summary>The velocity of a <see cref="Mover"/> platform this tick, m/s. The physics engine
    /// does not carry a ball on a platform that scripts move by writing its transform.</summary>
    public static Vector3 MoverVelocity(Entity entity, in Tick tick)
    {
        var mover = World.Read<Mover>(entity);
        if (mover.PeriodTicks <= 0) return Vector3.Zero;
        var phase = (tick.Frame + (ulong)Math.Max(0, mover.PhaseTicks)) % (ulong)mover.PeriodTicks;
        var omega = MathF.Tau / mover.PeriodTicks;
        return mover.Travel * (MathF.Cos(phase * omega) * omega / tick.Dt);
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

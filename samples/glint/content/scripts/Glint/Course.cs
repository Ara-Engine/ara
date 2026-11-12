namespace Glint;

/// <summary>Queries and moves over the course's surfaces and the ball.</summary>
public static class Course
{
    /// <summary>
    /// Whether a ball of <paramref name="radius"/> at <paramref name="position"/> rests on a
    /// <see cref="Surface"/>; <paramref name="top"/> is that surface's height.
    /// </summary>
    public static bool Resting(Vector3 position, float radius, out float top)
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
            return true;
        }
        top = 0;
        return false;
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

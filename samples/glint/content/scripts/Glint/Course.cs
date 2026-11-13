namespace Glint;

/// <summary>Small helpers shared by the course's scripts.</summary>
public static class Course
{
    const float Rad = MathF.PI / 180f;

    /// <summary>
    /// Euler degrees (the engine's Rx * Ry * Rz) that turn local +Z onto <paramref name="forward"/>
    /// and local +Y toward <paramref name="up"/>.
    /// </summary>
    public static Vector3 LookEuler(Vector3 forward, Vector3 up)
    {
        forward = Vector3.Normalize(forward);
        var right = Vector3.Normalize(Vector3.Cross(up, forward));
        up = Vector3.Cross(forward, right);
        var y = MathF.Asin(Math.Clamp(forward.X, -1f, 1f));
        var x = MathF.Atan2(-forward.Y, forward.Z);
        var z = MathF.Atan2(-up.X, right.X);
        return new Vector3(x, y, z) / Rad;
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

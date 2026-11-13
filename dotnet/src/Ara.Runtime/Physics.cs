using System.Numerics;
using Ara.Hosting;

namespace Ara;

/// <summary>What a ray cast found.</summary>
public readonly struct RayHit
{
    /// <summary>The entity that owns the surface.</summary>
    public Entity Entity { get; init; }
    /// <summary>Where the ray met it, in world space.</summary>
    public Vector3 Point { get; init; }
    /// <summary>The surface's outward normal there.</summary>
    public Vector3 Normal { get; init; }
    /// <summary>How far along the ray, in metres.</summary>
    public float Distance { get; init; }
}

/// <summary>
/// Scene queries against the physics world, for behaviours and commands. Answers come from the
/// bodies as physics left them after the last fixed tick, so the same tick always sees the same
/// world.
/// </summary>
public static class Physics
{
    /// <summary>
    /// Casts a ray from <paramref name="origin"/> along <paramref name="direction"/> for
    /// <paramref name="distance"/> metres and reports the first solid surface it meets
    /// (triggers are passed through). Pass the caster's own entity as
    /// <paramref name="ignore"/> so a ray that starts inside its collider does not hit it.
    /// </summary>
    /// <returns>Whether anything was hit.</returns>
    public static bool Raycast(Vector3 origin, Vector3 direction, float distance, out RayHit hit, Entity ignore = default)
    {
        hit = default;
        var length = direction.Length();
        if (!(length > 0f) || !(distance > 0f) || !float.IsFinite(distance)) return false;
        Span<float> values = stackalloc float[7];
        if (!Host.PhysicsRaycast(origin, direction * (distance / length), ignore, out var entity, values)) return false;
        hit = new RayHit
        {
            Entity = entity,
            Point = new Vector3(values[0], values[1], values[2]),
            Normal = new Vector3(values[3], values[4], values[5]),
            Distance = values[6] * distance,
        };
        return true;
    }
}

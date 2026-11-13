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

/// <summary>Where a contact is in its life this tick.</summary>
public enum ContactPhase
{
    /// <summary>The two first touched this tick.</summary>
    Started,
    /// <summary>They were already touching.</summary>
    Persisted,
    /// <summary>They stopped touching this tick (or one of them was removed).</summary>
    Stopped,
}

/// <summary>One contact of an entity in the last completed physics tick.</summary>
public readonly struct Contact
{
    /// <summary>The entity touched; null once it has been removed.</summary>
    public Entity Other { get; init; }
    /// <summary>Where they touch, in world space.</summary>
    public Vector3 Point { get; init; }
    /// <summary>The normal out of the other surface, towards the entity asked about.</summary>
    public Vector3 Normal { get; init; }
    /// <summary>Started, persisted or stopped this tick.</summary>
    public ContactPhase Phase { get; init; }
    /// <summary>The other collider is a trigger: nothing pushed back.</summary>
    public bool IsTrigger { get; init; }
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

    /// <summary>
    /// The contacts <paramref name="entity"/> took part in during the last completed physics
    /// tick, copied into caller-owned <paramref name="scratch"/>: solid contacts, then triggers,
    /// then character contacts, each in a fixed order. Throws if the span is too small.
    /// A body that has fallen asleep reports none: turn <c>RigidBody.allowSleep</c> off on a
    /// body whose resting contacts you need every tick.
    /// </summary>
    public static Span<Contact> Contacts(Entity entity, Span<Contact> scratch)
    {
        Span<ContactRecord> records = stackalloc ContactRecord[scratch.Length];
        var count = Host.PhysicsContacts(entity, records);
        for (var i = 0; i < count; i++)
        {
            ref readonly var r = ref records[i];
            scratch[i] = new Contact
            {
                Other = new Entity(r.Other),
                Point = new Vector3(r.Px, r.Py, r.Pz),
                Normal = new Vector3(r.Nx, r.Ny, r.Nz),
                Phase = (ContactPhase)r.Phase,
                IsTrigger = r.Sensor != 0,
            };
        }
        return scratch[..count];
    }
}

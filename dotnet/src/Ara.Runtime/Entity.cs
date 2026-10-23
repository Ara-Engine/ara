using System.Runtime.InteropServices;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// A generation-checked handle to an entity in the ECS world: the engine's
/// <c>Entity::to_bits</c> (a complemented index in the low 32 bits, the generation in
/// the high 32). A despawned entity's handle never becomes valid again, so a stale
/// handle reads as missing instead of aliasing a new entity. Use <see cref="IsAlive"/>
/// or <see cref="World.Has{T}(Entity)"/>; there is no overloaded null equality.
/// </summary>
/// <remarks><c>default(Entity)</c> is <see cref="Null"/>: all-zero bits are never a valid entity.</remarks>
[StructLayout(LayoutKind.Sequential)]
public readonly struct Entity : IEquatable<Entity>
{
    /// <summary>The raw 64-bit handle, identical to the engine's.</summary>
    public readonly ulong Bits;

    /// <summary>Wraps raw engine bits.</summary>
    public Entity(ulong bits) => Bits = bits;

    /// <summary>The null handle (<c>default(Entity)</c>).</summary>
    public static Entity Null => default;

    /// <summary>Index part (reused after despawn, with a new generation). The engine stores it complemented.</summary>
    public uint Index => ~(uint)Bits;

    /// <summary>Generation part.</summary>
    public uint Generation => (uint)(Bits >> 32);

    /// <summary>True for <see cref="Null"/>.</summary>
    public bool IsNull => (uint)Bits == 0;

    /// <summary>True if the entity exists in the world right now (behaviours and commands only).</summary>
    public bool IsAlive => !IsNull && Host.EntityAlive(Bits);

    /// <inheritdoc/>
    public bool Equals(Entity other) => Bits == other.Bits;

    /// <inheritdoc/>
    public override bool Equals(object? obj) => obj is Entity e && e.Bits == Bits;

    /// <inheritdoc/>
    public override int GetHashCode() => Bits.GetHashCode();

    /// <summary>The engine's text form: <c>12v1</c> (index, then generation).</summary>
    public override string ToString() => IsNull ? "null" : $"{Index}v{Generation}";
}

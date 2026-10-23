using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace Ara;

/// <summary>
/// A per-entity object with Unity-like ergonomics that holds no authoritative state.
/// Declare <c>sealed partial class Door : Behaviour</c> with
/// <c>[Property] public partial float OpenAngle { get; set; }</c>: the generator backs
/// each property with a column of the generated <c>Door</c> data component, so the
/// value is saved, replicated and survives reload. Other instance fields must be
/// <see cref="TransientAttribute"/> caches (ARA0101).
/// </summary>
/// <remarks>
/// Lifecycle: <see cref="OnSpawn"/> once when the entity gains the behaviour,
/// <see cref="OnTick"/> every fixed tick, <see cref="OnFrame"/> every presented frame,
/// <see cref="OnReload"/> instead of <c>OnSpawn</c> on objects recreated by a module
/// reload. Behaviours of one type run serially in a deterministic order, with
/// exclusive world access, so <see cref="Get{T}"/> may touch any component.
/// </remarks>
public abstract unsafe class Behaviour
{
    internal void* DataPtr;
    internal void* CommandBuffer;

    /// <summary>The entity this behaviour belongs to.</summary>
    public Entity Entity { get; internal set; }

    /// <summary>Deterministic random numbers, reseeded for every callback from the tick, the type and the entity.</summary>
    protected Rng Rng;

    internal void SetRng(Rng rng) => Rng = rng;

    /// <summary>Called once, the first tick the entity has this behaviour.</summary>
    protected internal virtual void OnSpawn() { }

    /// <summary>Called every fixed simulation tick.</summary>
    protected internal virtual void OnTick(in Tick tick) { }

    /// <summary>Called every presented frame (presentation only; never change simulation state here).</summary>
    protected internal virtual void OnFrame(in Frame frame) { }

    /// <summary>Called instead of <see cref="OnSpawn"/> on an object recreated by a module reload; rebuild transient caches here.</summary>
    protected internal virtual void OnReload() { }

    /// <summary>One of this entity's components, for reading and writing.</summary>
    protected ref T Get<T>() where T : unmanaged, IComponent<T> => ref World.Get<T>(Entity);

    /// <summary>True if this entity has the component.</summary>
    protected bool Has<T>() where T : unmanaged, IComponent<T> => World.Has<T>(Entity);

    /// <summary>Structural changes, applied after this phase.</summary>
    protected Commands Commands => new(CommandBuffer);

    /// <summary>Used by generated property bodies: this entity's row of the data component.</summary>
    [EditorBrowsable(EditorBrowsableState.Never)]
    protected ref T AraData<T>() where T : unmanaged, IComponent<T>
    {
        if (DataPtr != null) return ref Unsafe.AsRef<T>(DataPtr);
        return ref World.Get<T>(Entity);
    }
}

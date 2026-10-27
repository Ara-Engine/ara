using System.ComponentModel;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// Structural changes (spawn, despawn, add, remove) and cues, buffered and applied by
/// the host at the next sync point in a deterministic order. Systems take it as a
/// <c>ref Commands</c> parameter; behaviours and commands get one from their context.
/// </summary>
public readonly unsafe struct Commands
{
    readonly void* _buf;

    internal Commands(void* buffer) => _buf = buffer;

    /// <summary>Used by generated system entry points: wraps a call's command buffer.</summary>
    [EditorBrowsable(EditorBrowsableState.Never)]
    public static Commands FromCall(void* buffer) => new(buffer);

    /// <summary>
    /// Reserves a new entity. The handle is valid immediately (use it in later calls);
    /// the entity exists from the sync point on.
    /// </summary>
    public Entity Spawn()
    {
        var e = Host.ReserveEntity(_buf);
        Host.CmdPush(_buf, AbiConst.CmdSpawn, 0, e, null, 0);
        return new Entity(e);
    }

    /// <summary>Despawns the entity at the sync point.</summary>
    public void Despawn(Entity entity) => Host.CmdPush(_buf, AbiConst.CmdDespawn, 0, entity.Bits, null, 0);

    /// <summary>Adds (or replaces) a component at the sync point.</summary>
    public void Add<T>(Entity entity, in T value) where T : IComponent<T> => T.Insert(in this, entity, in value);

    internal void InsertNative<T>(Entity entity, in T value) where T : IComponent<T>
    {
        if (System.Runtime.CompilerServices.RuntimeHelpers.IsReferenceOrContainsReferences<T>())
            throw new InvalidOperationException("managed components require a generated codec");
        T copy = value;
        Host.CmdPush(_buf, AbiConst.CmdInsert, T.TypeIndex, entity.Bits,
            System.Runtime.CompilerServices.Unsafe.AsPointer(ref copy), (uint)System.Runtime.CompilerServices.Unsafe.SizeOf<T>());
    }

    internal void InsertJson(Entity entity, uint index, ReadOnlySpan<byte> json)
    {
        fixed (byte* data = json) Host.CmdPush(_buf, AbiConst.CmdInsert, index, entity.Bits, data, (uint)json.Length);
    }

    /// <summary>Removes a component at the sync point.</summary>
    public void Remove<T>(Entity entity) where T : IComponent<T> =>
        Host.CmdPush(_buf, AbiConst.CmdRemove, T.TypeIndex, entity.Bits, null, 0);

    /// <summary>Emits a presentation cue (sound, VFX). Cues never change simulation state.</summary>
    public void Cue<T>(in T cue) where T : unmanaged, ICue<T>
    {
        T copy = cue;
        Host.CmdPush(_buf, AbiConst.CmdCue, T.CueIndex, Entity.Null.Bits, &copy, (uint)sizeof(T));
    }
}

using System.ComponentModel;
using System.Runtime.CompilerServices;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// Implemented by the generated code of every component struct, so lookups need no
/// reflection (AOT-safe).
/// </summary>
public interface IComponent<TSelf> where TSelf : IComponent<TSelf>
{
    /// <summary>Script component index assigned by the host.</summary>
    static abstract uint TypeIndex { get; }
    /// <summary>Generated storage access; native module components use checked column pointers.</summary>
    static virtual ref TSelf Access(Entity entity, bool write) => ref World.NativeAccess<TSelf>(entity, write);
    /// <summary>Generated structural serialization.</summary>
    static virtual void Insert(in Commands commands, Entity entity, in TSelf value) => commands.InsertNative(entity, in value);
    /// <summary>Checks presence without loading a value.</summary>
    static virtual bool Contains(Entity entity) => World.NativeContains<TSelf>(entity);
}

/// <summary>Implemented by the generated code of every <see cref="ResourceAttribute"/> struct.</summary>
public interface IResource<TSelf> where TSelf : unmanaged, IResource<TSelf>
{
    /// <summary>Script resource index assigned by the host.</summary>
    static abstract uint ResourceIndex { get; }
}

/// <summary>Implemented by the generated code of every <see cref="CueAttribute"/> struct.</summary>
public interface ICue<TSelf> where TSelf : unmanaged, ICue<TSelf>
{
    /// <summary>Script cue index assigned by the host.</summary>
    static abstract uint CueIndex { get; }
}

/// <summary>Thrown when an entity lacks a component that code asked for.</summary>
public sealed class MissingComponentException(Entity entity, string component)
    : InvalidOperationException($"entity {entity} has no `{component}` component")
{
    /// <summary>The entity.</summary>
    public Entity Entity { get; } = entity;
    /// <summary>The component's C# type name.</summary>
    public string Component { get; } = component;
}

/// <summary>
/// Random access to ECS data from behaviours and commands (which run with exclusive
/// world access). Systems never use this: they declare their components as parameters
/// so the scheduler can run them in parallel. References stay valid until the current
/// callback returns; structural changes go through <see cref="Commands"/>.
/// </summary>
public static unsafe class World
{
    /// <summary>The entity's component, for reading and writing. Throws if it is missing.</summary>
    public static ref T Get<T>(Entity entity) where T : IComponent<T> => ref T.Access(entity, true);

    /// <summary>Replaces an existing component; structural insertion uses Commands.Add.</summary>
    public static void Set<T>(Entity entity, in T value) where T : IComponent<T> => T.Access(entity, true) = value;

    internal static ref T NativeAccess<T>(Entity entity, bool write) where T : IComponent<T>
    {
        if (RuntimeHelpers.IsReferenceOrContainsReferences<T>())
            throw new InvalidOperationException("managed components require a generated accessor");
        var p = Host.ComponentPtr(entity.Bits, T.TypeIndex, write);
        if (p == null) throw new MissingComponentException(entity, typeof(T).Name);
        return ref Unsafe.AsRef<T>(p);
    }

    /// <summary>The entity's component, read-only. Throws if it is missing.</summary>
    public static ref readonly T Read<T>(Entity entity) where T : IComponent<T> => ref T.Access(entity, false);

    /// <summary>Used by generated command entry points: the component's address, or null when missing.</summary>
    [EditorBrowsable(EditorBrowsableState.Never)]
    public static T* Ptr<T>(Entity entity, bool write) where T : unmanaged, IComponent<T> =>
        (T*)Host.ComponentPtr(entity.Bits, T.TypeIndex, write);

    /// <summary>True if the entity exists and has the component.</summary>
    public static bool Has<T>(Entity entity) where T : IComponent<T> => T.Contains(entity);

    internal static bool NativeContains<T>(Entity entity) where T : IComponent<T> =>
        Host.ComponentPtr(entity.Bits, T.TypeIndex, write: false) != null;

    /// <summary>A world resource, for reading and writing. Throws if it is not registered.</summary>
    public static ref T Resource<T>() where T : unmanaged, IResource<T>
    {
        var p = Host.ResourcePtr(T.ResourceIndex, write: true);
        if (p == null) throw new InvalidOperationException($"resource `{typeof(T).Name}` is not available");
        return ref Unsafe.AsRef<T>(p);
    }
}

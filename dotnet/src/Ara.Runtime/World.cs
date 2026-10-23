using System.ComponentModel;
using System.Runtime.CompilerServices;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// Implemented by the generated code of every component struct, so lookups need no
/// reflection (AOT-safe).
/// </summary>
public interface IComponent<TSelf> where TSelf : unmanaged, IComponent<TSelf>
{
    /// <summary>Script component index assigned by the host.</summary>
    static abstract uint TypeIndex { get; }
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
    public static ref T Get<T>(Entity entity) where T : unmanaged, IComponent<T>
    {
        var p = Host.ComponentPtr(entity.Bits, T.TypeIndex, write: true);
        if (p == null) throw new MissingComponentException(entity, typeof(T).Name);
        return ref Unsafe.AsRef<T>(p);
    }

    /// <summary>The entity's component, read-only. Throws if it is missing.</summary>
    public static ref readonly T Read<T>(Entity entity) where T : unmanaged, IComponent<T>
    {
        var p = Host.ComponentPtr(entity.Bits, T.TypeIndex, write: false);
        if (p == null) throw new MissingComponentException(entity, typeof(T).Name);
        return ref Unsafe.AsRef<T>(p);
    }

    /// <summary>Used by generated command entry points: the component's address, or null when missing.</summary>
    [EditorBrowsable(EditorBrowsableState.Never)]
    public static T* Ptr<T>(Entity entity, bool write) where T : unmanaged, IComponent<T> =>
        (T*)Host.ComponentPtr(entity.Bits, T.TypeIndex, write);

    /// <summary>True if the entity exists and has the component.</summary>
    public static bool Has<T>(Entity entity) where T : unmanaged, IComponent<T> =>
        Host.ComponentPtr(entity.Bits, T.TypeIndex, write: false) != null;

    /// <summary>A world resource, for reading and writing. Throws if it is not registered.</summary>
    public static ref T Resource<T>() where T : unmanaged, IResource<T>
    {
        var p = Host.ResourcePtr(T.ResourceIndex, write: true);
        if (p == null) throw new InvalidOperationException($"resource `{typeof(T).Name}` is not available");
        return ref Unsafe.AsRef<T>(p);
    }
}

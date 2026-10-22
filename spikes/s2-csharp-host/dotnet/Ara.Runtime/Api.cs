using System.Runtime.InteropServices;
using Ara.Hosting;

namespace Ara;

/// <summary>Opaque entity handle (bevy <c>Entity::to_bits</c>). Stable for the entity's lifetime.</summary>
[StructLayout(LayoutKind.Sequential)]
public readonly struct Entity
{
    /// <summary>The raw 64-bit handle.</summary>
    public readonly ulong Bits;

    /// <inheritdoc/>
    public override string ToString() => $"Entity({Bits:X16})";
}

/// <summary>
/// Collects the systems a module exposes. Each gameplay module has one public static
/// <c>Register(SystemRegistry)</c> method; the host calls it after every load and whenever an L1 edit
/// changes it. The typed <c>Add</c> overloads are generated per query shape (Shapes.g.cs).
/// </summary>
public sealed partial class SystemRegistry
{
    internal readonly List<SystemEntry> Entries = [];

    void AddEntry(string name, ShapeId shape, Delegate system)
    {
        foreach (var e in Entries)
            if (e.Name == name) throw new ArgumentException($"duplicate system '{name}'");
        Entries.Add(new SystemEntry(name, shape, system));
    }
}

/// <summary>Reload probe used by the latency harness: records that code carrying a value ran.</summary>
public static class Probe
{
    /// <summary>Tells the host this code version is running (the host keeps the maximum).</summary>
    public static void Stamp(uint value) => Host.Stamp(value);
}

/// <summary>Logging to the host console.</summary>
public static class Log
{
    /// <summary>Info-level message (shown only when the host is verbose).</summary>
    public static void Info(string message) => Host.Log(0, message);

    /// <summary>Warning.</summary>
    public static void Warn(string message) => Host.Log(1, message);
}

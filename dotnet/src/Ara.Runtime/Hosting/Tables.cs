namespace Ara.Hosting;

/// <summary>One registered system. <see cref="Faulted"/> disables it until the next reload.</summary>
internal sealed unsafe class SystemEntry(string name, delegate*<SystemCall*, void> run)
{
    internal readonly string Name = name;
    internal readonly delegate*<SystemCall*, void> Run = run;
    internal int Faulted;
}

/// <summary>One registered command.</summary>
internal sealed unsafe class CommandEntry(string name, delegate*<CommandCall*, int> invoke)
{
    internal readonly string Name = name;
    internal readonly delegate*<CommandCall*, int> Invoke = invoke;
    internal int Faulted;
}

/// <summary>
/// One behaviour type: its factory and the live objects, keyed by entity. Objects hold
/// no authoritative state; they are created on first sight of an entity and dropped
/// the tick after the entity stops appearing.
/// </summary>
internal sealed class BehaviourEntry(string name, int dataSize, Func<Behaviour> create)
{
    internal readonly string Name = name;
    internal readonly int DataSize = dataSize;
    internal readonly Func<Behaviour> Create = create;
    /// <summary>Stream key for <see cref="Rng"/>: a stable hash of the name.</summary>
    internal readonly uint RngKey = StableHash(name);
    internal readonly Dictionary<ulong, (Behaviour Obj, ulong Seen)> Live = [];
    /// <summary>Entities that had an object in the previous generation (they get <c>OnReload</c>, not <c>OnSpawn</c>).</summary>
    internal HashSet<ulong> Reloaded = [];
    internal ulong SweptFrame = ulong.MaxValue;
    internal int Faulted;
    readonly List<ulong> _gone = [];

    /// <summary>Drops objects for entities not seen since before <paramref name="frame"/> (despawned, or the component was removed).</summary>
    internal IReadOnlyList<ulong> Sweep(ulong frame)
    {
        if (SweptFrame == frame) return [];
        SweptFrame = frame;
        _gone.Clear();
        foreach (var (e, (_, seen)) in Live)
            if (seen != frame) _gone.Add(e);
        _gone.Sort();
        return _gone;
    }

    static uint StableHash(string s)
    {
        uint h = 2166136261;
        foreach (var c in s) h = (h ^ c) * 16777619;
        return h;
    }
}

/// <summary>The dispatch tables of one module generation, swapped only at tick boundaries.</summary>
internal sealed class ModuleTables(SystemEntry[] systems, BehaviourEntry[] behaviours, CommandEntry[] commands)
{
    internal static readonly ModuleTables Empty = new([], [], []);
    internal readonly SystemEntry[] Systems = systems;
    internal readonly BehaviourEntry[] Behaviours = behaviours;
    internal readonly CommandEntry[] Commands = commands;

    internal void ClearFaults()
    {
        foreach (var s in Systems) Volatile.Write(ref s.Faulted, 0);
        foreach (var b in Behaviours) Volatile.Write(ref b.Faulted, 0);
        foreach (var c in Commands) Volatile.Write(ref c.Faulted, 0);
    }
}

/// <summary>One module slot: its name and the running generation's tables.</summary>
internal sealed class ModuleSlot(uint index, string name)
{
    internal readonly uint Index = index;
    internal readonly string Name = name;
    internal ModuleTables Tables = ModuleTables.Empty;
    internal uint Generation;
    internal uint Deltas;
    internal bool Hot;
}

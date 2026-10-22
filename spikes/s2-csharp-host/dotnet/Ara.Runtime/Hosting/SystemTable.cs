namespace Ara.Hosting;

/// <summary>One registered system. <see cref="Faulted"/> disables it until the next reload.</summary>
internal sealed class SystemEntry(string name, ShapeId shape, Delegate system)
{
    internal readonly string Name = name;
    internal readonly ShapeId Shape = shape;
    internal readonly Delegate System = system;
    internal int Faulted;
}

/// <summary>
/// The immutable system table the host dispatches through. It is swapped only at tick boundaries
/// (no system is running then), so <see cref="Run"/> needs no locks. Rust never holds pointers into
/// module code: an L3 swap is a table swap here, after which the old ALC can unload.
/// </summary>
internal sealed unsafe class SystemTable(SystemEntry[] entries)
{
    internal static volatile SystemTable Current = new([]);
    static int s_faults;
    static int s_threadsSeen;
    [ThreadStatic] static bool t_seen;

    internal readonly SystemEntry[] Entries = entries;

    internal static int Faults => s_faults;
    internal static int ThreadsSeen => s_threadsSeen;

    /// <summary>True if both tables expose the same ids, names and shapes (Rust's schedule stays valid).</summary>
    internal bool SameShapeAs(SystemTable other)
    {
        if (other.Entries.Length != Entries.Length) return false;
        for (int i = 0; i < Entries.Length; i++)
            if (Entries[i].Name != other.Entries[i].Name || Entries[i].Shape != other.Entries[i].Shape) return false;
        return true;
    }

    internal void ClearFaults()
    {
        foreach (var e in Entries) Volatile.Write(ref e.Faulted, 0);
    }

    /// <summary>
    /// Runs system <paramref name="id"/> over one chunk. Every exception is caught here: the system is
    /// disabled until the next reload, the host gets one report, and the tick continues.
    /// Called concurrently from task-pool threads.
    /// </summary>
    internal static int Run(uint id, Chunk* chunk, Tick* tick)
    {
        if (!t_seen) { t_seen = true; Interlocked.Increment(ref s_threadsSeen); }
        var entries = Current.Entries;
        if (id >= (uint)entries.Length) return 2;
        var e = entries[id];
        if (Volatile.Read(ref e.Faulted) != 0) return 1;
        try
        {
            ShapeDispatch.Invoke(e.Shape, e.System, chunk, tick);
            return 0;
        }
        catch (Exception ex)
        {
            if (Interlocked.Exchange(ref e.Faulted, 1) == 0)
            {
                Interlocked.Increment(ref s_faults);
                var at = ex.StackTrace?.Split('\n').FirstOrDefault()?.Trim() ?? "";
                Host.ReportFault(id, $"{e.Name}: {ex.GetType().Name}: {ex.Message} {at} (disabled until next reload)");
            }
            return 1;
        }
    }
}

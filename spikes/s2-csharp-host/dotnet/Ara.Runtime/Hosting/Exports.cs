using System.Runtime;
using System.Runtime.InteropServices;
using System.Text;

namespace Ara.Hosting;

/// <summary>
/// The only entry points Rust calls. Each is <c>[UnmanagedCallersOnly]</c> and catches everything:
/// an exception escaping one of these would kill the process.
/// </summary>
public static unsafe class Exports
{
    /// <summary>Checks ABI identity, captures the host table and fills <paramref name="api"/>. Returns 0 on success.</summary>
    [UnmanagedCallersOnly]
    public static int Init(HostInit* host, ScriptApi* api)
    {
        try
        {
            Host.Attach(*host);
            if (host->AbiVersion != AbiInfo.Version)
            {
                Host.Log(2, $"ABI version mismatch: host {host->AbiVersion}, runtime {AbiInfo.Version}");
                return 1;
            }
            if (host->LayoutHash != AbiInfo.LayoutHash)
            {
                Host.Log(2, $"stale generated bindings: host layout hash {host->LayoutHash:X16}, C# {AbiInfo.LayoutHash:X16}; run `ara-s2 gen` and rebuild");
                return 2;
            }
            if (AbiInfo.VerifySizes() is { } err)
            {
                Host.Log(2, "layout mismatch: " + err);
                return 3;
            }
            GCSettings.LatencyMode = GCLatencyMode.SustainedLowLatency;
            api->LoadModule = &LoadModule;
            api->Prewarm = &Prewarm;
            api->HasPending = &HasPending;
            api->TickBoundary = &TickBoundary;
            api->SystemCount = &SystemCount;
            api->SystemInfo = &SystemInfoAt;
            api->RunSystem = &RunSystem;
            api->Diag = &Diag;
            api->Shutdown = &Shutdown;
            return 0;
        }
        catch (Exception e)
        {
            Report(e);
            return 99;
        }
    }

    [UnmanagedCallersOnly]
    static int LoadModule(byte* dir, uint len, uint flags)
    {
        try
        {
            var path = Encoding.UTF8.GetString(dir, (int)len);
            return ModuleHost.Load(path, debugIL: (flags & 1) != 0, watch: (flags & 2) != 0) ? 0 : 1;
        }
        catch (Exception e) { Report(e); return 99; }
    }

    [UnmanagedCallersOnly]
    static int Prewarm()
    {
        try { ModuleHost.Current?.Prewarm(); return 0; }
        catch (Exception e) { Report(e); return 99; }
    }

    [UnmanagedCallersOnly]
    static uint HasPending() => ModuleHost.Current?.HasPending == true ? 1u : 0u;

    [UnmanagedCallersOnly]
    static uint TickBoundary(Tick* tick)
    {
        try { return ModuleHost.Current?.TickBoundary() ?? 0; }
        catch (Exception e) { Report(e); return 0; }
    }

    [UnmanagedCallersOnly]
    static uint SystemCount() => (uint)SystemTable.Current.Entries.Length;

    [UnmanagedCallersOnly]
    static int SystemInfoAt(uint index, SystemInfo* info, byte* nameBuf, uint nameCap)
    {
        try
        {
            var entries = SystemTable.Current.Entries;
            if (index >= (uint)entries.Length) return 1;
            var e = entries[index];
            var n = Encoding.UTF8.GetBytes(e.Name, new Span<byte>(nameBuf, (int)nameCap));
            info->Shape = (uint)e.Shape;
            info->NameLen = (uint)n;
            return 0;
        }
        catch (Exception e) { Report(e); return 99; }
    }

    /// <summary>One boundary crossing per chunk; see <see cref="SystemTable.Run"/> for fault containment.</summary>
    [UnmanagedCallersOnly]
    static int RunSystem(uint id, Chunk* chunk, Tick* tick) => SystemTable.Run(id, chunk, tick);

    [UnmanagedCallersOnly]
    static ulong Diag(uint key)
    {
        try
        {
            return key switch
            {
                1 => (ulong)SystemTable.ThreadsSeen,
                2 => (ulong)(ModuleHost.Current?.AlcsAlive() ?? 0),
                3 => (ulong)(ModuleHost.Current?.AlcsCreated ?? 0),
                4 => (ulong)SystemTable.Faults,
                5 => (ulong)GC.GetTotalMemory(false),
                6 => (ulong)Environment.WorkingSet,
                7 => (ulong)GC.GetTotalPauseDuration().TotalMicroseconds,
                8 => (ulong)GC.CollectionCount(0),
                9 => (ulong)GC.CollectionCount(1),
                10 => (ulong)GC.CollectionCount(2),
                _ => 0,
            };
        }
        catch { return ulong.MaxValue; }
    }

    [UnmanagedCallersOnly]
    static int Shutdown()
    {
        try { return ModuleHost.Current?.Shutdown() ?? 0; }
        catch (Exception e) { Report(e); return -1; }
    }

    static void Report(Exception e)
    {
        try { Host.Log(2, e.ToString()); } catch { /* reporting must never throw across the boundary */ }
    }
}

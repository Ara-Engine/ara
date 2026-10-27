using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Concurrent;

namespace Ara.Hosting;

/// <summary>
/// The <c>ScriptApi</c> entry points the host calls. Each is
/// <c>[UnmanagedCallersOnly]</c> and catches everything: an exception escaping one
/// would kill the process.
/// </summary>
internal static unsafe class Exports
{
    // Routing infrastructure only; each entry owns one world's dispatch and callbacks.
    static readonly ConcurrentDictionary<nint, ScriptRuntime> Runtimes = new();
    internal static bool Register(void* handle, ScriptRuntime runtime) => Runtimes.TryAdd((nint)handle, runtime);
    static ScriptRuntime Resolve(void* handle) => Runtimes.TryGetValue((nint)handle, out var runtime)
        ? runtime : throw new InvalidOperationException("script host is stopped or unknown");
    internal static void Fill(ScriptApi* api)
    {
        api->LoadModules = &LoadModules;
        api->TickBoundary = &TickBoundary;
        api->HasPending = &HasPending;
        api->RunSystem = &RunSystem;
        api->RunBehaviours = &RunBehaviours;
        api->RunCommand = &RunCommand;
        api->SetHeat = &SetHeat;
        api->RequestCompile = &RequestCompile;
        api->Diag = &DiagCounter;
        api->Shutdown = &Shutdown;
    }

    [UnmanagedCallersOnly]
    static int LoadModules(void* host, byte* json, uint len)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.LoadModules(Encoding.UTF8.GetString(json, (int)len)); }
        catch (Exception e) { Report(host, e); return 99; }
    }

    [UnmanagedCallersOnly]
    static uint TickBoundary(void* host, Tick* tick)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.ApplyPending(); }
        catch (Exception e) { Report(host, e); return 0; }
    }

    [UnmanagedCallersOnly]
    static uint HasPending(void* host)
    {
        try { return Resolve(host).HasPending ? 1u : 0u; }
        catch { return 0; }
    }

    /// <summary>One boundary crossing per chunk; faults are contained in <see cref="ScriptRuntime.RunSystem"/>.</summary>
    [UnmanagedCallersOnly]
    static int RunSystem(void* host, SystemCall* call)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.RunSystem(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int RunBehaviours(void* host, BehaviourCall* call)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.RunBehaviours(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int RunCommand(void* host, CommandCall* call)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.RunCommand(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int SetHeat(void* host, uint module, uint hot)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.Loader.SetHeat(module, hot != 0); }
        catch (Exception e) { Report(host, e); return 99; }
    }

    [UnmanagedCallersOnly]
    static int RequestCompile(void* host, byte* json, uint len)
    {
        try { var runtime = Resolve(host); using var binding = runtime.Activate(); return runtime.Loader.RequestCompile(Encoding.UTF8.GetString(json, (int)len)); }
        catch (Exception e) { Report(host, e); return 99; }
    }

    [UnmanagedCallersOnly]
    static ulong DiagCounter(void* host, uint key)
    {
        try
        {
            var runtime = Resolve(host);
            return key switch
            {
                AbiConst.DiagThreadsSeen => (ulong)runtime.ThreadsSeen,
                AbiConst.DiagAlcsAlive => (ulong)runtime.Loader.AlcsAlive,
                AbiConst.DiagAlcsCreated => (ulong)runtime.Loader.AlcsCreated,
                AbiConst.DiagFaults => (ulong)runtime.Faults,
                AbiConst.DiagManagedHeap => (ulong)GC.GetTotalMemory(false),
                AbiConst.DiagGcPauseUs => (ulong)GC.GetTotalPauseDuration().TotalMicroseconds,
                AbiConst.DiagCanReload => runtime.Loader.CanReload ? 1UL : 0UL,
                _ => 0,
            };
        }
        catch { return ulong.MaxValue; }
    }

    [UnmanagedCallersOnly]
    static int Shutdown(void* host)
    {
        try
        {
            if (!Runtimes.TryRemove((nint)host, out var runtime)) return 0;
            using var binding = runtime.Activate();
            return runtime.Shutdown();
        }
        catch (Exception e) { Report(host, e); return -1; }
    }

    static void Report(void* handle, Exception e)
    {
        if (!Runtimes.TryGetValue((nint)handle, out var runtime)) return;
        using var binding = runtime.Activate();
        Diag.Report(AraCodes.RuntimeStart, "error", e.ToString());
    }
}

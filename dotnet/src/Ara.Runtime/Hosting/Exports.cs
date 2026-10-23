using System.Runtime.InteropServices;
using System.Text;

namespace Ara.Hosting;

/// <summary>
/// The <c>ScriptApi</c> entry points the host calls. Each is
/// <c>[UnmanagedCallersOnly]</c> and catches everything: an exception escaping one
/// would kill the process.
/// </summary>
internal static unsafe class Exports
{
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
    static int LoadModules(byte* json, uint len)
    {
        try { return ScriptRuntime.LoadModules(Encoding.UTF8.GetString(json, (int)len)); }
        catch (Exception e) { Report(e); return 99; }
    }

    [UnmanagedCallersOnly]
    static uint TickBoundary(Tick* tick)
    {
        try { return ScriptRuntime.ApplyPending(); }
        catch (Exception e) { Report(e); return 0; }
    }

    [UnmanagedCallersOnly]
    static uint HasPending() => ScriptRuntime.HasPending ? 1u : 0u;

    /// <summary>One boundary crossing per chunk; faults are contained in <see cref="ScriptRuntime.RunSystem"/>.</summary>
    [UnmanagedCallersOnly]
    static int RunSystem(SystemCall* call)
    {
        try { return ScriptRuntime.RunSystem(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int RunBehaviours(BehaviourCall* call)
    {
        try { return ScriptRuntime.RunBehaviours(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int RunCommand(CommandCall* call)
    {
        try { return ScriptRuntime.RunCommand(call); }
        catch { return AbiConst.RunFaulted; }
    }

    [UnmanagedCallersOnly]
    static int SetHeat(uint module, uint hot)
    {
        try { return ScriptRuntime.Loader?.SetHeat(module, hot != 0) ?? -1; }
        catch (Exception e) { Report(e); return 99; }
    }

    [UnmanagedCallersOnly]
    static int RequestCompile(byte* json, uint len)
    {
        try { return ScriptRuntime.Loader?.RequestCompile(Encoding.UTF8.GetString(json, (int)len)) ?? -1; }
        catch (Exception e) { Report(e); return 99; }
    }

    [UnmanagedCallersOnly]
    static ulong DiagCounter(uint key)
    {
        try
        {
            return key switch
            {
                AbiConst.DiagThreadsSeen => (ulong)ScriptRuntime.ThreadsSeen,
                AbiConst.DiagAlcsAlive => (ulong)(ScriptRuntime.Loader?.AlcsAlive ?? 0),
                AbiConst.DiagAlcsCreated => (ulong)(ScriptRuntime.Loader?.AlcsCreated ?? 0),
                AbiConst.DiagFaults => (ulong)ScriptRuntime.Faults,
                AbiConst.DiagManagedHeap => (ulong)GC.GetTotalMemory(false),
                AbiConst.DiagGcPauseUs => (ulong)GC.GetTotalPauseDuration().TotalMicroseconds,
                AbiConst.DiagCanReload => ScriptRuntime.Loader?.CanReload == true ? 1UL : 0UL,
                _ => 0,
            };
        }
        catch { return ulong.MaxValue; }
    }

    [UnmanagedCallersOnly]
    static int Shutdown()
    {
        try { return ScriptRuntime.Shutdown(); }
        catch (Exception e) { Report(e); return -1; }
    }

    static void Report(Exception e) => Diag.Report(AraCodes.RuntimeStart, "error", e.ToString());
}

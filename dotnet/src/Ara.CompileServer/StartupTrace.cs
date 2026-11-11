using System.Diagnostics;
using System.Reflection.PortableExecutable;

namespace Ara.CompileServer;

/// <summary>
/// Optional timestamps and counters on stderr for measuring where launch and edit time go (<c>ARA_TRACE=1</c>; off, and
/// free, otherwise). Every line is <c>[ara-trace] &lt;unix ms&gt; server &lt;label&gt; ...</c>; <c>reload_timing.cs --trace</c>
/// collects them with the bench's own lines and prints a launch timeline and a per-compile GC/JIT summary.
/// </summary>
internal static class StartupTrace
{
    internal static readonly bool Enabled = System.Environment.GetEnvironmentVariable("ARA_TRACE") == "1";

    [ThreadStatic] static string? _outcome;

    /// <summary>The runtime's counters at one moment, to subtract around a piece of work.</summary>
    internal readonly record struct Counters(int Gen0, int Gen1, int Gen2, double PauseMs, long Jit, double JitMs, double CpuMs)
    {
        internal static Counters Now() => new(GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2),
            GC.GetTotalPauseDuration().TotalMilliseconds, System.Runtime.JitInfo.GetCompiledMethodCount(),
            System.Runtime.JitInfo.GetCompilationTime().TotalMilliseconds, Process.GetCurrentProcess().TotalProcessorTime.TotalMilliseconds);
    }

    static long UnixMs() => DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();

    static string Format(Counters c) => $"jit={c.Jit}/{c.JitMs:F0}ms gc={c.Gen0}/{c.Gen1}/{c.Gen2} pause={c.PauseMs:F0}ms cpu={c.CpuMs:F0}ms";

    /// <summary>A labelled point in the server's life with its running totals.</summary>
    internal static void Mark(string label)
    {
        if (!Enabled) return;
        Console.Error.WriteLine($"[ara-trace] {UnixMs()} server {label} {Format(Counters.Now())}");
    }

    /// <summary>Process start, runtime and whether Roslyn's precompiled code is what is running (JIT counts say the same).</summary>
    internal static void Describe()
    {
        if (!Enabled) return;
        var started = new DateTimeOffset(Process.GetCurrentProcess().StartTime.ToUniversalTime()).ToUnixTimeMilliseconds();
        Console.Error.WriteLine($"[ara-trace] {started} server process-start pid={System.Environment.ProcessId}");
        foreach (var assembly in new[] { typeof(Microsoft.CodeAnalysis.Compilation).Assembly, typeof(Microsoft.CodeAnalysis.CSharp.CSharpCompilation).Assembly, typeof(StartupTrace).Assembly })
        {
            string state;
            try
            {
                using var stream = File.OpenRead(assembly.Location);
                using var pe = new PEReader(stream);
                state = pe.PEHeaders.CorHeader!.ManagedNativeHeaderDirectory.Size > 0 ? "readytorun" : "IL-only";
            }
            catch (Exception error) { state = "unknown: " + error.GetType().Name; }
            Console.Error.WriteLine($"[ara-trace] {UnixMs()} server image {assembly.GetName().Name} {state}");
        }
        Console.Error.WriteLine($"[ara-trace] {UnixMs()} server runtime {System.Runtime.InteropServices.RuntimeInformation.FrameworkDescription} "
            + $"tieredPGO={AppContext.GetData("System.Runtime.TieredPGO")} tiered={AppContext.GetData("System.Runtime.TieredCompilation")} "
            + $"cores={System.Environment.ProcessorCount} serverGC={System.Runtime.GCSettings.IsServerGC}");
    }

    /// <summary>The outcome the current work produced (set while the work runs; read when it ends).</summary>
    internal static void Outcome(string text) { if (Enabled) _outcome = text; }

    /// <summary>One line per unit of compile work: how long it took and what the runtime did meanwhile.</summary>
    internal static void Work(string kind, double elapsedMs, Counters before)
    {
        var after = Counters.Now();
        var outcome = _outcome ?? "-";
        _outcome = null;
        Console.Error.WriteLine($"[ara-trace] {UnixMs()} work {kind} outcome={outcome} ms={elapsedMs:F1} gcDelta={after.Gen0 - before.Gen0}/{after.Gen1 - before.Gen1}/{after.Gen2 - before.Gen2} "
            + $"pauseDeltaMs={after.PauseMs - before.PauseMs:F1} jitDelta={after.Jit - before.Jit}/{after.JitMs - before.JitMs:F0}ms cpuDeltaMs={after.CpuMs - before.CpuMs:F0}");
    }
}

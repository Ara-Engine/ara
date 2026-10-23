using System.Collections.Immutable;
using System.Diagnostics;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Emit;
using Microsoft.CodeAnalysis.Text;

namespace Ara.CompileServer;

/// <summary>
/// Everything one module generation was built from. Immutable: a compile produces the next
/// state inside an <see cref="Outcome"/>, and <see cref="ModuleSession.Commit"/> adopts it
/// only once the whole reload (a dependency cascade) succeeded.
/// </summary>
internal sealed record ModuleState(
    CSharpCompilation User,
    Compilation Final,
    ImmutableDictionary<string, SourceText> Generated,
    ImmutableDictionary<string, SyntaxTree> Trees,
    ImmutableDictionary<string, string> Texts,
    EmitBaseline? Baseline,
    bool Hot,
    uint Generation,
    // Hash of the module's reference assembly; null when unknown (an L2 delta may have changed it).
    string? Surface,
    // The surface of each referenced module this generation's code was compiled against.
    ImmutableDictionary<string, string?> DepSurfaces,
    // The reference in use for each referenced module (its final compilation).
    ImmutableDictionary<string, MetadataReference> DepRefs)
{
    /// <summary>Paths compare case-insensitively (Windows file system).</summary>
    internal static readonly StringComparer Paths = StringComparer.OrdinalIgnoreCase;

    static readonly System.Runtime.CompilerServices.ConditionalWeakTable<Compilation, MetadataReference> References = [];

    /// <summary>How modules that reference this one see it: a reference to its final compilation (one per compilation).</summary>
    internal MetadataReference Reference => References.GetValue(Final, c => c.ToMetadataReference());
}

/// <summary>What a compile produced.</summary>
internal enum OutcomeKind
{
    /// <summary>Nothing to send (no change, or a trivia-only change: commit adopts the new text).</summary>
    None,
    /// <summary>Errors: diagnostics are sent, nothing changes.</summary>
    Errors,
    /// <summary>An EnC delta (L1 or L2).</summary>
    Delta,
    /// <summary>A full image (initial load, L3, a rebuilt dependent).</summary>
    Image,
    /// <summary>A dependent reloaded unchanged against its dependencies' new generations.</summary>
    Relink,
    /// <summary>L4: the change cannot be applied in this process.</summary>
    Restart,
}

/// <summary>
/// The result of one compile of one module. Holds the next <see cref="ModuleState"/>; nothing
/// is applied until <see cref="ModuleSession.Commit"/>.
/// </summary>
internal sealed class Outcome(ModuleSession module, OutcomeKind kind)
{
    internal readonly ModuleSession Module = module;
    internal readonly OutcomeKind Kind = kind;
    internal ModuleState? Next;
    /// <summary><c>AbiConst.Reload*</c> for the host's report.</summary>
    internal uint ReloadKind;
    internal Escalation Escalation;
    internal ModuleImage? Image;
    internal ModuleDelta? Delta;
    internal int Changed;
    /// <summary>Diagnostics to send (errors for <see cref="OutcomeKind.Errors"/>, otherwise warnings and infos).</summary>
    internal List<Diagnostic> Problems = [];
    /// <summary>Host-coded diagnostics to send with <see cref="Problems"/>.</summary>
    internal List<(string Code, string Severity, string Message)> Extra = [];
    internal Phases Timing = new(0, 0);

    internal int ErrorCount => Problems.Count(d => d.Severity == DiagnosticSeverity.Error) + Extra.Count(e => e.Severity == "error");
}

/// <summary>
/// Phase timings of one compile: <see cref="Mark"/> charges the time since the previous mark
/// to a phase. The detect phase is file event → compile start.
/// </summary>
internal sealed class Phases(long tEvent, long tStart)
{
    long _t = tStart;
    internal readonly long TEvent = tEvent;
    internal uint Detect = tEvent == 0 ? 0 : CompileTiming.Micros(tEvent, tStart);
    internal uint Parse, Generate, Emit, Analyze;

    /// <summary>Microseconds since the previous mark.</summary>
    internal uint Mark()
    {
        var now = Stopwatch.GetTimestamp();
        var us = CompileTiming.Micros(_t, now);
        _t = now;
        return us;
    }

    /// <summary>The wire form; server time runs from the file event (or compile start) to now.</summary>
    internal CompileTiming ToTiming() =>
        new(Detect, Parse, Generate, Emit, Analyze, CompileTiming.Micros(TEvent == 0 ? _t : TEvent, Stopwatch.GetTimestamp()));

    public override string ToString() =>
        $"detect {Detect / 1000.0:F1}, parse {Parse / 1000.0:F1}, generate {Generate / 1000.0:F1}, emit {Emit / 1000.0:F1}, analyze {Analyze / 1000.0:F1} ms";
}

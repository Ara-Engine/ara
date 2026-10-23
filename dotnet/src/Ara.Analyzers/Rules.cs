using Microsoft.CodeAnalysis;

namespace Ara.Analyzers;

/// <summary>
/// The Ara analyzer rules (R10 D4). IDs are stable: never renumber or reuse one. The
/// generator owns ARA0700-0799 and the host ARA0900-0999.
/// </summary>
public static class Rules
{
    static DiagnosticDescriptor Rule(string id, string title, string message, string category, DiagnosticSeverity severity, string description) =>
        new(id, title, message, category, severity, isEnabledByDefault: true, description: description);

    /// <summary>A Unity API or convention that means something else (or nothing) in Ara.</summary>
    public static readonly DiagnosticDescriptor UnityIsm = Rule("ARA0001", "Unity-ism",
        "{0}", "Ara.Api", DiagnosticSeverity.Error,
        "Ara reuses Unity's concepts but renames what behaves differently, so a Unity habit fails to compile instead of meaning something else. The code fix rewrites it to the Ara equivalent.");

    /// <summary>A mutable static in a gameplay module.</summary>
    public static readonly DiagnosticDescriptor MutableStatic = Rule("ARA0100", "Mutable static state",
        "Static field '{0}' is mutable gameplay state; it survives reloads and play sessions and is invisible to save, replay and netcode. Use a [Resource] struct (or make it readonly/const).",
        "Ara.State", DiagnosticSeverity.Error,
        "Gameplay state lives in ECS components and resources only.");

    /// <summary>A Behaviour instance field that is not a [Transient] cache.</summary>
    public static readonly DiagnosticDescriptor BehaviourField = Rule("ARA0101", "Behaviour field without [Transient]",
        "Field '{0}' on Behaviour '{1}' would hold state outside the ECS; make it a [Property] partial property, or mark it [Transient] if it is a presentation cache",
        "Ara.State", DiagnosticSeverity.Error,
        "Behaviour objects are recreated on reload and hold no authoritative state.");

    /// <summary>Non-determinism in [Simulation] code.</summary>
    public static readonly DiagnosticDescriptor NonDeterminism = Rule("ARA0200", "Non-deterministic simulation code",
        "{0} is not allowed in [Simulation] code: simulation must be deterministic (fixed tick, RNG from the context, no wall-clock, no I/O, no threads)",
        "Ara.Determinism", DiagnosticSeverity.Error,
        "Prediction, replay and lockstep tests need identical results from identical inputs.");

    /// <summary>A heap allocation in tick-reachable code.</summary>
    public static readonly DiagnosticDescriptor TickAllocation = Rule("ARA0300", "Allocation on the tick path",
        "{0} allocates on the tick path; prefer structs, spans, stackalloc or pooled buffers",
        "Ara.Performance", DiagnosticSeverity.Warning,
        "Systems, behaviour callbacks and commands run every tick; allocations there turn into GC pauses.");

    /// <summary>An API that breaks under NativeAOT or trimming.</summary>
    public static readonly DiagnosticDescriptor AotUnsafe = Rule("ARA0400", "NativeAOT/trim-unsafe API",
        "{0} does not work in NativeAOT builds (shipping, iOS); use generated code or static references instead",
        "Ara.Aot", DiagnosticSeverity.Error,
        "Shipping builds are NativeAOT: no runtime code generation, no dynamic loading, no unannotated reflection.");

    static DiagnosticDescriptor Api(string id, string title, string description) => Rule(id, title,
        "`{0}` is not available to {1} modules: {2}", "Ara.Api", DiagnosticSeverity.Error, description);

    /// <summary>An API outside the module profile's allowlist (see <see cref="ApiProfiles"/>).</summary>
    public static readonly DiagnosticDescriptor ApiNotAllowed = Api("ARA0500", "API not on the allowlist",
        "Gameplay modules compile against an allowlist: the language runtime, Ara and module types. Editor modules (profile = \"editor\") get a broader one.");

    /// <summary>File-system access.</summary>
    public static readonly DiagnosticDescriptor ApiFileSystem = Api("ARA0501", "File system access",
        "Gameplay code has no file system: data comes in through assets, state lives in components.");

    /// <summary>Network access.</summary>
    public static readonly DiagnosticDescriptor ApiNetwork = Api("ARA0502", "Network access",
        "Gameplay code has no sockets or HTTP; replication is the engine's.");

    /// <summary>Reflection, dynamic code or assembly loading.</summary>
    public static readonly DiagnosticDescriptor ApiReflection = Api("ARA0503", "Reflection or dynamic code",
        "Reflection and dynamic code break NativeAOT and reload; generated code covers registration and serialization.");

    /// <summary>Threads, locks, tasks or async.</summary>
    public static readonly DiagnosticDescriptor ApiThreading = Api("ARA0504", "Threads, locks or tasks",
        "Gameplay code runs on the engine's schedule; systems already run in parallel over chunks.");

    /// <summary>Native interop.</summary>
    public static readonly DiagnosticDescriptor ApiInterop = Api("ARA0505", "Native interop",
        "P/Invoke, Marshal and native libraries are the engine's; modules call Ara APIs.");

    /// <summary>Process state: console, environment, GC, debugger, other processes.</summary>
    public static readonly DiagnosticDescriptor ApiProcess = Api("ARA0506", "Process or environment access",
        "The process belongs to the editor or the player; use Log for output and the Tick for time. The code fix rewrites console output to Log.");

    /// <summary>The descriptor for an area.</summary>
    public static DiagnosticDescriptor For(ApiArea area) => area switch
    {
        ApiArea.FileSystem => ApiFileSystem,
        ApiArea.Network => ApiNetwork,
        ApiArea.Reflection => ApiReflection,
        ApiArea.Threading => ApiThreading,
        ApiArea.Interop => ApiInterop,
        ApiArea.Process => ApiProcess,
        _ => ApiNotAllowed,
    };
}

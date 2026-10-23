using System;
using System.Collections.Generic;

namespace Ara.Analyzers;

/// <summary>Why an API is refused; each area has its own diagnostic (ARA0500-ARA0506).</summary>
public enum ApiArea
{
    /// <summary>On the allowlist.</summary>
    Allowed,
    /// <summary>ARA0500: not on the profile's allowlist.</summary>
    NotAllowed,
    /// <summary>ARA0501: files and directories.</summary>
    FileSystem,
    /// <summary>ARA0502: sockets, HTTP, DNS.</summary>
    Network,
    /// <summary>ARA0503: reflection, dynamic code and loading.</summary>
    Reflection,
    /// <summary>ARA0504: threads, locks, tasks and async.</summary>
    Threading,
    /// <summary>ARA0505: native interop (P/Invoke, Marshal, native libraries).</summary>
    Interop,
    /// <summary>ARA0506: the process and its environment (console, environment, GC, debugger, other processes).</summary>
    Process,
}

/// <summary>
/// The API allowlists for C# code Ara compiles, in the shape of s&amp;box's
/// <c>Sandbox.Access</c> rules, by profile:
/// <list type="bullet">
/// <item><b>gameplay</b> (default for modules): the language runtime (primitives, strings,
/// math, spans, collections, LINQ, numerics and SIMD, text, globalization, buffers,
/// <c>Unsafe</c>/<c>MemoryMarshal</c> for hot loops) and every Ara and module type. No files,
/// network, reflection, threads or tasks, native interop, console or process state: state goes
/// through the ECS, I/O through engine services, parallelism through systems.</item>
/// <item><b>editor</b> (<c>profile = "editor"</c>): everything gameplay has, plus files, network,
/// processes, console, tasks and threads, read-only reflection, XML, JSON and cryptography.
/// Still refused: dynamic code and assembly loading (they break reload), native interop, and
/// ending or crashing the editor process.</item>
/// <item><b>agent</b> (<c>ara_run</c> scripts, checked on IL by the worker): the language runtime,
/// JSON and tasks for <c>await</c>, and <c>Ara.Agent</c>; no <c>Unsafe</c>.</item>
/// </list>
/// Rules apply to <c>System.*</c> and <c>Microsoft.*</c> namespaces; Ara, module and user types
/// are always allowed. This file is plain C# shared by the analyzer (netstandard2.0) and the
/// <c>ara_run</c> worker's IL check.
/// </summary>
public static class ApiProfiles
{
    /// <summary>Gameplay modules (the default).</summary>
    public const string Gameplay = "gameplay";
    /// <summary>Editor modules.</summary>
    public const string Editor = "editor";
    /// <summary><c>ara_run</c> agent scripts.</summary>
    public const string Agent = "agent";

    // Namespaces (and their sub-namespaces unless listed exactly) every profile may use.
    static readonly string[] CoreNamespaces =
    [
        "System.Collections", "System.Collections.Generic", "System.Collections.Immutable", "System.Collections.ObjectModel",
        "System.Collections.Specialized", "System.Collections.Frozen", "System.Linq", "System.Numerics", "System.Text",
        "System.Text.RegularExpressions", "System.Text.Unicode", "System.Globalization", "System.Buffers", "System.Buffers.Binary",
        "System.Buffers.Text", "System.Runtime.CompilerServices", "System.Diagnostics.CodeAnalysis", "System.Diagnostics.Contracts",
    ];

    // Gameplay adds SIMD intrinsics for hot loops.
    static readonly string[] GameplayNamespaces = ["System.Runtime.Intrinsics", "System.Runtime.Intrinsics.X86", "System.Runtime.Intrinsics.Arm"];

    // Agents add JSON and tasks (scripts are async).
    static readonly string[] AgentNamespaces = ["System.Text.Json", "System.Text.Json.Nodes", "System.Threading.Tasks"];

    /// <summary>Types of the <c>System</c> namespace on every allowlist (plus <c>*Exception</c>, <c>*Attribute</c> and delegate/tuple families).</summary>
    static readonly HashSet<string> SystemTypes = new(StringComparer.Ordinal)
    {
        "Object", "String", "Boolean", "Char", "SByte", "Byte", "Int16", "UInt16", "Int32", "UInt32", "Int64", "UInt64", "Int128", "UInt128",
        "Half", "Single", "Double", "Decimal", "IntPtr", "UIntPtr", "Void", "ValueType", "Enum", "Array", "Nullable", "Nullable`1",
        "Span`1", "ReadOnlySpan`1", "Memory`1", "ReadOnlyMemory`1", "ArraySegment`1", "Index", "Range", "Math", "MathF", "BitConverter",
        "Convert", "HashCode", "Buffer", "Random", "Guid", "DateTime", "DateTimeOffset", "DateTimeKind", "DayOfWeek", "TimeSpan",
        "DateOnly", "TimeOnly", "Version", "StringComparison", "StringComparer", "StringSplitOptions", "MidpointRounding",
        "Base64FormattingOptions", "MemoryExtensions", "FormattableString", "IFormattable", "IFormatProvider", "ISpanFormattable",
        "IUtf8SpanFormattable", "IParsable`1", "ISpanParsable`1", "IUtf8SpanParsable`1", "IComparable", "IComparable`1", "IEquatable`1",
        "IDisposable", "IAsyncDisposable", "ICloneable", "IObservable`1", "IObserver`1", "Lazy`1", "WeakReference", "WeakReference`1",
        "Delegate", "MulticastDelegate", "EventArgs", "EventHandler", "EventHandler`1", "Predicate`1", "Comparison`1", "Converter`2",
        "Type", "RuntimeTypeHandle", "RuntimeFieldHandle", "RuntimeMethodHandle", "Tuple", "ValueTuple", "AttributeTargets",
        "StringNormalizationExtensions", "CharEnumerator", "UriKind", "Uri",
    };

    /// <summary>In-memory types of <c>System.IO</c> gameplay may use (no files).</summary>
    static readonly HashSet<string> MemoryIo = new(StringComparer.Ordinal)
    {
        "Stream", "MemoryStream", "UnmanagedMemoryStream", "BinaryReader", "BinaryWriter", "StringReader", "StringWriter",
        "TextReader", "TextWriter", "SeekOrigin", "EndOfStreamException", "InvalidDataException", "IOException",
    };

    /// <summary>Types of <c>System.Runtime.InteropServices</c> that are memory tools or layout attributes, not interop.</summary>
    static readonly HashSet<string> InteropAllowed = new(StringComparer.Ordinal)
    {
        "MemoryMarshal", "CollectionsMarshal", "StructLayoutAttribute", "LayoutKind", "FieldOffsetAttribute", "InAttribute",
        "OutAttribute", "OptionalAttribute",
    };

    /// <summary>Types of <c>System.Diagnostics</c> gameplay may use: assertions, debugger display attributes, timing.</summary>
    static readonly HashSet<string> DiagnosticsAllowed = new(StringComparer.Ordinal)
    {
        "Debug", "ConditionalAttribute", "DebuggerDisplayAttribute", "DebuggerBrowsableAttribute", "DebuggerBrowsableState",
        "DebuggerHiddenAttribute", "DebuggerStepThroughAttribute", "DebuggerNonUserCodeAttribute", "DebuggerTypeProxyAttribute",
        "DebuggableAttribute", "Stopwatch", "UnreachableException",
    };

    /// <summary><c>System</c> types refused with a specific area (editor allows them unless listed in <see cref="EditorDenied"/>).</summary>
    static readonly Dictionary<string, ApiArea> SystemDenied = new(StringComparer.Ordinal)
    {
        ["Console"] = ApiArea.Process, ["Environment"] = ApiArea.Process, ["AppDomain"] = ApiArea.Process,
        ["AppContext"] = ApiArea.Process, ["GC"] = ApiArea.Process, ["Activator"] = ApiArea.Reflection,
        ["OperatingSystem"] = ApiArea.Process, ["TimeProvider"] = ApiArea.NotAllowed,
    };

    /// <summary>Members refused on otherwise allowed types (<c>Type.Member</c>).</summary>
    static readonly Dictionary<string, ApiArea> MembersDenied = new(StringComparer.Ordinal)
    {
        ["System.Type.GetType"] = ApiArea.Reflection, ["System.Type.GetMethod"] = ApiArea.Reflection, ["System.Type.GetMethods"] = ApiArea.Reflection,
        ["System.Type.GetField"] = ApiArea.Reflection, ["System.Type.GetFields"] = ApiArea.Reflection, ["System.Type.GetProperty"] = ApiArea.Reflection,
        ["System.Type.GetProperties"] = ApiArea.Reflection, ["System.Type.GetMember"] = ApiArea.Reflection, ["System.Type.GetMembers"] = ApiArea.Reflection,
        ["System.Type.InvokeMember"] = ApiArea.Reflection, ["System.Type.MakeGenericType"] = ApiArea.Reflection, ["System.Type.GetConstructor"] = ApiArea.Reflection,
        ["System.Type.GetConstructors"] = ApiArea.Reflection, ["System.Type.GetEvent"] = ApiArea.Reflection, ["System.Type.GetEvents"] = ApiArea.Reflection,
        ["System.Type.GetNestedType"] = ApiArea.Reflection, ["System.Type.GetInterface"] = ApiArea.Reflection, ["System.Type.get_Assembly"] = ApiArea.Reflection,
        ["System.Type.get_Module"] = ApiArea.Reflection, ["System.Delegate.DynamicInvoke"] = ApiArea.Reflection,
        ["System.Delegate.CreateDelegate"] = ApiArea.Reflection, ["System.Delegate.get_Method"] = ApiArea.Reflection,
        ["System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject"] = ApiArea.Reflection,
        ["System.Runtime.CompilerServices.RuntimeHelpers.RunClassConstructor"] = ApiArea.Reflection,
        ["System.Diagnostics.Debug.Write"] = ApiArea.Process, ["System.Diagnostics.Debug.WriteLine"] = ApiArea.Process,
        ["System.Diagnostics.Debug.WriteIf"] = ApiArea.Process, ["System.Diagnostics.Debug.WriteLineIf"] = ApiArea.Process,
        ["System.Diagnostics.Debug.Print"] = ApiArea.Process,
    };

    /// <summary>Refused even in editor modules: they break reload, NativeAOT or the editor process itself.</summary>
    static readonly Dictionary<string, ApiArea> EditorDenied = new(StringComparer.Ordinal)
    {
        ["System.Environment.Exit"] = ApiArea.Process, ["System.Environment.FailFast"] = ApiArea.Process,
        ["System.AppDomain"] = ApiArea.Process, ["System.Runtime.Loader"] = ApiArea.Reflection, ["System.Reflection.Emit"] = ApiArea.Reflection,
        ["System.Reflection.Assembly.Load"] = ApiArea.Reflection, ["System.Reflection.Assembly.LoadFrom"] = ApiArea.Reflection,
        ["System.Reflection.Assembly.LoadFile"] = ApiArea.Reflection, ["System.Reflection.Assembly.UnsafeLoadFrom"] = ApiArea.Reflection,
        ["System.Reflection.MethodBase.Invoke"] = ApiArea.Reflection, ["System.Linq.Expressions.LambdaExpression.Compile"] = ApiArea.Reflection,
    };

    /// <summary>Agent scripts: refused although their namespace is allowed (mirrors the first <c>ara_run</c> cut).</summary>
    static readonly HashSet<string> AgentDeniedTypes = new(StringComparer.Ordinal)
    {
        "System.Runtime.CompilerServices.Unsafe", "System.Threading.Tasks.TaskScheduler", "System.Threading.Tasks.Parallel",
    };

    /// <summary>Single types agents may use outside their namespaces.</summary>
    static readonly HashSet<string> AgentTypes = new(StringComparer.Ordinal)
    {
        "System.Diagnostics.DebuggableAttribute", "System.Diagnostics.DebuggableAttribute+DebuggingModes", "System.Diagnostics.DebuggerDisplayAttribute",
        "System.Diagnostics.DebuggerBrowsableAttribute", "System.Diagnostics.DebuggerBrowsableState", "System.Diagnostics.DebuggerHiddenAttribute",
        "System.Diagnostics.DebuggerStepThroughAttribute", "System.Diagnostics.DebuggerNonUserCodeAttribute", "System.Threading.CancellationToken",
    };

    /// <summary>
    /// Classifies a use of <paramref name="type"/> (metadata name, nested types joined by <c>+</c>)
    /// in namespace <paramref name="ns"/>, or of its <paramref name="member"/> (method, property
    /// accessor <c>get_X</c>, field or event name), under <paramref name="profile"/>.
    /// </summary>
    public static ApiArea Classify(string profile, string ns, string type, string? member = null)
    {
        if (!IsUnder(ns, "System") && !IsUnder(ns, "Microsoft")) return ApiArea.Allowed;
        var full = ns.Length == 0 ? type : ns + "." + type;
        var outer = type.Split('+')[0];
        if (profile == Agent) return ClassifyAgent(ns, full, outer, member);
        if (profile == Editor)
        {
            if (Find(EditorDenied, full, member, ns) is { } denied) return denied;
            if (IsUnder(ns, "System.Runtime.InteropServices") && !InteropAllowed.Contains(outer)) return ApiArea.Interop;
            return ApiArea.Allowed;
        }
        if (member is not null && MembersDenied.TryGetValue(full + "." + member, out var m)) return m;
        if (ns == "System")
        {
            if (SystemDenied.TryGetValue(outer, out var area)) return area;
            return SystemTypes.Contains(outer) || IsFamily(outer) ? ApiArea.Allowed : ApiArea.NotAllowed;
        }
        if (IsUnder(ns, "System.IO")) return ns == "System.IO" && MemoryIo.Contains(outer) ? ApiArea.Allowed : ApiArea.FileSystem;
        if (IsUnder(ns, "System.Net")) return ApiArea.Network;
        if (IsUnder(ns, "System.Reflection") || IsUnder(ns, "System.Runtime.Loader") || IsUnder(ns, "System.Linq.Expressions")
            || IsUnder(ns, "System.Dynamic") || IsUnder(ns, "Microsoft.CSharp"))
            return ns == "System.Reflection" && outer.EndsWith("Attribute", StringComparison.Ordinal) ? ApiArea.Allowed : ApiArea.Reflection;
        if (IsUnder(ns, "System.Threading")) return ApiArea.Threading;
        if (IsUnder(ns, "System.Runtime.InteropServices")) return InteropAllowed.Contains(outer) ? ApiArea.Allowed : ApiArea.Interop;
        if (ns == "System.Diagnostics") return DiagnosticsAllowed.Contains(outer) ? ApiArea.Allowed : ApiArea.Process;
        if (ns == "System.ComponentModel") return outer.EndsWith("Attribute", StringComparison.Ordinal) || outer == "EditorBrowsableState" ? ApiArea.Allowed : ApiArea.NotAllowed;
        if (ns == "System.Text.RegularExpressions" || ns.StartsWith("System.Text.Json", StringComparison.Ordinal))
            return ns == "System.Text.RegularExpressions" ? ApiArea.Allowed : ApiArea.NotAllowed;
        foreach (var allowed in CoreNamespaces) if (IsUnder(ns, allowed)) return ApiArea.Allowed;
        foreach (var allowed in GameplayNamespaces) if (ns == allowed) return ApiArea.Allowed;
        return ApiArea.NotAllowed;
    }

    static ApiArea ClassifyAgent(string ns, string full, string outer, string? member)
    {
        if (AgentDeniedTypes.Contains(full)) return ApiArea.NotAllowed;
        if (member is not null && MembersDenied.TryGetValue(full + "." + member, out var m) && m == ApiArea.Reflection) return m;
        if (AgentTypes.Contains(full)) return ApiArea.Allowed;
        if (ns == "System") return SystemTypes.Contains(outer) || IsFamily(outer) ? ApiArea.Allowed : ApiArea.NotAllowed;
        foreach (var allowed in CoreNamespaces) if (ns == allowed) return ApiArea.Allowed;
        foreach (var allowed in AgentNamespaces) if (ns == allowed) return ApiArea.Allowed;
        return ApiArea.NotAllowed;
    }

    static ApiArea? Find(Dictionary<string, ApiArea> rules, string full, string? member, string ns)
    {
        if (member is not null && rules.TryGetValue(full + "." + member, out var a)) return a;
        if (rules.TryGetValue(full, out a)) return a;
        for (var n = ns; n.Length > 0; n = n.LastIndexOf('.') > 0 ? n.Substring(0, n.LastIndexOf('.')) : "")
            if (rules.TryGetValue(n, out a)) return a;
        return null;
    }

    /// <summary>Exceptions, attributes, delegates and tuples of the <c>System</c> namespace.</summary>
    static bool IsFamily(string name) =>
        name.EndsWith("Exception", StringComparison.Ordinal) || name.EndsWith("Attribute", StringComparison.Ordinal)
        || name.StartsWith("Func`", StringComparison.Ordinal) || name.StartsWith("Action", StringComparison.Ordinal)
        || name.StartsWith("ValueTuple`", StringComparison.Ordinal) || name.StartsWith("Tuple`", StringComparison.Ordinal);

    static bool IsUnder(string ns, string root) =>
        ns == root || (ns.Length > root.Length && ns[root.Length] == '.' && ns.StartsWith(root, StringComparison.Ordinal));

    /// <summary>What to use instead, per area (the diagnostic's second sentence).</summary>
    public static string Advice(ApiArea area) => area switch
    {
        ApiArea.FileSystem => "gameplay code has no file system; load data through assets and keep state in components",
        ApiArea.Network => "gameplay code has no network access; replication is the engine's (D4)",
        ApiArea.Reflection => "gameplay code has no reflection or dynamic code; use generated code and static references",
        ApiArea.Threading => "gameplay code runs on the engine's schedule; use systems (they run in parallel) instead of threads, locks or tasks",
        ApiArea.Interop => "native code is the engine's; call it through Ara APIs",
        ApiArea.Process => "gameplay code does not touch the process; use Log for output and the Tick for time",
        _ => "use Ara or core runtime APIs (collections, math, text) instead, or move the code into an editor module",
    };
}

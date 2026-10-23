using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;

namespace Ara.RunWorker;

/// <summary>
/// The IL whitelist for <c>ara_run</c> scripts: a first cut in the shape of s&amp;box's
/// <c>Sandbox.Access</c> (rules by area, checked against every type and member the
/// compiled assembly references), written for agent scripts.
/// </summary>
/// <remarks>
/// Agent scripts run as trusted-but-fallible code (R10 D5): the threat is mistakes, not
/// malice. The whitelist keeps them on the one write path (typed engine calls) and away
/// from ambient authority: no files, network, processes, threads, reflection, native
/// interop or dynamic loading. Allowed: the language runtime (primitives, strings,
/// collections, LINQ, math, text, JSON, tasks for <c>await</c>) and <c>Ara.Agent</c>.
/// The process boundary, the job object and the kill-on-limit supervisor are the
/// containment; this check is the review gate.
/// </remarks>
internal static class Whitelist
{
    /// <summary>Namespaces whose types scripts may use, unless denied below.</summary>
    static readonly string[] AllowedNamespaces =
    [
        "System.Collections", "System.Collections.Generic", "System.Collections.ObjectModel", "System.Linq",
        "System.Numerics", "System.Text", "System.Text.Json", "System.Text.Json.Nodes", "System.Text.RegularExpressions",
        "System.Globalization", "System.Threading.Tasks", "System.Runtime.CompilerServices",
        "System.Diagnostics.CodeAnalysis", "Ara.Agent",
    ];

    /// <summary>Types in <c>System</c> scripts may use (the namespace itself holds ambient authority too).</summary>
    static readonly HashSet<string> AllowedSystemTypes =
    [
        "Object", "String", "Boolean", "Char", "Byte", "SByte", "Int16", "UInt16", "Int32", "UInt32", "Int64", "UInt64",
        "Single", "Double", "Decimal", "IntPtr", "UIntPtr", "Half", "Int128", "UInt128", "Void", "ValueType", "Enum",
        "Math", "MathF", "Array", "Span`1", "ReadOnlySpan`1", "Memory`1", "ReadOnlyMemory`1", "Nullable`1", "Index", "Range",
        "Tuple", "ValueTuple", "DateTime", "DateTimeOffset", "TimeSpan", "DateOnly", "TimeOnly", "Guid", "Random", "Convert",
        "BitConverter", "StringComparison", "StringComparer", "StringSplitOptions", "MidpointRounding", "HashCode",
        "IComparable", "IComparable`1", "IEquatable`1", "IDisposable", "IAsyncDisposable", "IFormattable", "IFormatProvider",
        "Exception", "ArgumentException", "ArgumentNullException", "ArgumentOutOfRangeException", "InvalidOperationException",
        "NotSupportedException", "NotImplementedException", "FormatException", "KeyNotFoundException", "OverflowException",
        "IndexOutOfRangeException", "Attribute", "AttributeUsageAttribute", "AttributeTargets", "ObsoleteAttribute",
        "FlagsAttribute", "ParamArrayAttribute", "Delegate", "MulticastDelegate", "Type", "RuntimeTypeHandle",
        "RuntimeFieldHandle", "MemoryExtensions", "FormattableString", "Lazy`1",
    ];

    /// <summary>Single types allowed outside the allowed namespaces (compiler-emitted attributes).</summary>
    static readonly HashSet<string> AllowedTypes =
    [
        "System.Diagnostics.DebuggableAttribute", "System.Diagnostics.DebuggableAttribute+DebuggingModes",
        "System.Diagnostics.DebuggerDisplayAttribute", "System.Diagnostics.DebuggerBrowsableAttribute",
        "System.Diagnostics.DebuggerBrowsableState", "System.Diagnostics.DebuggerHiddenAttribute",
        "System.Diagnostics.DebuggerStepThroughAttribute", "System.Diagnostics.DebuggerNonUserCodeAttribute",
        "System.Threading.CancellationToken",
    ];

    /// <summary>Members denied even on allowed types (reflection through <c>Type</c>).</summary>
    static readonly HashSet<string> DeniedMembers =
    [
        "System.Type.GetType", "System.Type.GetMethod", "System.Type.GetMethods", "System.Type.GetField", "System.Type.GetFields",
        "System.Type.GetProperty", "System.Type.GetProperties", "System.Type.GetMember", "System.Type.GetMembers",
        "System.Type.InvokeMember", "System.Type.MakeGenericType", "System.Type.GetConstructor", "System.Type.GetConstructors",
        "System.Type.get_Assembly", "System.Type.get_Module", "System.Runtime.CompilerServices.Unsafe",
        "System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject",
    ];

    /// <summary>Types denied even inside allowed namespaces.</summary>
    static readonly HashSet<string> DeniedTypes =
    [
        "System.Runtime.CompilerServices.Unsafe", "System.Threading.Tasks.TaskScheduler", "System.Threading.Tasks.Parallel",
    ];

    /// <summary>Every whitelist violation in an assembly image (empty when it passes).</summary>
    internal static List<string> Check(byte[] image)
    {
        var problems = new List<string>();
        using var pe = new PEReader(new MemoryStream(image));
        var md = pe.GetMetadataReader();
        if (md.GetTableRowCount(TableIndex.ImplMap) > 0) problems.Add("P/Invoke (DllImport) is not allowed");
        if (md.GetTableRowCount(TableIndex.ModuleRef) > 0) problems.Add("native module references are not allowed");
        foreach (var h in md.TypeReferences)
        {
            var name = FullName(md, h);
            if (!TypeAllowed(name)) problems.Add($"`{name}` is not on the agent whitelist");
        }
        foreach (var h in md.MemberReferences)
        {
            var m = md.GetMemberReference(h);
            var parent = m.Parent.Kind switch
            {
                HandleKind.TypeReference => FullName(md, (TypeReferenceHandle)m.Parent),
                HandleKind.TypeSpecification => SpecName(md, (TypeSpecificationHandle)m.Parent),
                _ => null,
            };
            if (parent is null) continue;
            var member = parent + "." + md.GetString(m.Name);
            if (DeniedMembers.Contains(member) || DeniedMembers.Contains(parent))
                problems.Add($"`{member}` is not on the agent whitelist");
        }
        return [.. problems.Distinct()];
    }

    static bool TypeAllowed(string full)
    {
        if (DeniedTypes.Contains(full)) return false;
        if (AllowedTypes.Contains(full)) return true;
        var outer = full.Split('+')[0];
        var dot = outer.LastIndexOf('.');
        var ns = dot < 0 ? "" : outer[..dot];
        var name = outer[(dot + 1)..];
        if (ns == "System") return AllowedSystemTypes.Contains(name) || name.StartsWith("Func`", StringComparison.Ordinal)
            || name.StartsWith("Action", StringComparison.Ordinal) || name.StartsWith("ValueTuple`", StringComparison.Ordinal)
            || name.StartsWith("Tuple`", StringComparison.Ordinal);
        return AllowedNamespaces.Contains(ns);
    }

    static string FullName(MetadataReader md, TypeReferenceHandle h)
    {
        var t = md.GetTypeReference(h);
        var name = md.GetString(t.Name);
        if (t.ResolutionScope.Kind == HandleKind.TypeReference)
            return FullName(md, (TypeReferenceHandle)t.ResolutionScope) + "+" + name;
        var ns = md.GetString(t.Namespace);
        return ns.Length == 0 ? name : ns + "." + name;
    }

    /// <summary>The generic type a member reference's parent instantiates (<c>List`1</c>), or null.</summary>
    static string? SpecName(MetadataReader md, TypeSpecificationHandle h)
    {
        var blob = md.GetBlobReader(md.GetTypeSpecification(h).Signature);
        if (blob.ReadSignatureTypeCode() != SignatureTypeCode.GenericTypeInstance) return null;
        blob.ReadSignatureTypeCode(); // class or value type
        var handle = blob.ReadTypeHandle();
        return handle.Kind == HandleKind.TypeReference ? FullName(md, (TypeReferenceHandle)handle) : null;
    }
}

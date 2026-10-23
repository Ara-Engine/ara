using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using Ara.Analyzers;

namespace Ara.RunWorker;

/// <summary>
/// The IL whitelist for <c>ara_run</c> scripts: the <c>agent</c> profile of
/// <see cref="ApiProfiles"/> (shared with the module analyzers), checked against every type
/// and member the compiled assembly references, in the shape of s&amp;box's <c>Sandbox.Access</c>.
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
            var (ns, name) = Name(md, h);
            if (ApiProfiles.Classify(ApiProfiles.Agent, ns, name) != ApiArea.Allowed)
                problems.Add($"`{Full(ns, name)}` is not on the agent whitelist");
        }
        foreach (var h in md.MemberReferences)
        {
            var m = md.GetMemberReference(h);
            (string Ns, string Name)? parent = m.Parent.Kind switch
            {
                HandleKind.TypeReference => Name(md, (TypeReferenceHandle)m.Parent),
                HandleKind.TypeSpecification => SpecName(md, (TypeSpecificationHandle)m.Parent),
                _ => null,
            };
            if (parent is not { } p) continue;
            var member = md.GetString(m.Name);
            if (ApiProfiles.Classify(ApiProfiles.Agent, p.Ns, p.Name, member) != ApiArea.Allowed)
                problems.Add($"`{Full(p.Ns, p.Name)}.{member}` is not on the agent whitelist");
        }
        return [.. problems.Distinct()];
    }

    static string Full(string ns, string name) => ns.Length == 0 ? name : ns + "." + name;

    /// <summary>Namespace and metadata name of a type reference (nested types joined by <c>+</c>).</summary>
    static (string Ns, string Name) Name(MetadataReader md, TypeReferenceHandle h)
    {
        var t = md.GetTypeReference(h);
        var name = md.GetString(t.Name);
        if (t.ResolutionScope.Kind == HandleKind.TypeReference)
        {
            var (ns, outer) = Name(md, (TypeReferenceHandle)t.ResolutionScope);
            return (ns, outer + "+" + name);
        }
        return (md.GetString(t.Namespace), name);
    }

    /// <summary>The generic type a member reference's parent instantiates (<c>List`1</c>), or null.</summary>
    static (string Ns, string Name)? SpecName(MetadataReader md, TypeSpecificationHandle h)
    {
        var blob = md.GetBlobReader(md.GetTypeSpecification(h).Signature);
        if (blob.ReadSignatureTypeCode() != SignatureTypeCode.GenericTypeInstance) return null;
        blob.ReadSignatureTypeCode(); // class or value type
        var handle = blob.ReadTypeHandle();
        return handle.Kind == HandleKind.TypeReference ? Name(md, (TypeReferenceHandle)handle) : null;
    }
}

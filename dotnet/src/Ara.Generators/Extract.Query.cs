using System.Collections.Generic;
using System.Linq;
using Microsoft.CodeAnalysis;

namespace Ara.Generators;

public static partial class Extract
{
    /// <summary>A <c>[Query]</c> method, consuming individual rows or a complete span batch.</summary>
    public static SystemModel? Query(GeneratorAttributeSyntaxContext ctx)
    {
        if (ctx.TargetSymbol is not IMethodSymbol m) return null;
        var diags = new List<DiagInfo>();
        var shell = ShellOf(m.ContainingType, diags);
        if (!m.IsStatic || !m.ReturnsVoid || m.IsGenericMethod)
            diags.Add(DiagInfo.At(Descriptors.BadSystem, m, $"`{m.Name}` must be a non-generic `static void` method to be a [Query] system"));
        var attr = ctx.Attributes[0];
        var order = 0;
        string? name = null;
        foreach (var kv in attr.NamedArguments)
        {
            if (kv.Key == "Order" && kv.Value.Value is int o) order = o;
            if (kv.Key == "Name" && kv.Value.Value is string n) name = n;
        }
        name ??= m.Name == "Run" ? m.ContainingType.Name : m.ContainingType.Name + "." + m.Name;
        var batch = m.Parameters.Any(p => SpanElement(p.Type, out _) is not null);
        var ps = new List<SystemParam>();
        foreach (var p in m.Parameters)
        {
            var refKind = p.RefKind switch { RefKind.Ref => "ref", RefKind.In or RefKind.RefReadOnlyParameter => "in", _ => "" };
            var ty = p.Type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat);
            if (SpanElement(p.Type, out var writable) is { } element)
            {
                if (p.RefKind != RefKind.None)
                    Bad(p, "batch spans are passed by value; Span<T> declares a write and ReadOnlySpan<T> declares a read");
                else if (Schema.IsAraType(element, "Entity"))
                {
                    if (writable) Bad(p, "entity identities are read-only; use ReadOnlySpan<Entity>");
                    else ps.Add(new("entity-span", element.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat), "", ""));
                }
                else if (!Schema.IsComponent(element, out var component))
                    Bad(p, "batch spans contain components or Entity; resources remain scalar parameters");
                else if (!element.IsUnmanagedType)
                    Bad(p, $"`{element.Name}` contains managed references and cannot be viewed as a native component span");
                else if (element is INamedTypeSymbol tag && !Schema.HasFields(tag))
                    Bad(p, $"`{element.Name}` has no fields (a tag); filter on it with [With<{element.Name}>] instead");
                else ps.Add(new("span", element.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat), writable ? "ref" : "in", component));
            }
            else if (Schema.IsAraType(p.Type, "Entity") && refKind != "ref") ps.Add(new("entity", ty, "", ""));
            else if (Schema.IsAraType(p.Type, "Tick") && refKind != "ref" && (!batch || p.RefKind != RefKind.Out))
                ps.Add(new("tick", ty, batch ? refKind : "in", ""));
            else if (Schema.IsAraType(p.Type, "Rng") && refKind == "ref") ps.Add(new("rng", ty, "ref", ""));
            else if (Schema.IsAraType(p.Type, "Commands") && (!batch || p.RefKind != RefKind.Out)) ps.Add(new("commands", ty, refKind, ""));
            else if (Schema.IsComponent(p.Type, out var c) && p.Type is INamedTypeSymbol tag && !Schema.HasFields(tag))
                diags.Add(DiagInfo.At(Descriptors.BadSystemParam, p,
                    $"`{p.Type.Name}` has no fields (a tag), so it has no column to read; filter on it with [With<{p.Type.Name}>] instead"));
            else if (Schema.IsComponent(p.Type, out c)) ps.Add(new("column", ty, refKind, c));
            else if (Schema.IsResource(p.Type, out var r) && (!batch || p.RefKind != RefKind.Out)) ps.Add(new("resource", ty, refKind, r));
            else
                diags.Add(DiagInfo.At(Descriptors.BadSystemParam, p, batch
                    ? $"parameter `{p.Name}` of system `{name}` has type `{p.Type.ToDisplayString()}`; batch systems take component spans, resources, ReadOnlySpan<Entity>, in Tick or ref Commands"
                    : $"parameter `{p.Name}` of system `{name}` has type `{p.Type.ToDisplayString()}`; systems take components (`ref`/`in`), resources, `Entity`, `in Tick`, `ref Rng` or `ref Commands`"));
        }
        if (batch)
        {
            foreach (var p in m.Parameters)
            {
                if (Schema.IsComponent(p.Type, out _) || Schema.IsAraType(p.Type, "Entity"))
                    Bad(p, "batch queries use spans for every component and ReadOnlySpan<Entity> for identities; scalar row parameters cannot be mixed with spans");
                else if (Schema.IsAraType(p.Type, "Rng"))
                    Bad(p, "a batch has no single entity RNG; use Rng.For(in tick, streamKey, entities[i]) with ReadOnlySpan<Entity>");
            }
            foreach (var duplicate in ps.Where(p => p.IsColumn).GroupBy(p => p.SchemaName).Where(g => g.Count() > 1))
                diags.Add(DiagInfo.At(Descriptors.BadSystemParam, m, $"batch query `{name}` declares component `{duplicate.Key}` more than once; declare one span for each component"));
        }
        if (!ps.Any(p => p.IsColumn))
            diags.Add(DiagInfo.At(Descriptors.BadSystem, m, $"system `{name}` needs at least one component parameter"));
        var with = Filters(m, "WithAttribute", diags);
        var without = Filters(m, "WithoutAttribute", diags);
        return new SystemModel(shell ?? new Shell(null, default, "", ""), m.Name, name, order, ps.ToEquatable(),
            with.ToEquatable(), without.ToEquatable(), diags.ToEquatable());

        void Bad(IParameterSymbol parameter, string message) =>
            diags.Add(DiagInfo.At(Descriptors.BadSystemParam, parameter, $"parameter `{parameter.Name}` of system `{name}`: {message}"));
    }

    static ITypeSymbol? SpanElement(ITypeSymbol type, out bool writable)
    {
        writable = false;
        if (type is not INamedTypeSymbol { Arity: 1, IsRefLikeType: true, ContainingNamespace.Name: "System", ContainingNamespace.ContainingNamespace.IsGlobalNamespace: true } span
            || span.Name is not ("Span" or "ReadOnlySpan")) return null;
        writable = span.Name == "Span";
        return span.TypeArguments[0];
    }

    static List<string> Filters(IMethodSymbol m, string attr, List<DiagInfo> diags)
    {
        var names = new List<string>();
        foreach (var a in Schema.Attrs(m, attr))
        {
            if (a.AttributeClass?.TypeArguments.FirstOrDefault() is { } t && Schema.IsComponent(t, out var n)) names.Add(n);
            else diags.Add(DiagInfo.At(Descriptors.BadSystemParam, m, $"[{attr.Replace("Attribute", "")}<T>] needs a component type"));
        }
        return names;
    }
}

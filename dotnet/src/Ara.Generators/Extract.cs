using System.Collections.Generic;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.Generators;

/// <summary>Turns attributed declarations into equatable pipeline models (with diagnostics).</summary>
public static class Extract
{
    /// <summary>Where <paramref name="t"/> lives, or null (plus <c>ARA0701</c>) if it or an enclosing type is not partial.</summary>
    public static Shell? ShellOf(INamedTypeSymbol t, List<DiagInfo> diags)
    {
        var containers = new List<string>();
        for (var c = t; c is not null; c = c.ContainingType)
        {
            var partial = c.DeclaringSyntaxReferences.All(r =>
                r.GetSyntax() is TypeDeclarationSyntax d && d.Modifiers.Any(SyntaxKind.PartialKeyword));
            if (!partial)
            {
                diags.Add(DiagInfo.At(Descriptors.NotPartial, c,
                    $"`{c.Name}` must be `partial` so the Ara generator can add its registration code"));
                return null;
            }
            if (!SymbolEqualityComparer.Default.Equals(c, t)) containers.Insert(0, Header(c));
        }
        var ns = t.ContainingNamespace.IsGlobalNamespace ? null : t.ContainingNamespace.ToDisplayString();
        return new Shell(ns, containers.ToEquatable(), Header(t), t.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat));
    }

    static string Header(INamedTypeSymbol t) => (t.IsRecord, t.TypeKind) switch
    {
        (true, TypeKind.Struct) => "partial record struct " + t.Name,
        (true, _) => "partial record " + t.Name,
        (_, TypeKind.Struct) => "partial struct " + t.Name,
        (_, TypeKind.Interface) => "partial interface " + t.Name,
        _ => "partial class " + t.Name,
    };

    /// <summary>A <c>[Component]</c>, <c>[Resource]</c> or <c>[Cue]</c> struct.</summary>
    public static DataModel? Data(GeneratorAttributeSyntaxContext ctx, string role)
    {
        if (ctx.TargetSymbol is not INamedTypeSymbol t) return null;
        var diags = new List<DiagInfo>();
        var defs = new List<TypeDef>();
        var shell = ShellOf(t, diags);
        var name = Schema.DataName(t, ctx.Attributes[0]);
        if (!Schema.IsTypeName(name))
            diags.Add(DiagInfo.At(Descriptors.NotBlittable, t, $"schema name `{name}` must be PascalCase letters and digits (no dots; the module is metadata)"));
        if (!t.IsUnmanagedType)
            diags.Add(DiagInfo.At(Descriptors.NotBlittable, t, $"`{t.Name}` must be an unmanaged struct (no reference-type fields) to live in ECS columns"));
        var fields = Schema.Fields(t);
        var kind = role switch { "Component" => "component", "Resource" => "resource", _ => "struct" };
        defs.Add(new TypeDef(name, Schema.StructJson(t, kind, fields, defs, diags)));
        return new DataModel(shell ?? new Shell(null, default, "", ""), role, name,
            fields.Select(f => f.Name).ToEquatable(), defs.ToEquatable(), diags.ToEquatable());
    }

    /// <summary>A <c>[Query]</c> method.</summary>
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
        var ps = new List<SystemParam>();
        foreach (var p in m.Parameters)
        {
            var refKind = p.RefKind switch { RefKind.Ref => "ref", RefKind.In or RefKind.RefReadOnlyParameter => "in", _ => "" };
            var ty = p.Type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat);
            if (Schema.IsAraType(p.Type, "Entity") && refKind != "ref") ps.Add(new("entity", ty, "", ""));
            else if (Schema.IsAraType(p.Type, "Tick") && refKind != "ref") ps.Add(new("tick", ty, "in", ""));
            else if (Schema.IsAraType(p.Type, "Rng") && refKind == "ref") ps.Add(new("rng", ty, "ref", ""));
            else if (Schema.IsAraType(p.Type, "Commands")) ps.Add(new("commands", ty, refKind, ""));
            else if (Schema.IsComponent(p.Type, out var c) && p.Type is INamedTypeSymbol tag && Schema.Fields(tag).Count == 0)
                diags.Add(DiagInfo.At(Descriptors.BadSystemParam, p,
                    $"`{p.Type.Name}` has no fields (a tag), so it has no column to read; filter on it with [With<{p.Type.Name}>] instead"));
            else if (Schema.IsComponent(p.Type, out c)) ps.Add(new("column", ty, refKind, c));
            else if (Schema.IsResource(p.Type, out var r)) ps.Add(new("resource", ty, refKind, r));
            else
                diags.Add(DiagInfo.At(Descriptors.BadSystemParam, p,
                    $"parameter `{p.Name}` of system `{name}` has type `{p.Type.ToDisplayString()}`; systems take components (`ref`/`in`), resources, `Entity`, `in Tick`, `ref Rng` or `ref Commands`"));
        }
        if (!ps.Any(p => p.Kind == "column"))
            diags.Add(DiagInfo.At(Descriptors.BadSystem, m, $"system `{name}` needs at least one component parameter"));
        var with = Filters(m, "WithAttribute", diags);
        var without = Filters(m, "WithoutAttribute", diags);
        return new SystemModel(shell ?? new Shell(null, default, "", ""), m.Name, name, order, ps.ToEquatable(),
            with.ToEquatable(), without.ToEquatable(), diags.ToEquatable());
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

    /// <summary>A class deriving from <c>Ara.Behaviour</c>, or null for other classes.</summary>
    public static BehaviourModel? Behaviour(GeneratorSyntaxContext ctx)
    {
        if (ctx.SemanticModel.GetDeclaredSymbol(ctx.Node) is not INamedTypeSymbol t || t.IsAbstract) return null;
        if (!DerivesFromBehaviour(t)) return null;
        // One model per type: only the first partial declaration produces it.
        // The syntax provider visits declarations with a base list. A helper
        // partial in another file may precede the declaration spelling Behaviour.
        if (t.DeclaringSyntaxReferences.Select(r => r.GetSyntax())
            .OfType<ClassDeclarationSyntax>().FirstOrDefault(n => n.BaseList is not null) != ctx.Node) return null;
        var diags = new List<DiagInfo>();
        var defs = new List<TypeDef>();
        var shell = ShellOf(t, diags);
        if (!t.InstanceConstructors.Any(c => c.Parameters.Length == 0 && c.DeclaredAccessibility is Accessibility.Public or Accessibility.Internal))
            diags.Add(DiagInfo.At(Descriptors.BadBehaviour, t, $"behaviour `{t.Name}` needs a public parameterless constructor (the runtime creates one per entity)"));
        var props = new List<PropModel>();
        var members = new List<Member>();
        foreach (var p in t.GetMembers().OfType<IPropertySymbol>())
        {
            if (Schema.Attr(p, "PropertyAttribute") is null) continue;
            if (!p.IsPartialDefinition || p.IsStatic)
            {
                diags.Add(DiagInfo.At(Descriptors.BadBehaviour, p, $"[Property] `{p.Name}` must be a `partial` instance property without a body; the generator stores it in the behaviour's ECS column"));
                continue;
            }
            var syntax = p.DeclaringSyntaxReferences.First().GetSyntax() as PropertyDeclarationSyntax;
            var modifiers = syntax is null ? "public partial" : string.Join(" ", syntax.Modifiers.Select(x => x.Text));
            var set = p.SetMethod is null ? null : p.SetMethod.IsInitOnly ? "init" : "set";
            var doc = Docs.Summary(p);
            props.Add(new PropModel(modifiers, p.Type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat), p.Name, p.GetMethod is not null, set, doc));
            members.Add(new Member(p.Name, p.Type, doc, p.GetAttributes(), p));
        }
        var summary = Docs.Summary(t);
        defs.Add(new TypeDef(t.Name, Schema.StructJson(t, "component", members, defs, diags,
            doc: summary.Length > 0 ? summary : null)));
        return new BehaviourModel(shell ?? new Shell(null, default, "", ""), t.Name, props.ToEquatable(), defs.ToEquatable(), diags.ToEquatable());
    }

    static bool DerivesFromBehaviour(INamedTypeSymbol t)
    {
        for (var b = t.BaseType; b is not null; b = b.BaseType)
            if (Schema.IsAraType(b, "Behaviour")) return true;
        return false;
    }

    /// <summary>A <c>[Command]</c> class.</summary>
    public static CommandModel? Command(GeneratorAttributeSyntaxContext ctx)
    {
        if (ctx.TargetSymbol is not INamedTypeSymbol t) return null;
        var diags = new List<DiagInfo>();
        var defs = new List<TypeDef>();
        var shell = ShellOf(t, diags);
        var name = ctx.Attributes[0].ConstructorArguments.FirstOrDefault().Value as string ?? t.Name;
        var exec = t.GetMembers("Execute").OfType<IMethodSymbol>().FirstOrDefault();
        var ps = new List<CommandParam>();
        var members = new List<Member>();
        if (exec is null || !exec.IsStatic || !Schema.IsAraType(exec.ReturnType, "CommandResult")
            || exec.Parameters.Length == 0 || exec.Parameters[0].RefKind != RefKind.Ref
            || !Schema.IsAraType(exec.Parameters[0].Type, "CommandContext"))
        {
            diags.Add(DiagInfo.At(Descriptors.BadCommand, (ISymbol?)exec ?? t,
                $"command `{name}` needs `public static CommandResult Execute(ref CommandContext ctx, Entity target, ...)`"));
        }
        else
        {
            var target = exec.Parameters.Skip(1).FirstOrDefault(p => Schema.IsAraType(p.Type, "Entity"));
            foreach (var p in exec.Parameters.Skip(1))
            {
                var refKind = p.RefKind switch { RefKind.Ref => "ref", RefKind.In or RefKind.RefReadOnlyParameter => "in", _ => "" };
                var ty = p.Type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat);
                if (refKind.Length > 0 && Schema.IsComponent(p.Type, out var comp))
                {
                    if (target is null)
                        diags.Add(DiagInfo.At(Descriptors.BadCommand, p, $"component parameter `{p.Name}` needs an `Entity` parameter before it: components bind to that entity"));
                    ps.Add(new CommandParam("component", ty, p.Name, refKind, comp));
                    continue;
                }
                if (refKind.Length > 0)
                {
                    diags.Add(DiagInfo.At(Descriptors.BadCommand, p, $"`{p.Name}`: only component parameters may be `ref`/`in`"));
                    continue;
                }
                var doc = Docs.Param(exec, p.Name);
                var def = p.HasExplicitDefaultValue ? DefaultLiteral(p.ExplicitDefaultValue) : null;
                members.Add(new Member(p.Name, p.Type, doc, p.GetAttributes(), p, Required: def is null, Default: def));
                ps.Add(new CommandParam("value", ty, p.Name, "", ""));
            }
        }
        var summary = Docs.Summary(t);
        if (summary.Length == 0 && exec is not null) summary = Docs.Summary(exec);
        var paramsType = t.Name + "Params";
        defs.Add(new TypeDef(paramsType, Schema.StructJson(t, "params", members, defs, diags,
            doc: $"Params of the `{name}` command.")));
        if (summary.Length == 0) Schema.RequireDoc(t, diags);
        var examples = Schema.Attrs(t, "ExampleAttribute")
            .Select(a => a.ConstructorArguments.FirstOrDefault().Value as string)
            .Where(s => s is not null).Select(s => s!).ToList();
        return new CommandModel(shell ?? new Shell(null, default, "", ""), name, summary, paramsType, ps.ToEquatable(),
            Schema.ExposeJson(t), examples.ToEquatable(), defs.ToEquatable(), diags.ToEquatable());
    }

    static string? DefaultLiteral(object? v) => v switch
    {
        null => null,
        bool b => b ? "true" : "false",
        string s => new Json().String(s).ToString(),
        _ => System.Convert.ToDouble(v, System.Globalization.CultureInfo.InvariantCulture).ToString("R", System.Globalization.CultureInfo.InvariantCulture),
    };

    /// <summary>
    /// Modules linked from referenced assemblies (<c>[assembly: AraModule]</c>), for the NativeAOT
    /// entry point, in dependency order: a module comes after every module it references, so
    /// registration sees the types it uses. Ties sort by name (deterministic output).
    /// </summary>
    public static List<LinkedModule> LinkedModules(Compilation c)
    {
        var modules = new Dictionary<IAssemblySymbol, LinkedModule>(SymbolEqualityComparer.Default);
        foreach (var asm in c.SourceModule.ReferencedAssemblySymbols)
        {
            foreach (var a in asm.GetAttributes())
            {
                if (a.AttributeClass?.Name != "AraModuleAttribute" || a.ConstructorArguments.Length != 2) continue;
                if (a.ConstructorArguments[0].Value is string n && a.ConstructorArguments[1].Value is INamedTypeSymbol entry)
                    modules[asm] = new LinkedModule(n, entry.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat));
            }
        }
        var list = new List<LinkedModule>();
        var done = new HashSet<IAssemblySymbol>(SymbolEqualityComparer.Default);
        void Visit(IAssemblySymbol asm)
        {
            if (!done.Add(asm)) return;
            foreach (var dep in asm.Modules.SelectMany(m => m.ReferencedAssemblySymbols).Where(modules.ContainsKey).OrderBy(d => modules[d].Name, System.StringComparer.Ordinal))
                Visit(dep);
            list.Add(modules[asm]);
        }
        foreach (var asm in modules.Keys.OrderBy(a => modules[a].Name, System.StringComparer.Ordinal)) Visit(asm);
        return list;
    }
}

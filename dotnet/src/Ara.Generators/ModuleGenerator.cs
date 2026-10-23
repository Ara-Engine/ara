using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.Generators;

/// <summary>
/// The Ara module generator. For each gameplay module (one assembly) it emits:
/// <list type="bullet">
/// <item>registration partials for <c>[Component]</c>, <c>[Resource]</c> and <c>[Cue]</c> structs;</item>
/// <item>chunk-batched entry points for <c>[Query]</c> systems;</item>
/// <item>the data component and property bodies of every <c>Behaviour</c>;</item>
/// <item>params structs and entry points for <c>[Command]</c> classes;</item>
/// <item>the module's <c>Register</c> method, which hands schema types, layouts and access sets to the host;</item>
/// <item>with the MSBuild property <c>AraNativeEntry=true</c>, the NativeAOT <c>ara_script_init</c> export.</item>
/// </list>
/// Output depends only on declarations, so a method-body edit leaves it unchanged (the
/// compile server relies on that to keep such edits on the L1 path).
/// </summary>
[Generator(LanguageNames.CSharp)]
public sealed class ModuleGenerator : IIncrementalGenerator
{
    /// <inheritdoc/>
    public void Initialize(IncrementalGeneratorInitializationContext context)
    {
        var components = Data(context, "Ara.ComponentAttribute", "Component");
        var resources = Data(context, "Ara.ResourceAttribute", "Resource");
        var cues = Data(context, "Ara.CueAttribute", "Cue");
        var systems = context.SyntaxProvider.ForAttributeWithMetadataName("Ara.QueryAttribute",
            static (n, _) => n is MethodDeclarationSyntax, static (c, _) => Extract.Query(c))
            .Where(static m => m is not null).Select(static (m, _) => m!);
        var behaviours = context.SyntaxProvider.CreateSyntaxProvider(
            static (n, _) => n is ClassDeclarationSyntax { BaseList: not null },
            static (c, _) => Extract.Behaviour(c))
            .Where(static m => m is not null).Select(static (m, _) => m!);
        var commands = context.SyntaxProvider.ForAttributeWithMetadataName("Ara.CommandAttribute",
            static (n, _) => n is ClassDeclarationSyntax, static (c, _) => Extract.Command(c))
            .Where(static m => m is not null).Select(static (m, _) => m!);

        context.RegisterSourceOutput(components, static (spc, m) => EmitData(spc, m));
        context.RegisterSourceOutput(resources, static (spc, m) => EmitData(spc, m));
        context.RegisterSourceOutput(cues, static (spc, m) => EmitData(spc, m));
        context.RegisterSourceOutput(systems, static (spc, m) =>
        {
            if (Report(spc, m.Diags)) spc.AddSource(Hint(m.Shell, "system." + m.Method), Emit.Query(m));
        });
        context.RegisterSourceOutput(behaviours, static (spc, m) =>
        {
            if (Report(spc, m.Diags)) spc.AddSource(Hint(m.Shell, "behaviour"), Emit.Behaviour(m));
        });
        context.RegisterSourceOutput(commands, static (spc, m) =>
        {
            if (Report(spc, m.Diags)) spc.AddSource(Hint(m.Shell, "command"), Emit.Command(m));
        });

        var nativeEntry = context.AnalyzerConfigOptionsProvider.Select(static (o, _) =>
            o.GlobalOptions.TryGetValue("build_property.AraNativeEntry", out var v) && v.Equals("true", System.StringComparison.OrdinalIgnoreCase));
        var module = context.CompilationProvider.Select(static (c, _) =>
            (Name: c.AssemblyName ?? "Module", Linked: Extract.LinkedModules(c).ToEquatable()));
        var all = components.Collect().Combine(resources.Collect()).Combine(cues.Collect())
            .Combine(systems.Collect()).Combine(behaviours.Collect()).Combine(commands.Collect())
            .Combine(module).Combine(nativeEntry);
        context.RegisterSourceOutput(all, static (spc, x) =>
        {
            var (((((((comps, res), cues), sys), behs), cmds), mod), native) = x;
            var data = comps.Concat(res).Concat(cues).ToList();
            var models = data.Select(d => (d.Diags, d.Defs)).Concat(sys.Select(s => (s.Diags, default(EquatableArray<TypeDef>))))
                .Concat(behs.Select(b => (b.Diags, b.Defs))).Concat(cmds.Select(c => (c.Diags, c.Defs))).ToList();
            if (models.Any(m => m.Diags.Length > 0)) return; // diagnostics were reported per declaration
            var types = new Dictionary<string, string>();
            foreach (var def in models.SelectMany(m => m.Item2))
            {
                if (types.TryGetValue(def.Name, out var existing) && existing != def.Json)
                {
                    spc.ReportDiagnostic(Diagnostic.Create(Descriptors.NotBlittable, Location.None,
                        $"two different types are registered as `{def.Name}`; schema names must be unique in a module"));
                    return;
                }
                types[def.Name] = def.Json;
            }
            var declares = data.Count + sys.Length + behs.Length + cmds.Length > 0;
            if (declares || native)
                spc.AddSource("AraModule.g.cs", Emit.Module(mod.Name, declares, data, sys, behs, cmds, types, native, mod.Linked.ToList()));
        });
    }

    static IncrementalValuesProvider<DataModel> Data(IncrementalGeneratorInitializationContext context, string attribute, string role) =>
        context.SyntaxProvider.ForAttributeWithMetadataName(attribute,
            static (n, _) => n is StructDeclarationSyntax or RecordDeclarationSyntax,
            (c, _) => Extract.Data(c, role))
            .Where(static m => m is not null).Select(static (m, _) => m!);

    static void EmitData(SourceProductionContext spc, DataModel m)
    {
        if (Report(spc, m.Diags)) spc.AddSource(Hint(m.Shell, m.Role.ToLowerInvariant()), Emit.Data(m));
    }

    /// <summary>Reports diagnostics; true if there were none.</summary>
    static bool Report(SourceProductionContext spc, EquatableArray<DiagInfo> diags)
    {
        foreach (var d in diags) spc.ReportDiagnostic(d.ToDiagnostic());
        return diags.Length == 0;
    }

    static string Hint(Shell s, string suffix) =>
        new string(s.FullName.Replace("global::", "").Select(ch => char.IsLetterOrDigit(ch) || ch == '.' ? ch : '_').ToArray())
        + "." + suffix + ".g.cs";
}

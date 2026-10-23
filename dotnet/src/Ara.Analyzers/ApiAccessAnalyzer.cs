using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.Analyzers;

/// <summary>
/// ARA0500-ARA0506: every framework API a module uses must be on its profile's allowlist
/// (<see cref="ApiProfiles"/>). The profile comes from the MSBuild property
/// <c>AraModuleProfile</c> (<c>gameplay</c> by default, <c>editor</c>), which the compile
/// server sets from <c>ara.module.toml</c>. Calls, constructions, member references,
/// <c>await</c>, <c>lock</c> and <c>extern</c>/P/Invoke methods are checked. Uses that ARA0400
/// (AOT) or, in <c>[Simulation]</c> code, ARA0200 (determinism) already report are left to them.
/// The code fix rewrites console and debug output to <c>Log</c>, <c>Environment.NewLine</c> to
/// <c>"\n"</c>, and drops <c>GC.Collect()</c> calls.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class ApiAccessAnalyzer : DiagnosticAnalyzer
{
    /// <summary>The MSBuild property naming the module's profile.</summary>
    public const string ProfileOption = "build_property.AraModuleProfile";

    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics =>
        [Rules.ApiNotAllowed, Rules.ApiFileSystem, Rules.ApiNetwork, Rules.ApiReflection, Rules.ApiThreading, Rules.ApiInterop, Rules.ApiProcess];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            var profile = start.Options.AnalyzerConfigOptionsProvider.GlobalOptions.TryGetValue(ProfileOption, out var p) && p == ApiProfiles.Editor
                ? ApiProfiles.Editor
                : ApiProfiles.Gameplay;
            start.RegisterOperationAction(c => Member(c, profile), OperationKind.Invocation, OperationKind.ObjectCreation,
                OperationKind.PropertyReference, OperationKind.FieldReference, OperationKind.EventReference, OperationKind.MethodReference);
            if (profile == ApiProfiles.Gameplay)
            {
                start.RegisterOperationAction(c => Construct(c, "await"), OperationKind.Await);
                start.RegisterOperationAction(c => Construct(c, "lock"), OperationKind.Lock);
            }
            start.RegisterSymbolAction(Extern, SymbolKind.Method);
        });
    }

    static void Member(OperationAnalysisContext c, string profile)
    {
        var (member, name) = c.Operation switch
        {
            IInvocationOperation i => ((ISymbol)i.TargetMethod, i.TargetMethod.Name),
            IObjectCreationOperation { Constructor: { } ctor } => (ctor, ".ctor"),
            IPropertyReferenceOperation p => (p.Property, (IsWrite(p) ? "set_" : "get_") + p.Property.Name),
            IFieldReferenceOperation f => (f.Field, f.Field.Name),
            IEventReferenceOperation e => (e.Event, e.Event.Name),
            IMethodReferenceOperation m => (m.Method, m.Method.Name),
            _ => (null, ""),
        };
        if (member?.ContainingType is not { } type) return;
        if (member is IMethodSymbol method && AotAnalyzer.Covers(method)) return;
        if (Symbols.InSimulation(c.ContainingSymbol) && DeterminismAnalyzer.Covers(type, member.Name)) return;
        var (ns, metadata) = Names(type);
        var area = ApiProfiles.Classify(profile, ns, metadata, name);
        if (area == ApiArea.Allowed) return;
        // A denied receiver (Console.Out) is reported once, on the call it feeds.
        if (c.Operation.Parent is IInvocationOperation outer && outer.Instance == c.Operation)
        {
            Report(c, outer, area, profile, $"{type.Name}.{member.Name}", Fix(type, member.Name, outer));
            return;
        }
        if (c.Operation is IInvocationOperation { Instance: { } receiver } && Denied(receiver, profile)) return;
        Report(c, c.Operation, area, profile, $"{type.Name}.{(name == ".ctor" ? "new" : member.Name)}", Fix(type, member.Name, c.Operation));
    }

    /// <summary>True if the receiver itself is reported (the call is reported there).</summary>
    static bool Denied(IOperation receiver, string profile) =>
        receiver is IPropertyReferenceOperation { Property.ContainingType: { } t } p
        && ApiProfiles.Classify(profile, Names(t).Ns, Names(t).Metadata, "get_" + p.Property.Name) != ApiArea.Allowed;

    static bool IsWrite(IPropertyReferenceOperation p) =>
        p.Parent is ISimpleAssignmentOperation a && a.Target == p;

    static (string Ns, string Metadata) Names(INamedTypeSymbol type)
    {
        var t = type.OriginalDefinition;
        var metadata = t.MetadataName;
        for (var outer = t.ContainingType; outer is not null; outer = outer.ContainingType) metadata = outer.MetadataName + "+" + metadata;
        var root = t;
        while (root.ContainingType is not null) root = root.ContainingType;
        return (root.ContainingNamespace.IsGlobalNamespace ? "" : root.ContainingNamespace.ToDisplayString(), metadata);
    }

    /// <summary>The mechanical fix for a use, if there is one: <c>(fix, to)</c> diagnostic properties.</summary>
    static ImmutableDictionary<string, string?> Fix(INamedTypeSymbol type, string member, IOperation at)
    {
        var props = ImmutableDictionary<string, string?>.Empty;
        var (ns, name) = (type.ContainingNamespace.ToDisplayString(), type.Name);
        if (at is IInvocationOperation { TargetMethod: { } m } call)
        {
            var target = (m.ContainingType.ContainingNamespace.ToDisplayString(), m.ContainingType.Name, m.Name);
            var log = target switch
            {
                ("System", "Console", "WriteLine" or "Write") => "Info",
                ("System.Diagnostics", "Debug", "WriteLine" or "Write" or "Print") => "Info",
                ("System.IO", "TextWriter", "WriteLine" or "Write") when (ns, name, member) is ("System", "Console", "Error") => "Error",
                ("System.IO", "TextWriter", "WriteLine" or "Write") when (ns, name, member) is ("System", "Console", "Out") => "Info",
                _ => null,
            };
            if (log is not null && call.Arguments.All(a => a.ArgumentKind != ArgumentKind.ParamArray || a.Value is IArrayCreationOperation))
                return props.Add("fix", "log").Add("to", log);
            if (target is ("System", "GC", "Collect") && call.Parent is IExpressionStatementOperation)
                return props.Add("fix", "remove");
        }
        if ((ns, name, member) is ("System", "Environment", "NewLine")) return props.Add("fix", "newline");
        return props;
    }

    static void Report(OperationAnalysisContext c, IOperation at, ApiArea area, string profile, string what,
        ImmutableDictionary<string, string?>? props = null) =>
        c.ReportDiagnostic(Diagnostic.Create(Rules.For(area), at.Syntax.GetLocation(), props, what, profile, ApiProfiles.Advice(area)));

    static void Construct(OperationAnalysisContext c, string what)
    {
        if (Symbols.InSimulation(c.ContainingSymbol) && what == "await") return; // ARA0200 reports it
        c.ReportDiagnostic(Diagnostic.Create(Rules.ApiThreading, c.Operation.Syntax.GetLocation(), what, ApiProfiles.Gameplay,
            ApiProfiles.Advice(ApiArea.Threading)));
    }

    /// <summary>P/Invoke and other <c>extern</c> methods are native interop in every profile.</summary>
    static void Extern(SymbolAnalysisContext c)
    {
        var m = (IMethodSymbol)c.Symbol;
        var native = m.IsExtern || m.GetAttributes().Any(a => a.AttributeClass?.Name is "DllImportAttribute" or "LibraryImportAttribute");
        if (!native || m.Locations.FirstOrDefault(l => l.IsInSource) is not { } at) return;
        c.ReportDiagnostic(Diagnostic.Create(Rules.ApiInterop, at, $"extern {m.Name}", "Ara",
            ApiProfiles.Advice(ApiArea.Interop)));
    }
}

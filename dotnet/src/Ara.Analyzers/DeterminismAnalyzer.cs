using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.Analyzers;

/// <summary>
/// ARA0200: non-determinism in <c>[Simulation]</c> code (a type or the whole assembly):
/// async and tasks, wall-clock time, <c>System.Random</c>, I/O, threads, and lambdas that
/// capture state inside commands.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class DeterminismAnalyzer : DiagnosticAnalyzer
{
    /// <summary>Types whose every use is non-deterministic, by metadata name.</summary>
    static readonly ImmutableHashSet<string> BannedTypes = ImmutableHashSet.Create(
        "System.Random", "System.Threading.Thread", "System.Threading.ThreadPool", "System.Threading.Timer",
        "System.Threading.Tasks.Task", "System.Threading.Tasks.Task`1", "System.Threading.Tasks.ValueTask",
        "System.Threading.Tasks.ValueTask`1", "System.Threading.Tasks.Parallel", "System.Diagnostics.Stopwatch",
        "System.Console");

    /// <summary>Members that read the wall clock or other ambient state.</summary>
    static readonly ImmutableHashSet<string> BannedMembers = ImmutableHashSet.Create(
        "System.DateTime.Now", "System.DateTime.UtcNow", "System.DateTime.Today",
        "System.DateTimeOffset.Now", "System.DateTimeOffset.UtcNow",
        "System.Environment.TickCount", "System.Environment.TickCount64", "System.Guid.NewGuid");

    static readonly ImmutableArray<string> BannedNamespaces = ["System.IO", "System.Net"];

    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Rules.NonDeterminism];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            start.RegisterOperationAction(Member, OperationKind.Invocation, OperationKind.PropertyReference,
                OperationKind.FieldReference, OperationKind.ObjectCreation);
            start.RegisterOperationAction(Await, OperationKind.Await);
            start.RegisterOperationAction(Lambda, OperationKind.AnonymousFunction);
            start.RegisterSyntaxNodeAction(Async, SyntaxKind.MethodDeclaration, SyntaxKind.LocalFunctionStatement);
        });
    }

    static void Report(OperationAnalysisContext c, Location at, string what) =>
        c.ReportDiagnostic(Diagnostic.Create(Rules.NonDeterminism, at, what));

    static bool InNamespace(INamespaceSymbol? ns, string name)
    {
        for (var n = ns; n is not null && !n.IsGlobalNamespace; n = n.ContainingNamespace)
            if (n.ToDisplayString() == name) return true;
        return false;
    }

    static void Member(OperationAnalysisContext c)
    {
        if (!Symbols.InSimulation(c.ContainingSymbol)) return;
        var (member, type) = c.Operation switch
        {
            IInvocationOperation i => ((ISymbol)i.TargetMethod, i.TargetMethod.ContainingType),
            IPropertyReferenceOperation p => (p.Property, p.Property.ContainingType),
            IFieldReferenceOperation f => (f.Field, f.Field.ContainingType),
            IObjectCreationOperation o when o.Type is INamedTypeSymbol t => ((ISymbol?)o.Constructor ?? t, t),
            _ => (null, null),
        };
        if (member is null || type is null) return;
        var typeName = type.OriginalDefinition.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat).Replace("global::", "");
        var metadata = type.ContainingNamespace.ToDisplayString() + "." + type.MetadataName;
        var memberName = typeName + "." + member.Name;
        if (BannedMembers.Contains(memberName))
            Report(c, c.Operation.Syntax.GetLocation(), $"`{type.Name}.{member.Name}`");
        else if (BannedTypes.Contains(metadata))
            Report(c, c.Operation.Syntax.GetLocation(), $"`{type.Name}`");
        else if (BannedNamespaces.Any(n => InNamespace(type.ContainingNamespace, n)))
            Report(c, c.Operation.Syntax.GetLocation(), $"`{type.ContainingNamespace.ToDisplayString()}.{type.Name}` (I/O)");
    }

    static void Await(OperationAnalysisContext c)
    {
        if (Symbols.InSimulation(c.ContainingSymbol)) Report(c, c.Operation.Syntax.GetLocation(), "`await`");
    }

    static void Async(SyntaxNodeAnalysisContext c)
    {
        var modifiers = c.Node switch
        {
            MethodDeclarationSyntax m => m.Modifiers,
            LocalFunctionStatementSyntax l => l.Modifiers,
            _ => default,
        };
        var async = modifiers.FirstOrDefault(m => m.IsKind(SyntaxKind.AsyncKeyword));
        if (async.IsKind(SyntaxKind.AsyncKeyword) && c.SemanticModel.GetDeclaredSymbol(c.Node) is { } s && Symbols.InSimulation(s))
            c.ReportDiagnostic(Diagnostic.Create(Rules.NonDeterminism, async.GetLocation(), "an `async` method"));
    }

    /// <summary>Lambdas in commands may not capture: commands are re-run for prediction and must not carry state.</summary>
    static void Lambda(OperationAnalysisContext c)
    {
        var lambda = (IAnonymousFunctionOperation)c.Operation;
        var owner = lambda.Symbol.ContainingType;
        if (owner is null || !Symbols.HasAra(owner, "CommandAttribute") || !Symbols.InSimulation(owner)) return;
        if (c.Operation.SemanticModel?.AnalyzeDataFlow(c.Operation.Syntax) is { Succeeded: true } flow && flow.Captured.Length > 0)
            Report(c, c.Operation.Syntax.GetLocation(), $"a lambda capturing `{flow.Captured[0].Name}` in a command");
    }
}

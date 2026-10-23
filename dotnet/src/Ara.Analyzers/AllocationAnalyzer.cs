using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.Analyzers;

/// <summary>
/// ARA0300 (warning): heap allocations directly in tick-path code: <c>[Query]</c>
/// systems, behaviour <c>OnTick</c>/<c>OnFrame</c>, and command <c>Execute</c> (including
/// lambdas and local functions inside them). Flags class and array creation, LINQ,
/// non-static lambdas, boxing, and string building.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class AllocationAnalyzer : DiagnosticAnalyzer
{
    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Rules.TickAllocation];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            start.RegisterOperationAction(Check, OperationKind.ObjectCreation, OperationKind.ArrayCreation,
                OperationKind.Invocation, OperationKind.AnonymousFunction, OperationKind.Conversion,
                OperationKind.InterpolatedString, OperationKind.Binary);
        });
    }

    /// <summary>The tick entry a symbol's code runs in, looking through lambdas and local functions.</summary>
    static bool OnTickPath(ISymbol? s)
    {
        while (s is IMethodSymbol { MethodKind: MethodKind.AnonymousFunction or MethodKind.LocalFunction } m) s = m.ContainingSymbol;
        return s is IMethodSymbol method && Symbols.IsTickEntry(method);
    }

    static void Check(OperationAnalysisContext c)
    {
        // Attribute arguments are metadata, not code that runs on the tick.
        if (!OnTickPath(c.ContainingSymbol) || c.Operation.Syntax.FirstAncestorOrSelf<AttributeSyntax>() is not null) return;
        var what = c.Operation switch
        {
            IObjectCreationOperation { Type: { IsReferenceType: true } t } => $"`new {t.Name}(...)`",
            IArrayCreationOperation a => $"`new {(a.Type as IArrayTypeSymbol)?.ElementType.Name}[...]`",
            IInvocationOperation { TargetMethod.ContainingType: { Name: "Enumerable", ContainingNamespace: { Name: "Linq", ContainingNamespace.Name: "System" } } } i => $"LINQ `{i.TargetMethod.Name}`",
            IAnonymousFunctionOperation f when !f.Symbol.IsStatic => "a non-static lambda (a closure or delegate)",
            IConversionOperation conv when conv.GetConversion().IsBoxing => $"boxing `{conv.Operand.Type?.Name}`",
            IInterpolatedStringOperation => "string interpolation",
            IBinaryOperation { OperatorKind: BinaryOperatorKind.Add, Type.SpecialType: SpecialType.System_String } => "string concatenation",
            _ => null,
        };
        if (what is not null) c.ReportDiagnostic(Diagnostic.Create(Rules.TickAllocation, c.Operation.Syntax.GetLocation(), what));
    }
}

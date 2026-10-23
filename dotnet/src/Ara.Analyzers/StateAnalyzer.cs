using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.Analyzers;

/// <summary>
/// ARA0100 (mutable statics) and ARA0101 (Behaviour fields without <c>[Transient]</c>):
/// gameplay state lives in ECS components and resources only, so reload, save, replay
/// and netcode see all of it.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class StateAnalyzer : DiagnosticAnalyzer
{
    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Rules.MutableStatic, Rules.BehaviourField];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            start.RegisterSymbolAction(Field, SymbolKind.Field);
            start.RegisterSymbolAction(Property, SymbolKind.Property);
        });
    }

    static void Field(SymbolAnalysisContext c)
    {
        var f = (IFieldSymbol)c.Symbol;
        if (f.IsImplicitlyDeclared || f.IsConst) return;
        if (f.IsStatic)
        {
            if (!f.IsReadOnly) c.ReportDiagnostic(Diagnostic.Create(Rules.MutableStatic, f.Locations[0], f.Name));
            return;
        }
        if (Symbols.IsBehaviour(f.ContainingType) && !Symbols.HasAra(f, "TransientAttribute"))
            c.ReportDiagnostic(Diagnostic.Create(Rules.BehaviourField, f.Locations[0], f.Name, f.ContainingType.Name));
    }

    static void Property(SymbolAnalysisContext c)
    {
        var p = (IPropertySymbol)c.Symbol;
        // A static auto-property with a setter is a mutable static field in disguise.
        if (p.IsStatic && p.SetMethod is not null && !p.IsImplicitlyDeclared && IsAuto(p))
            c.ReportDiagnostic(Diagnostic.Create(Rules.MutableStatic, p.Locations[0], p.Name));
    }

    static bool IsAuto(IPropertySymbol p) =>
        p.ContainingType.GetMembers().OfType<IFieldSymbol>().Any(f => SymbolEqualityComparer.Default.Equals(f.AssociatedSymbol, p));
}

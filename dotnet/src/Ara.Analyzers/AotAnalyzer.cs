using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.Analyzers;

/// <summary>
/// ARA0400: APIs that break under NativeAOT or trimming (shipping and iOS builds are
/// NativeAOT): dynamic assembly loading, <c>Reflection.Emit</c>, <c>Activator</c> with a
/// runtime type, string-based type and member lookup, <c>MakeGeneric*</c>, reflective
/// invoke, <c>Expression.Compile</c> and <c>dynamic</c>.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class AotAnalyzer : DiagnosticAnalyzer
{
    /// <summary>Banned methods as <c>Type.Method</c> (any overload).</summary>
    static readonly ImmutableHashSet<string> BannedMethods = ImmutableHashSet.Create(
        "System.Reflection.Assembly.Load", "System.Reflection.Assembly.LoadFrom", "System.Reflection.Assembly.LoadFile",
        "System.Reflection.Assembly.UnsafeLoadFrom", "System.Type.GetType", "System.Type.MakeGenericType",
        "System.Type.GetMethod", "System.Type.GetMethods", "System.Type.GetProperty", "System.Type.GetField",
        "System.Type.InvokeMember", "System.Reflection.MethodInfo.MakeGenericMethod", "System.Reflection.MethodBase.Invoke",
        "System.Linq.Expressions.LambdaExpression.Compile", "System.Linq.Expressions.Expression`1.Compile",
        "System.AppDomain.Load");

    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Rules.AotUnsafe];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            start.RegisterOperationAction(Check, OperationKind.Invocation, OperationKind.ObjectCreation,
                OperationKind.DynamicInvocation, OperationKind.DynamicMemberReference, OperationKind.DynamicObjectCreation);
        });
    }

    static string Name(ITypeSymbol t) => t.ContainingNamespace.ToDisplayString() + "." + t.MetadataName;

    static void Check(OperationAnalysisContext c)
    {
        string? what = null;
        switch (c.Operation)
        {
            case IInvocationOperation i:
                var type = i.TargetMethod.ContainingType;
                var key = Name(type.OriginalDefinition) + "." + i.TargetMethod.Name;
                if (BannedMethods.Contains(key)) what = $"`{type.Name}.{i.TargetMethod.Name}`";
                else if (Name(type) == "System.Activator" && i.TargetMethod.Name == "CreateInstance" && !i.TargetMethod.IsGenericMethod)
                    what = "`Activator.CreateInstance(Type)`";
                else if (type.ContainingNamespace.ToDisplayString().StartsWith("System.Reflection.Emit", System.StringComparison.Ordinal))
                    what = $"`System.Reflection.Emit.{type.Name}`";
                else if (Name(type) == "System.Runtime.Loader.AssemblyLoadContext")
                    what = "`AssemblyLoadContext`";
                break;
            case IObjectCreationOperation { Type: { } t } when t.ContainingNamespace.ToDisplayString().StartsWith("System.Reflection.Emit", System.StringComparison.Ordinal):
                what = $"`System.Reflection.Emit.{t.Name}`";
                break;
            case IDynamicInvocationOperation or IDynamicMemberReferenceOperation or IDynamicObjectCreationOperation:
                what = "`dynamic`";
                break;
        }
        if (what is not null) c.ReportDiagnostic(Diagnostic.Create(Rules.AotUnsafe, c.Operation.Syntax.GetLocation(), what));
    }
}

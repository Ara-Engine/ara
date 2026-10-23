using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.Analyzers;

/// <summary>
/// ARA0001: Unity habits that compile to nothing (or something else) in Ara. Each
/// diagnostic carries a <c>fix</c> property telling <c>UnityFix</c> what to rewrite:
/// <c>using</c>, <c>base</c>, <c>magic</c> (with <c>to</c>), <c>getcomponent</c>,
/// <c>member</c> (with <c>to</c>), <c>deltatime</c>, <c>debuglog</c>.
/// </summary>
[DiagnosticAnalyzer(LanguageNames.CSharp)]
public sealed class UnityAnalyzer : DiagnosticAnalyzer
{
    /// <summary>Unity lifecycle methods and their Ara overrides.</summary>
    public static readonly ImmutableDictionary<string, string> MagicMethods = new[]
    {
        ("Update", "OnTick"), ("FixedUpdate", "OnTick"), ("LateUpdate", "OnFrame"),
        ("Start", "OnSpawn"), ("Awake", "OnSpawn"),
    }.ToImmutableDictionary(p => p.Item1, p => p.Item2);

    static readonly ImmutableHashSet<string> VectorTypes = ImmutableHashSet.Create("Vector2", "Vector3", "Vector4", "Quaternion");

    /// <inheritdoc/>
    public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Rules.UnityIsm];

    /// <inheritdoc/>
    public override void Initialize(AnalysisContext context)
    {
        context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
        context.EnableConcurrentExecution();
        context.RegisterCompilationStartAction(start =>
        {
            if (!Symbols.IsGameplayModule(start.Compilation)) return;
            start.RegisterSyntaxNodeAction(Using, SyntaxKind.UsingDirective);
            start.RegisterSyntaxNodeAction(BaseType, SyntaxKind.SimpleBaseType);
            start.RegisterSyntaxNodeAction(Magic, SyntaxKind.MethodDeclaration);
            start.RegisterSyntaxNodeAction(Invocation, SyntaxKind.InvocationExpression);
            start.RegisterSyntaxNodeAction(Member, SyntaxKind.SimpleMemberAccessExpression);
        });
    }

    static void Report(SyntaxNodeAnalysisContext c, Location at, string message, params (string Key, string Value)[] props) =>
        c.ReportDiagnostic(Diagnostic.Create(Rules.UnityIsm, at,
            props.ToImmutableDictionary(p => p.Key, p => (string?)p.Value), message));

    static void Using(SyntaxNodeAnalysisContext c)
    {
        var u = (UsingDirectiveSyntax)c.Node;
        if (u.Name?.ToString() is { } n && (n == "UnityEngine" || n.StartsWith("UnityEngine.", System.StringComparison.Ordinal)))
            Report(c, u.GetLocation(), $"`using {n}`: Ara has no UnityEngine; its API lives in the `Ara` namespace", ("fix", "using"));
    }

    static void BaseType(SyntaxNodeAnalysisContext c)
    {
        var b = (SimpleBaseTypeSyntax)c.Node;
        if (b.Type is IdentifierNameSyntax { Identifier.ValueText: "MonoBehaviour" } or QualifiedNameSyntax { Right.Identifier.ValueText: "MonoBehaviour" })
            Report(c, b.Type.GetLocation(), "`MonoBehaviour` → derive from `Behaviour` (state lives in [Property] partial properties backed by ECS columns)", ("fix", "base"));
    }

    static bool LooksLikeBehaviour(TypeDeclarationSyntax t, SemanticModel model) =>
        t.BaseList?.Types.Any(b => b.Type.ToString().EndsWith("MonoBehaviour", System.StringComparison.Ordinal)) == true
        || Symbols.IsBehaviour(model.GetDeclaredSymbol(t) as INamedTypeSymbol);

    static void Magic(SyntaxNodeAnalysisContext c)
    {
        var m = (MethodDeclarationSyntax)c.Node;
        if (!MagicMethods.TryGetValue(m.Identifier.ValueText, out var to) || m.ParameterList.Parameters.Count != 0
            || m.Modifiers.Any(SyntaxKind.OverrideKeyword) || m.Parent is not TypeDeclarationSyntax t || !LooksLikeBehaviour(t, c.SemanticModel))
            return;
        var sig = to switch { "OnTick" => "OnTick(in Tick tick)", "OnFrame" => "OnFrame(in Frame frame)", _ => "OnSpawn()" };
        Report(c, m.Identifier.GetLocation(), $"`{m.Identifier.ValueText}()` is never called by Ara; override `{sig}`", ("fix", "magic"), ("to", to));
    }

    static void Invocation(SyntaxNodeAnalysisContext c)
    {
        var inv = (InvocationExpressionSyntax)c.Node;
        var name = inv.Expression switch
        {
            GenericNameSyntax g => g,
            MemberAccessExpressionSyntax { Name: GenericNameSyntax g } => g,
            _ => null,
        };
        if (name is { Identifier.ValueText: "GetComponent", TypeArgumentList.Arguments.Count: 1 })
        {
            Report(c, name.GetLocation(), "`GetComponent<T>()` → `Get<T>()`: it returns a `ref` to the ECS value (write through it; there is no null)", ("fix", "getcomponent"));
            return;
        }
        if (inv.Expression is MemberAccessExpressionSyntax { Expression: IdentifierNameSyntax { Identifier.ValueText: "Debug" }, Name.Identifier.ValueText: "Log" or "LogWarning" or "LogError" } log
            && c.SemanticModel.GetSymbolInfo(log).Symbol is null)
        {
            Report(c, log.GetLocation(), $"`Debug.{log.Name.Identifier.ValueText}` → `Log.{LogTarget(log.Name.Identifier.ValueText)}`", ("fix", "debuglog"), ("to", LogTarget(log.Name.Identifier.ValueText)));
        }
    }

    static string LogTarget(string unity) => unity switch { "LogWarning" => "Warn", "LogError" => "Error", _ => "Info" };

    static void Member(SyntaxNodeAnalysisContext c)
    {
        var ma = (MemberAccessExpressionSyntax)c.Node;
        var member = ma.Name.Identifier.ValueText;
        if (ma.Expression is IdentifierNameSyntax { Identifier.ValueText: "Time" } && member is "deltaTime" or "fixedDeltaTime"
            && c.SemanticModel.GetSymbolInfo(ma).Symbol is null)
        {
            Report(c, ma.GetLocation(), $"`Time.{member}` → the tick's fixed step (`tick.Dt`); simulation never reads wall-clock time", ("fix", "deltatime"));
            return;
        }
        if (member is not ("x" or "y" or "z" or "w")) return;
        var type = c.SemanticModel.GetTypeInfo(ma.Expression).Type;
        if (type is INamedTypeSymbol { ContainingNamespace: { Name: "Numerics", ContainingNamespace.Name: "System" } } v && VectorTypes.Contains(v.Name))
            Report(c, ma.Name.GetLocation(), $"`.{member}` → `.{member.ToUpperInvariant()}` (`System.Numerics.{v.Name}` fields are uppercase)",
                ("fix", "member"), ("to", member.ToUpperInvariant()));
    }
}

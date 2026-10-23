using System.Collections.Immutable;
using System.Composition;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CodeActions;
using Microsoft.CodeAnalysis.CodeFixes;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.Analyzers.CodeFixes;

/// <summary>
/// Fixes ARA0001 Unity-isms mechanically: drops <c>using UnityEngine</c>,
/// <c>MonoBehaviour</c> → <c>Behaviour</c> (and makes the class <c>partial</c>), lifecycle
/// methods → Ara overrides, <c>GetComponent&lt;T&gt;()</c> → <c>ref var x = ref Get&lt;T&gt;()</c>,
/// <c>.x</c> → <c>.X</c>, <c>Time.deltaTime</c> → <c>tick.Dt</c> and <c>Debug.Log</c> →
/// <c>Log.Info</c>. Also fixes CS1061 on lowercase vector members.
/// </summary>
[ExportCodeFixProvider(LanguageNames.CSharp, Name = nameof(UnityFix)), Shared]
public sealed class UnityFix : CodeFixProvider
{
    /// <inheritdoc/>
    public override ImmutableArray<string> FixableDiagnosticIds => ["ARA0001", "CS1061"];

    /// <inheritdoc/>
    public override FixAllProvider GetFixAllProvider() => WellKnownFixAllProviders.BatchFixer;

    /// <inheritdoc/>
    public override async Task RegisterCodeFixesAsync(CodeFixContext context)
    {
        var root = await context.Document.GetSyntaxRootAsync(context.CancellationToken).ConfigureAwait(false);
        if (root is null) return;
        foreach (var d in context.Diagnostics)
        {
            var node = root.FindNode(d.Location.SourceSpan, getInnermostNodeForTie: true);
            var fix = d.Id == "CS1061" ? Cs1061(node) : d.Properties.TryGetValue("fix", out var f) ? f : null;
            d.Properties.TryGetValue("to", out var to);
            if (fix is null) continue;
            var title = fix switch
            {
                "using" => "Remove `using UnityEngine`",
                "base" => "Derive from `Behaviour`",
                "magic" => $"Override `{to}`",
                "getcomponent" => "Use `Get<T>()`",
                "member" => $"Use `.{to ?? node.ToString().ToUpperInvariant()}`",
                "deltatime" => "Use the tick's `Dt`",
                "debuglog" => $"Use `Log.{to}`",
                _ => null,
            };
            if (title is null) continue;
            context.RegisterCodeFix(
                CodeAction.Create(title, ct => Apply(context.Document, root, node, fix, to, ct), equivalenceKey: "ARA0001." + fix),
                d);
        }
    }

    /// <summary>CS1061 on <c>.x</c> of a <c>System.Numerics</c> vector is the same fix as ARA0001's.</summary>
    static string? Cs1061(SyntaxNode node) =>
        node is IdentifierNameSyntax { Identifier.ValueText: "x" or "y" or "z" or "w" } && node.Parent is MemberAccessExpressionSyntax ? "member" : null;

    static Task<Document> Apply(Document doc, SyntaxNode root, SyntaxNode node, string fix, string? to, CancellationToken ct)
    {
        var updated = fix switch
        {
            "using" => node.FirstAncestorOrSelf<UsingDirectiveSyntax>() is { } u ? root.RemoveNode(u, SyntaxRemoveOptions.KeepNoTrivia) : root,
            "base" => BaseType(root, node),
            "magic" => node.FirstAncestorOrSelf<MethodDeclarationSyntax>() is { } m && to is not null ? root.ReplaceNode(m, Override(m, to)) : root,
            "getcomponent" => GetComponent(root, node),
            "member" => node is IdentifierNameSyntax id
                ? root.ReplaceNode(id, id.WithIdentifier(SyntaxFactory.Identifier(id.Identifier.LeadingTrivia, to ?? id.Identifier.ValueText.ToUpperInvariant(), id.Identifier.TrailingTrivia)))
                : root,
            "deltatime" => DeltaTime(root, node),
            "debuglog" => node.FirstAncestorOrSelf<MemberAccessExpressionSyntax>() is { } log && to is not null
                ? root.ReplaceNode(log, SyntaxFactory.ParseExpression("Log." + to).WithTriviaFrom(log))
                : root,
            _ => root,
        };
        return Task.FromResult(doc.WithSyntaxRoot(updated ?? root));
    }

    static SyntaxNode BaseType(SyntaxNode root, SyntaxNode node)
    {
        var type = node.FirstAncestorOrSelf<SimpleBaseTypeSyntax>()?.Type ?? node as TypeSyntax;
        if (type is null || type.FirstAncestorOrSelf<ClassDeclarationSyntax>() is not { } cls) return root;
        var replaced = cls.ReplaceNode(type, SyntaxFactory.IdentifierName("Behaviour").WithTriviaFrom(type));
        if (!replaced.Modifiers.Any(SyntaxKind.PartialKeyword))
            replaced = replaced.AddModifiers(SyntaxFactory.Token(SyntaxKind.PartialKeyword).WithTrailingTrivia(SyntaxFactory.Space));
        return root.ReplaceNode(cls, replaced);
    }

    static MethodDeclarationSyntax Override(MethodDeclarationSyntax m, string to)
    {
        var parameters = to switch
        {
            "OnTick" => "(in Tick tick)",
            "OnFrame" => "(in Frame frame)",
            _ => "()",
        };
        var modifiers = SyntaxFactory.TokenList(
            SyntaxFactory.Token(SyntaxKind.ProtectedKeyword).WithLeadingTrivia(m.GetLeadingTrivia()).WithTrailingTrivia(SyntaxFactory.Space),
            SyntaxFactory.Token(SyntaxKind.OverrideKeyword).WithTrailingTrivia(SyntaxFactory.Space));
        return m.WithAttributeLists(default)
            .WithModifiers(modifiers)
            .WithReturnType(SyntaxFactory.PredefinedType(SyntaxFactory.Token(SyntaxKind.VoidKeyword)).WithTrailingTrivia(SyntaxFactory.Space))
            .WithIdentifier(SyntaxFactory.Identifier(to))
            .WithParameterList(SyntaxFactory.ParseParameterList(parameters).WithTrailingTrivia(m.ParameterList.GetTrailingTrivia()));
    }

    static SyntaxNode GetComponent(SyntaxNode root, SyntaxNode node)
    {
        if (node.FirstAncestorOrSelf<GenericNameSyntax>() is not { } g) return root;
        var renamed = g.WithIdentifier(SyntaxFactory.Identifier("Get").WithTriviaFrom(g.Identifier));
        // `var x = GetComponent<T>();` → `ref var x = ref Get<T>();` (writes must reach the ECS).
        if (g.FirstAncestorOrSelf<InvocationExpressionSyntax>() is { } inv
            && inv.Parent is EqualsValueClauseSyntax { Parent: VariableDeclaratorSyntax { Parent: VariableDeclarationSyntax decl } } eq
            && decl.Type is IdentifierNameSyntax { IsVar: true } && decl.Variables.Count == 1)
        {
            var newInv = inv.ReplaceNode(g, renamed);
            var newDecl = decl
                .WithType(SyntaxFactory.RefType(decl.Type.WithoutTrivia()).WithTriviaFrom(decl.Type))
                .ReplaceNode(decl.Variables[0].Initializer!, eq.WithValue(SyntaxFactory.RefExpression(newInv.WithoutLeadingTrivia()).WithLeadingTrivia(SyntaxFactory.Space)));
            return root.ReplaceNode(decl, newDecl);
        }
        return root.ReplaceNode(g, renamed);
    }

    static SyntaxNode DeltaTime(SyntaxNode root, SyntaxNode node)
    {
        if (node.FirstAncestorOrSelf<MemberAccessExpressionSyntax>() is not { } ma) return root;
        var method = ma.FirstAncestorOrSelf<MethodDeclarationSyntax>();
        var tick = method?.ParameterList.Parameters.FirstOrDefault(p => p.Type?.ToString() is "Tick" or "Ara.Tick")?.Identifier.ValueText;
        // A Unity Update() becomes OnTick(in Tick tick) under its own fix.
        tick ??= method is not null && UnityAnalyzer.MagicMethods.TryGetValue(method.Identifier.ValueText, out var to) && to == "OnTick" ? "tick" : null;
        return tick is null ? root : root.ReplaceNode(ma, SyntaxFactory.ParseExpression(tick + ".Dt").WithTriviaFrom(ma));
    }
}

/// <summary>Fixes ARA0101 by marking the field <c>[Transient]</c> (a presentation cache).</summary>
[ExportCodeFixProvider(LanguageNames.CSharp, Name = nameof(TransientFix)), Shared]
public sealed class TransientFix : CodeFixProvider
{
    /// <inheritdoc/>
    public override ImmutableArray<string> FixableDiagnosticIds => ["ARA0101"];

    /// <inheritdoc/>
    public override FixAllProvider GetFixAllProvider() => WellKnownFixAllProviders.BatchFixer;

    /// <inheritdoc/>
    public override async Task RegisterCodeFixesAsync(CodeFixContext context)
    {
        var root = await context.Document.GetSyntaxRootAsync(context.CancellationToken).ConfigureAwait(false);
        if (root?.FindNode(context.Span).FirstAncestorOrSelf<FieldDeclarationSyntax>() is not { } field) return;
        context.RegisterCodeFix(CodeAction.Create("Mark as [Transient] cache", _ =>
        {
            var attr = SyntaxFactory.AttributeList(SyntaxFactory.SingletonSeparatedList(SyntaxFactory.Attribute(SyntaxFactory.IdentifierName("Transient"))))
                .WithLeadingTrivia(field.GetLeadingTrivia())
                .WithTrailingTrivia(SyntaxFactory.Space);
            var updated = field.WithoutLeadingTrivia().WithAttributeLists(field.AttributeLists.Insert(0, attr));
            return Task.FromResult(context.Document.WithSyntaxRoot(root.ReplaceNode(field, updated)));
        }, equivalenceKey: "ARA0101"), context.Diagnostics);
    }
}

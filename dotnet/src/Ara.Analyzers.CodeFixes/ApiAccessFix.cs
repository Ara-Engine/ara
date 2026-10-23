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
/// Fixes the mechanical ARA0506 cases: console and debug output → <c>Log.Info</c> (or
/// <c>Log.Error</c> for <c>Console.Error</c>; arguments become one string),
/// <c>Environment.NewLine</c> → <c>"\n"</c>, and a <c>GC.Collect()</c> statement is removed.
/// </summary>
[ExportCodeFixProvider(LanguageNames.CSharp, Name = nameof(ApiAccessFix)), Shared]
public sealed class ApiAccessFix : CodeFixProvider
{
    /// <inheritdoc/>
    public override ImmutableArray<string> FixableDiagnosticIds => ["ARA0506"];

    /// <inheritdoc/>
    public override FixAllProvider GetFixAllProvider() => WellKnownFixAllProviders.BatchFixer;

    /// <inheritdoc/>
    public override async Task RegisterCodeFixesAsync(CodeFixContext context)
    {
        var root = await context.Document.GetSyntaxRootAsync(context.CancellationToken).ConfigureAwait(false);
        if (root is null) return;
        foreach (var d in context.Diagnostics)
        {
            if (!d.Properties.TryGetValue("fix", out var fix) || fix is null) continue;
            d.Properties.TryGetValue("to", out var to);
            var node = root.FindNode(d.Location.SourceSpan, getInnermostNodeForTie: true);
            var title = fix switch
            {
                "log" => $"Use `Log.{to}`",
                "newline" => "Use \"\\n\"",
                "remove" => "Remove the `GC.Collect()` call",
                _ => null,
            };
            if (title is null) continue;
            context.RegisterCodeFix(
                CodeAction.Create(title, ct => Apply(context.Document, root, node, fix, to ?? "Info", ct), equivalenceKey: "ARA0506." + fix),
                d);
        }
    }

    static Task<Document> Apply(Document doc, SyntaxNode root, SyntaxNode node, string fix, string to, CancellationToken ct)
    {
        _ = ct;
        SyntaxNode? next = fix switch
        {
            "log" when node.FirstAncestorOrSelf<InvocationExpressionSyntax>() is { } call =>
                root.ReplaceNode(call, SyntaxFactory.ParseExpression($"Log.{to}({Message(call.ArgumentList.Arguments)})").WithTriviaFrom(call)),
            "newline" => root.ReplaceNode(node, SyntaxFactory.LiteralExpression(SyntaxKind.StringLiteralExpression, SyntaxFactory.Literal("\n")).WithTriviaFrom(node)),
            "remove" when node.FirstAncestorOrSelf<ExpressionStatementSyntax>() is { } statement =>
                root.RemoveNode(statement, SyntaxRemoveOptions.KeepNoTrivia),
            _ => null,
        };
        return Task.FromResult(next is null ? doc : doc.WithSyntaxRoot(next));
    }

    /// <summary>The arguments as one string: none → <c>""</c>, one → itself (interpolated unless a string literal), several → <c>string.Format</c>.</summary>
    static string Message(SeparatedSyntaxList<ArgumentSyntax> args) => args.Count switch
    {
        0 => "\"\"",
        1 when args[0].Expression is LiteralExpressionSyntax l && l.IsKind(SyntaxKind.StringLiteralExpression) => args[0].ToString(),
        1 when args[0].Expression is InterpolatedStringExpressionSyntax => args[0].ToString(),
        1 => $"$\"{{{args[0]}}}\"",
        _ => $"string.Format({string.Join(", ", args.Select(a => a.ToString()))})",
    };
}

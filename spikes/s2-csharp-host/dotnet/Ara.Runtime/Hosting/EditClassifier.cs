using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.Hosting;

/// <summary>Why an edit could not take the L1 (EnC) path. Values are reported to the host.</summary>
internal enum Escalation : uint
{
    None = 0,
    /// <summary>A file was added, removed or renamed.</summary>
    FileSet = 1,
    /// <summary>Anything outside a method/accessor/operator body changed: signatures, members, fields, attributes, initializers.</summary>
    Structural = 2,
    /// <summary>A changed body is an iterator or async state machine.</summary>
    StateMachine = 3,
    /// <summary>Roslyn could not emit a delta (EnC limitation).</summary>
    EmitFailed = 4,
    /// <summary>The module is compiled optimized (cold); EnC needs Debug IL.</summary>
    ColdModule = 5,
    /// <summary>A debugger is attached; the runtime refuses ApplyUpdate then.</summary>
    Debugger = 6,
    /// <summary>
    /// A changed body contains a lambda, anonymous method, local function or query. Without a
    /// Roslyn syntax map (whose analyzer is internal) the delta is unsafe: S2 crashed the process
    /// with an AccessViolationException after applying a static-lambda body edit.
    /// </summary>
    Closure = 7,
}

/// <summary>One file in a change set. A null tree means the file did not exist on that side.</summary>
internal readonly record struct FileChange(string Path, SyntaxTree? Old, SyntaxTree? New);

/// <summary>A body that changed: the old and new declaration nodes.</summary>
internal readonly record struct BodyEdit(SyntaxTree OldTree, SyntaxNode Old, SyntaxTree NewTree, SyntaxNode New);

/// <summary>
/// Conservative syntax-diff classifier for the L1 path. An edit is body-only when each changed file,
/// with every method, accessor and operator body replaced by a placeholder, is syntactically
/// identical before and after (trivia ignored). Constructors and initializers are never stripped,
/// so any change to them escalates; so does any changed body with a lambda or local function.
/// Anything else escalates to L3. It never asks Roslyn's (internal)
/// rude-edit analyzer, and it never lets the runtime decide: the runtime does not reliably reject
/// unsafe deltas (R10 finding 1.3).
/// </summary>
internal static class EditClassifier
{
    internal static (Escalation escalation, List<BodyEdit> edits) Classify(IReadOnlyList<FileChange> changes)
    {
        var edits = new List<BodyEdit>();
        foreach (var (_, oldTree, newTree) in changes)
        {
            if (oldTree is null || newTree is null) return (Escalation.FileSet, edits);
            var oldRoot = oldTree.GetRoot();
            var newRoot = newTree.GetRoot();
            if (!SyntaxFactory.AreEquivalent(Skeleton(oldRoot), Skeleton(newRoot), topLevel: false))
                return (Escalation.Structural, edits);
            var a = BodyOwners(oldRoot);
            var b = BodyOwners(newRoot);
            if (a.Count != b.Count) return (Escalation.Structural, edits); // unreachable when skeletons match
            for (int i = 0; i < a.Count; i++)
            {
                if (SyntaxFactory.AreEquivalent(a[i], b[i], topLevel: false)) continue;
                if (IsStateMachine(a[i]) || IsStateMachine(b[i])) return (Escalation.StateMachine, edits);
                if (HasClosure(a[i]) || HasClosure(b[i])) return (Escalation.Closure, edits);
                edits.Add(new BodyEdit(oldTree, a[i], newTree, b[i]));
            }
        }
        return (Escalation.None, edits);
    }

    /// <summary>Maps body edits to Roslyn semantic edits (Update of the old and new method symbols).</summary>
    internal static List<SemanticEdit> ToSemanticEdits(List<BodyEdit> edits, Compilation oldComp, Compilation newComp)
    {
        var result = new List<SemanticEdit>(edits.Count);
        foreach (var e in edits)
        {
            var oldSym = Method(oldComp.GetSemanticModel(e.OldTree).GetDeclaredSymbol(e.Old));
            var newSym = Method(newComp.GetSemanticModel(e.NewTree).GetDeclaredSymbol(e.New));
            if (oldSym is null || newSym is null) throw new InvalidOperationException($"no symbol for {e.New.Kind()}");
            result.Add(new SemanticEdit(SemanticEditKind.Update, oldSym, newSym));
        }
        return result;
    }

    /// <summary>True if any edit changes a module's <c>Register</c> method (the host must re-register).</summary>
    internal static bool TouchesRegister(List<BodyEdit> edits) =>
        edits.Any(e => e.New is MethodDeclarationSyntax { Identifier.Text: "Register" });

    /// <summary>The symbol whose IL a body owner produces (property expression bodies compile to the getter).</summary>
    static IMethodSymbol? Method(ISymbol? s) => s switch
    {
        IMethodSymbol m => m,
        IPropertySymbol p => p.GetMethod,
        _ => null,
    };

    static bool IsBodyOwner(SyntaxNode n) => n switch
    {
        MethodDeclarationSyntax or OperatorDeclarationSyntax or ConversionOperatorDeclarationSyntax => true,
        AccessorDeclarationSyntax a => a.Body is not null || a.ExpressionBody is not null,
        PropertyDeclarationSyntax p => p.ExpressionBody is not null,
        IndexerDeclarationSyntax x => x.ExpressionBody is not null,
        _ => false,
    };

    static List<SyntaxNode> BodyOwners(SyntaxNode root) =>
        root.DescendantNodes(n => n is not (BlockSyntax or ArrowExpressionClauseSyntax)).Where(IsBodyOwner).ToList();

    static bool IsStateMachine(SyntaxNode n) =>
        (n is MethodDeclarationSyntax m && m.Modifiers.Any(SyntaxKind.AsyncKeyword))
        || n.DescendantNodes().Any(d => d is YieldStatementSyntax or AwaitExpressionSyntax);

    static bool HasClosure(SyntaxNode n) =>
        n.DescendantNodes().Any(d => d is AnonymousFunctionExpressionSyntax or LocalFunctionStatementSyntax or QueryExpressionSyntax);

    static SyntaxNode Skeleton(SyntaxNode root) => new BodyStripper().Visit(root);

    /// <summary>Replaces bodies with a fixed placeholder; block and expression bodies normalize to the same shape.</summary>
    sealed class BodyStripper : CSharpSyntaxRewriter
    {
        static readonly BlockSyntax Empty = SyntaxFactory.Block();
        static readonly ArrowExpressionClauseSyntax Arrow = SyntaxFactory.ArrowExpressionClause(SyntaxFactory.IdentifierName("_"));

        static T Strip<T>(T node, BlockSyntax? body, ArrowExpressionClauseSyntax? arrow) where T : BaseMethodDeclarationSyntax =>
            body is null && arrow is null
                ? node
                : (T)node.WithBody(Empty).WithExpressionBody(null).WithSemicolonToken(default);

        public override SyntaxNode? VisitMethodDeclaration(MethodDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);
        public override SyntaxNode? VisitOperatorDeclaration(OperatorDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);
        public override SyntaxNode? VisitConversionOperatorDeclaration(ConversionOperatorDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);

        public override SyntaxNode? VisitAccessorDeclaration(AccessorDeclarationSyntax n) =>
            n.Body is null && n.ExpressionBody is null ? n : n.WithBody(Empty).WithExpressionBody(null).WithSemicolonToken(default);

        public override SyntaxNode? VisitPropertyDeclaration(PropertyDeclarationSyntax n) =>
            n.ExpressionBody is null ? base.VisitPropertyDeclaration(n) : n.WithExpressionBody(Arrow);

        public override SyntaxNode? VisitIndexerDeclaration(IndexerDeclarationSyntax n) =>
            n.ExpressionBody is null ? base.VisitIndexerDeclaration(n) : n.WithExpressionBody(Arrow);
    }
}

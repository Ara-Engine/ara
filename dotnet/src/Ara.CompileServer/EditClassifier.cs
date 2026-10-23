using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.CompileServer;

/// <summary>Why an edit could not take the L1 (EnC) path; the values are the ABI's <c>ESC_*</c> codes.</summary>
internal enum Escalation : uint
{
    /// <summary>The edit takes L1.</summary>
    None = 0,
    /// <summary>A file was added, removed or renamed.</summary>
    FileSet = 1,
    /// <summary>Something outside a method, accessor or operator body changed.</summary>
    Structural = 2,
    /// <summary>A changed body is an iterator or async state machine.</summary>
    StateMachine = 3,
    /// <summary>Roslyn could not emit a delta.</summary>
    EmitFailed = 4,
    /// <summary>The module is cold (Release IL); its first edit makes it hot.</summary>
    Cold = 5,
    /// <summary>A debugger is attached in the game process.</summary>
    Debugger = 6,
    /// <summary>
    /// A changed body contains a lambda, anonymous method, local function or query. Without
    /// Roslyn's (internal) syntax map such deltas are unsafe: in S2 a static-lambda body edit
    /// crashed the process with an AccessViolationException after ApplyUpdate.
    /// </summary>
    Closure = 7,
    /// <summary>A constructor or a field/property initializer changed (initializers are inlined into constructors; consts fold into call sites).</summary>
    Constructor = 8,
    /// <summary>The Ara generator's output changed (attributes, partial properties, registration).</summary>
    Generated = 9,
    /// <summary>The host asked for a full image (failed delta, heat change).</summary>
    Requested = 10,
}

/// <summary>One file in a change set. A null tree means the file did not exist on that side.</summary>
internal readonly record struct FileChange(string Path, SyntaxTree? Old, SyntaxTree? New);

/// <summary>A body that changed: the old and new declaration nodes.</summary>
internal readonly record struct BodyEdit(SyntaxTree OldTree, SyntaxNode Old, SyntaxTree NewTree, SyntaxNode New);

/// <summary>
/// The conservative syntax-diff classifier for L1 (S2, adopted in A13). An edit is
/// body-only when each changed file, with every method, accessor and operator body
/// replaced by a placeholder, is identical before and after (trivia ignored); and no
/// changed body is a state machine or contains a closure. Constructors and initializers
/// are never stripped, so changing them escalates. Roslyn's rude-edit analyzer is
/// internal and the runtime does not reliably reject unsafe deltas, so anything not
/// provably safe takes L3. New allowances here are reviewed changes: a mistake crashes
/// the game process.
/// </summary>
internal static class EditClassifier
{
    internal static (Escalation Escalation, List<BodyEdit> Edits) Classify(IReadOnlyList<FileChange> changes)
    {
        var edits = new List<BodyEdit>();
        foreach (var (_, oldTree, newTree) in changes)
        {
            if (oldTree is null || newTree is null) return (Escalation.FileSet, edits);
            var oldRoot = oldTree.GetRoot();
            var newRoot = newTree.GetRoot();
            if (!SyntaxFactory.AreEquivalent(Skeleton(oldRoot, false), Skeleton(newRoot, false), topLevel: false))
            {
                var onlyCtors = SyntaxFactory.AreEquivalent(Skeleton(oldRoot, true), Skeleton(newRoot, true), topLevel: false);
                return (onlyCtors ? Escalation.Constructor : Escalation.Structural, edits);
            }
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

    /// <summary>The method whose IL a body owner produces (an expression-bodied property compiles to its getter).</summary>
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

    /// <summary>The file with bodies replaced; with <paramref name="stripCtors"/>, constructors and initializers too.</summary>
    static SyntaxNode Skeleton(SyntaxNode root, bool stripCtors) => new BodyStripper(stripCtors).Visit(root);

    /// <summary>Replaces bodies with a fixed placeholder; block and expression bodies normalize to the same shape.</summary>
    sealed class BodyStripper(bool stripCtors) : CSharpSyntaxRewriter
    {
        static readonly BlockSyntax Empty = SyntaxFactory.Block();
        static readonly ArrowExpressionClauseSyntax Arrow = SyntaxFactory.ArrowExpressionClause(SyntaxFactory.IdentifierName("_"));

        static T Strip<T>(T node, BlockSyntax? body, ArrowExpressionClauseSyntax? arrow) where T : BaseMethodDeclarationSyntax =>
            body is null && arrow is null ? node : (T)node.WithBody(Empty).WithExpressionBody(null).WithSemicolonToken(default);

        public override SyntaxNode? VisitMethodDeclaration(MethodDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);
        public override SyntaxNode? VisitOperatorDeclaration(OperatorDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);
        public override SyntaxNode? VisitConversionOperatorDeclaration(ConversionOperatorDeclarationSyntax n) => Strip(n, n.Body, n.ExpressionBody);

        public override SyntaxNode? VisitConstructorDeclaration(ConstructorDeclarationSyntax n) =>
            stripCtors ? Strip(n, n.Body, n.ExpressionBody).WithInitializer(null) : base.VisitConstructorDeclaration(n);

        public override SyntaxNode? VisitEqualsValueClause(EqualsValueClauseSyntax n) =>
            stripCtors && n.Parent is VariableDeclaratorSyntax or PropertyDeclarationSyntax
                ? n.WithValue(SyntaxFactory.IdentifierName("_"))
                : base.VisitEqualsValueClause(n);

        public override SyntaxNode? VisitAccessorDeclaration(AccessorDeclarationSyntax n) =>
            n.Body is null && n.ExpressionBody is null ? n : n.WithBody(Empty).WithExpressionBody(null).WithSemicolonToken(default);

        public override SyntaxNode? VisitPropertyDeclaration(PropertyDeclarationSyntax n) =>
            n.ExpressionBody is null ? base.VisitPropertyDeclaration(n) : n.WithExpressionBody(Arrow);

        public override SyntaxNode? VisitIndexerDeclaration(IndexerDeclarationSyntax n) =>
            n.ExpressionBody is null ? base.VisitIndexerDeclaration(n) : n.WithExpressionBody(Arrow);
    }
}

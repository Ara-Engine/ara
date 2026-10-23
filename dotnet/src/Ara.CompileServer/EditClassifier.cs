using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.CompileServer;

/// <summary>Why an edit could not take the EnC path (L1/L2); the values are the ABI's <c>ESC_*</c> codes.</summary>
internal enum Escalation : uint
{
    /// <summary>The edit takes L1 or L2.</summary>
    None = 0,
    /// <summary>A file was removed or renamed.</summary>
    FileSet = 1,
    /// <summary>A declaration changed or was removed (signatures, struct fields, attributes, virtual members, partial types).</summary>
    Structural = 2,
    /// <summary>A changed (or new) member is an iterator or async state machine.</summary>
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
    /// crashed the process with an AccessViolationException after ApplyUpdate. Lambdas in
    /// new members are fine (L2): there is no old closure to map.
    /// </summary>
    Closure = 7,
    /// <summary>
    /// A constructor or a field/property initializer changed or was added to an existing type
    /// (initializers are inlined into constructors that already ran; consts fold into call
    /// sites), or a body changed in a type with a primary constructor (a newly captured
    /// parameter becomes a new field).
    /// </summary>
    Constructor = 8,
    /// <summary>The Ara generator's output changed (attributes, partial properties, registration, schema docs).</summary>
    Generated = 9,
    /// <summary>The host asked for a full image (failed delta, heat change).</summary>
    Requested = 10,
    /// <summary>The runtime rejected the delta (<c>ApplyUpdate</c> threw).</summary>
    RuntimeRejected = 11,
    /// <summary>The runtime lacks an EnC capability the additive edit needs.</summary>
    Capability = 12,
    /// <summary>A referenced module changed its public surface; this module was rebuilt against it.</summary>
    Dependency = 13,
    /// <summary>A referenced module got a new generation; this module's image was reloaded unchanged.</summary>
    Relink = 14,
}

/// <summary>One file in a change set. A null tree means the file did not exist on that side.</summary>
internal readonly record struct FileChange(string Path, SyntaxTree? Old, SyntaxTree? New);

/// <summary>A body that changed: the old and new declaration nodes.</summary>
internal readonly record struct BodyEdit(SyntaxTree OldTree, SyntaxNode Old, SyntaxTree NewTree, SyntaxNode New);

/// <summary>A new member or type declaration (L2), in the new tree.</summary>
internal readonly record struct Insert(SyntaxTree Tree, SyntaxNode Node);

/// <summary>The outcome of <see cref="EditClassifier.Classify"/>: an escalation, or the body edits (L1) and inserts (L2).</summary>
internal sealed record Classification(Escalation Escalation, List<BodyEdit> Bodies, List<Insert> Inserts)
{
    /// <summary>True when the edit adds members or types (L2), not only bodies (L1).</summary>
    internal bool Additive => Inserts.Count > 0;
}

/// <summary>The EnC capabilities the host runtime reports (<c>MetadataUpdater.GetCapabilities</c>).</summary>
internal sealed class EncCapabilities(IEnumerable<string> names)
{
    readonly HashSet<string> _names = new(names, StringComparer.Ordinal);

    /// <summary>What CoreCLR 10 reports.</summary>
    internal static readonly EncCapabilities Net10 = Parse(
        "Baseline AddMethodToExistingType AddStaticFieldToExistingType AddInstanceFieldToExistingType NewTypeDefinition ChangeCustomAttributes UpdateParameters GenericUpdateMethod GenericAddMethodToExistingType GenericAddFieldToExistingType AddFieldRva");

    /// <summary>No additive capabilities: every insert escalates (L1 only).</summary>
    internal static readonly EncCapabilities BodiesOnly = Parse("Baseline");

    /// <summary>From the runtime's space-separated capability string.</summary>
    internal static EncCapabilities Parse(string text) => new(text.Split(' ', StringSplitOptions.RemoveEmptyEntries));

    /// <summary>True if the runtime reports <paramref name="name"/>.</summary>
    internal bool Has(string name) => _names.Contains(name);
}

/// <summary>
/// The conservative syntax-diff classifier for L1 and L2 (S2, A13). Each changed file is
/// diffed member by member:
/// <list type="bullet">
/// <item>a member whose declaration (its body replaced by a placeholder) is unchanged but whose
/// body changed is an L1 body edit, unless the body is a state machine or contains a closure,
/// or the type has a primary constructor;</item>
/// <item>a new member or type is an L2 insert when it is in the allowed set below and the
/// runtime has the capability for it;</item>
/// <item>anything else (a changed or removed declaration, a reordered member, a new
/// constructor or initializer, a removed file) escalates to L3.</item>
/// </list>
/// Roslyn's rude-edit analyzer is internal and the runtime does not reliably reject unsafe
/// deltas, so anything not provably safe takes L3. New allowances here are reviewed
/// changes: a mistake crashes the game process. The compile server also checks that the
/// generator's output is unchanged before taking L2.
/// </summary>
internal static class EditClassifier
{
    /// <summary>Classifies a change set against the runtime's EnC capabilities.</summary>
    internal static Classification Classify(IReadOnlyList<FileChange> changes, EncCapabilities caps)
    {
        var c = new Classifier(caps);
        foreach (var (_, oldTree, newTree) in changes)
        {
            var esc = (oldTree, newTree) switch
            {
                (null, { } n) => c.NewFile(n),
                ({ }, null) => Escalation.FileSet,
                ({ } o, { } n) => c.Container(o, o.GetRoot(), n, n.GetRoot()),
                _ => Escalation.None,
            };
            if (esc != Escalation.None) return new(esc, [], []);
        }
        return new(Escalation.None, c.Bodies, c.Inserts);
    }

    /// <summary>
    /// True if any doc comment changed. Doc comments are trivia (invisible to the member
    /// diff) but the generator turns them into schema docs, so the compile server checks
    /// the generator's output before such an edit takes L1.
    /// </summary>
    internal static bool DocsChanged(IReadOnlyList<FileChange> changes) =>
        changes.Any(c => c.Old is null || c.New is null || Docs(c.Old) != Docs(c.New));

    static string Docs(SyntaxTree tree) =>
        string.Concat(tree.GetRoot().DescendantTrivia(descendIntoTrivia: false)
            .Where(t => t.IsKind(SyntaxKind.SingleLineDocumentationCommentTrivia) || t.IsKind(SyntaxKind.MultiLineDocumentationCommentTrivia))
            .Select(t => t.ToString()));

    /// <summary>
    /// Maps the classification to Roslyn semantic edits: Update of each changed body's method,
    /// Insert of each new member or type. Returns the inserted symbols too (Roslyn's
    /// <c>isAddedSymbol</c>).
    /// </summary>
    internal static (List<SemanticEdit> Edits, HashSet<ISymbol> Added) ToSemanticEdits(Classification c, Compilation oldComp, Compilation newComp)
    {
        var result = new List<SemanticEdit>(c.Bodies.Count + c.Inserts.Count);
        var added = new HashSet<ISymbol>(SymbolEqualityComparer.Default);
        foreach (var e in c.Bodies)
        {
            var oldSym = Method(oldComp.GetSemanticModel(e.OldTree).GetDeclaredSymbol(e.Old));
            var newSym = Method(newComp.GetSemanticModel(e.NewTree).GetDeclaredSymbol(e.New));
            if (oldSym is null || newSym is null) throw new InvalidOperationException($"no symbol for {e.New.Kind()}");
            result.Add(new SemanticEdit(SemanticEditKind.Update, oldSym, newSym));
        }
        foreach (var i in c.Inserts)
        {
            var model = newComp.GetSemanticModel(i.Tree);
            IEnumerable<ISymbol?> symbols = i.Node is BaseFieldDeclarationSyntax f
                ? f.Declaration.Variables.Select(v => model.GetDeclaredSymbol(v))
                : [model.GetDeclaredSymbol(i.Node)];
            foreach (var s in symbols)
            {
                if (s is null) throw new InvalidOperationException($"no symbol for inserted {i.Node.Kind()}");
                result.Add(new SemanticEdit(SemanticEditKind.Insert, null, s));
                added.Add(s);
                if (s is IPropertySymbol p)
                {
                    if (p.GetMethod is { } get) added.Add(get);
                    if (p.SetMethod is { } set) added.Add(set);
                }
            }
        }
        return (result, added);
    }

    /// <summary>The method whose IL a body owner produces (an expression-bodied property compiles to its getter).</summary>
    static IMethodSymbol? Method(ISymbol? s) => s switch
    {
        IMethodSymbol m => m,
        IPropertySymbol p => p.GetMethod,
        _ => null,
    };

    /// <summary>The walk over one change set: collects body edits and inserts, returns the first escalation.</summary>
    sealed class Classifier(EncCapabilities caps)
    {
        internal readonly List<BodyEdit> Bodies = [];
        internal readonly List<Insert> Inserts = [];

        /// <summary>A file that did not exist: every type in it is new.</summary>
        internal Escalation NewFile(SyntaxTree tree)
        {
            var root = (CompilationUnitSyntax)tree.GetRoot();
            // Assembly attributes, global usings and aliases change how every file binds.
            if (root.AttributeLists.Count > 0 || root.Usings.Any(u => u.GlobalKeyword != default) || root.Externs.Count > 0) return Escalation.Structural;
            foreach (var member in root.Members)
            {
                var esc = member switch
                {
                    BaseNamespaceDeclarationSyntax ns => NewNamespace(tree, ns),
                    BaseTypeDeclarationSyntax or DelegateDeclarationSyntax => NewType(tree, member),
                    _ => Escalation.Structural, // top-level statements, global members
                };
                if (esc != Escalation.None) return esc;
            }
            return Escalation.None;
        }

        Escalation NewNamespace(SyntaxTree tree, BaseNamespaceDeclarationSyntax ns)
        {
            foreach (var member in ns.Members)
            {
                var esc = member switch
                {
                    BaseNamespaceDeclarationSyntax inner => NewNamespace(tree, inner),
                    BaseTypeDeclarationSyntax or DelegateDeclarationSyntax => NewType(tree, member),
                    _ => Escalation.Structural,
                };
                if (esc != Escalation.None) return esc;
            }
            return Escalation.None;
        }

        /// <summary>
        /// A new type (top level or nested): allowed when the runtime can add type definitions
        /// and the type cannot merge into an existing one (not partial) or be generic. Its
        /// members are all new, so constructors, initializers and lambdas are fine inside it.
        /// </summary>
        Escalation NewType(SyntaxTree tree, SyntaxNode node)
        {
            if (!caps.Has("NewTypeDefinition")) return Escalation.Capability;
            var modifiers = node switch
            {
                BaseTypeDeclarationSyntax t => t.Modifiers,
                DelegateDeclarationSyntax d => d.Modifiers,
                _ => default,
            };
            if (modifiers.Any(SyntaxKind.PartialKeyword)) return Escalation.Structural;
            if (node is TypeDeclarationSyntax { TypeParameterList: not null } or DelegateDeclarationSyntax { TypeParameterList: not null })
                return Escalation.Structural;
            if (node.DescendantNodes().OfType<TypeDeclarationSyntax>().Any(t => t.Modifiers.Any(SyntaxKind.PartialKeyword) || t.TypeParameterList is not null))
                return Escalation.Structural;
            Inserts.Add(new Insert(tree, node));
            return Escalation.None;
        }

        /// <summary>
        /// Diffs a compilation unit, namespace or type declaration: its own declaration must be
        /// unchanged; members are matched by key, in order (none removed or reordered).
        /// </summary>
        internal Escalation Container(SyntaxTree oldTree, SyntaxNode oldNode, SyntaxTree newTree, SyntaxNode newNode)
        {
            if (!SyntaxFactory.AreEquivalent(Header(oldNode), Header(newNode), topLevel: false)) return Escalation.Structural;
            if (oldNode is EnumDeclarationSyntax) return SyntaxFactory.AreEquivalent(oldNode, newNode, topLevel: false) ? Escalation.None : Escalation.Structural;
            var olds = Keyed(Members(oldNode));
            var news = Keyed(Members(newNode));
            int at = 0;
            foreach (var (key, newMember) in news)
            {
                if (at < olds.Count && olds[at].Key == key)
                {
                    var esc = Matched(oldTree, olds[at].Node, newTree, newMember, newNode);
                    if (esc != Escalation.None) return esc;
                    at++;
                }
                else if (olds.Skip(at).Any(o => o.Key == key)) return Escalation.Structural; // reordered
                else
                {
                    var esc = Inserted(newTree, newMember, newNode);
                    if (esc != Escalation.None) return esc;
                }
            }
            return at == olds.Count ? Escalation.None : Escalation.Structural; // something was removed
        }

        Escalation Matched(SyntaxTree oldTree, SyntaxNode o, SyntaxTree newTree, SyntaxNode n, SyntaxNode container)
        {
            if (o is BaseNamespaceDeclarationSyntax or TypeDeclarationSyntax or EnumDeclarationSyntax)
                return Container(oldTree, o, newTree, n);
            if (SyntaxFactory.AreEquivalent(o, n, topLevel: false)) return Escalation.None;
            if (!SyntaxFactory.AreEquivalent(Strip(o, false), Strip(n, false), topLevel: false))
                return SyntaxFactory.AreEquivalent(Strip(o, true), Strip(n, true), topLevel: false) ? Escalation.Constructor : Escalation.Structural;
            // Same declaration, different body: the L1 rules.
            if (IsStateMachine(o) || IsStateMachine(n)) return Escalation.StateMachine;
            if (HasClosure(o) || HasClosure(n)) return Escalation.Closure;
            if (container is TypeDeclarationSyntax { ParameterList: not null }) return Escalation.Constructor;
            foreach (var (ob, nb) in BodyOwners(o).Zip(BodyOwners(n)))
                if (!SyntaxFactory.AreEquivalent(ob, nb, topLevel: false)) Bodies.Add(new BodyEdit(oldTree, ob, newTree, nb));
            return Escalation.None;
        }

        /// <summary>A member that is new in an existing type or namespace (L2 rules).</summary>
        Escalation Inserted(SyntaxTree tree, SyntaxNode member, SyntaxNode container)
        {
            if (member is BaseTypeDeclarationSyntax or DelegateDeclarationSyntax) return NewType(tree, member);
            if (member is BaseNamespaceDeclarationSyntax ns) return NewNamespace(tree, ns);
            if (container is not TypeDeclarationSyntax type || type is InterfaceDeclarationSyntax) return Escalation.Structural;
            if (type.ParameterList is not null) return Escalation.Constructor; // a new member may capture a parameter (a new field)
            var isStruct = type is StructDeclarationSyntax || type is RecordDeclarationSyntax r && r.ClassOrStructKeyword.IsKind(SyntaxKind.StructKeyword);
            var genericType = member.Ancestors().OfType<TypeDeclarationSyntax>().Any(t => t.TypeParameterList is not null);
            var mods = member is MemberDeclarationSyntax m ? m.Modifiers : default;
            if (mods.Any(SyntaxKind.AbstractKeyword) || mods.Any(SyntaxKind.VirtualKeyword) || mods.Any(SyntaxKind.OverrideKeyword)
                || mods.Any(SyntaxKind.ExternKeyword) || mods.Any(SyntaxKind.PartialKeyword) || mods.Any(SyntaxKind.NewKeyword))
                return Escalation.Structural;
            var isStatic = mods.Any(SyntaxKind.StaticKeyword) || mods.Any(SyntaxKind.ConstKeyword);
            switch (member)
            {
                case MethodDeclarationSyntax method:
                    if (method.ExplicitInterfaceSpecifier is not null || method.TypeParameterList is not null) return Escalation.Structural;
                    if (IsStateMachine(method)) return Escalation.StateMachine;
                    return Need(genericType ? "GenericAddMethodToExistingType" : "AddMethodToExistingType", ClosureCaps(method), tree, member);
                case OperatorDeclarationSyntax or ConversionOperatorDeclarationSyntax:
                    if (IsStateMachine(member)) return Escalation.StateMachine;
                    return Need(genericType ? "GenericAddMethodToExistingType" : "AddMethodToExistingType", ClosureCaps(member), tree, member);
                case PropertyDeclarationSyntax prop:
                    if (prop.ExplicitInterfaceSpecifier is not null) return Escalation.Structural;
                    if (prop.Initializer is not null) return Escalation.Constructor;
                    var auto = prop.AccessorList is { } list && list.Accessors.All(a => a.Body is null && a.ExpressionBody is null);
                    if (auto && isStruct && !isStatic) return Escalation.Structural; // a backing field changes the struct's layout
                    if (auto && !caps.Has(isStatic ? "AddStaticFieldToExistingType" : (genericType ? "GenericAddFieldToExistingType" : "AddInstanceFieldToExistingType")))
                        return Escalation.Capability;
                    return Need(genericType ? "GenericAddMethodToExistingType" : "AddMethodToExistingType", ClosureCaps(member), tree, member);
                case FieldDeclarationSyntax field:
                    if (field.Modifiers.Any(SyntaxKind.FixedKeyword)) return Escalation.Structural;
                    var isConst = field.Modifiers.Any(SyntaxKind.ConstKeyword);
                    if (!isConst && field.Declaration.Variables.Any(v => v.Initializer is not null)) return Escalation.Constructor;
                    if (isStruct && !isStatic) return Escalation.Structural; // a new field changes the struct's layout
                    return Need(isStatic ? "AddStaticFieldToExistingType" : (genericType ? "GenericAddFieldToExistingType" : "AddInstanceFieldToExistingType"), [], tree, member);
                case ConstructorDeclarationSyntax:
                    return Escalation.Constructor;
                default:
                    return Escalation.Structural; // events, indexers, finalizers
            }
        }

        Escalation Need(string capability, string[] more, SyntaxTree tree, SyntaxNode member)
        {
            if (!caps.Has(capability) || more.Any(c => !caps.Has(c))) return Escalation.Capability;
            Inserts.Add(new Insert(tree, member));
            return Escalation.None;
        }

        /// <summary>Lambdas in a new member add methods and cache fields to the type's closure class, or a new one.</summary>
        static string[] ClosureCaps(SyntaxNode member) =>
            HasClosure(member) ? ["AddStaticFieldToExistingType", "NewTypeDefinition"] : [];
    }

    /// <summary>The members of a container, in declaration order.</summary>
    static IEnumerable<SyntaxNode> Members(SyntaxNode node) => node switch
    {
        CompilationUnitSyntax cu => cu.Members,
        BaseNamespaceDeclarationSyntax ns => ns.Members,
        TypeDeclarationSyntax t => t.Members,
        _ => [],
    };

    /// <summary>A container without its members: what must stay equal for its members to be diffed.</summary>
    static SyntaxNode Header(SyntaxNode node) => node switch
    {
        CompilationUnitSyntax cu => cu.WithMembers(default),
        BaseNamespaceDeclarationSyntax ns => ns.WithMembers(default),
        TypeDeclarationSyntax t => t.WithMembers(default),
        _ => node,
    };

    /// <summary>Members keyed by kind, name and parameter types; repeats get an occurrence suffix.</summary>
    static List<(string Key, SyntaxNode Node)> Keyed(IEnumerable<SyntaxNode> members)
    {
        var seen = new Dictionary<string, int>();
        var list = new List<(string, SyntaxNode)>();
        foreach (var m in members)
        {
            var key = Key(m);
            var n = seen.TryGetValue(key, out var k) ? k + 1 : 0;
            seen[key] = n;
            list.Add((n == 0 ? key : $"{key}#{n}", m));
        }
        return list;
    }

    static string Key(SyntaxNode m) => m switch
    {
        BaseNamespaceDeclarationSyntax ns => "namespace " + ns.Name,
        TypeDeclarationSyntax t => $"{t.Keyword.Text} {t.Identifier.Text}`{t.TypeParameterList?.Parameters.Count ?? 0}",
        EnumDeclarationSyntax e => "enum " + e.Identifier.Text,
        DelegateDeclarationSyntax d => $"delegate {d.Identifier.Text}`{d.TypeParameterList?.Parameters.Count ?? 0}",
        MethodDeclarationSyntax x => $"method {x.ExplicitInterfaceSpecifier}{x.Identifier.Text}`{x.TypeParameterList?.Parameters.Count ?? 0}{Params(x.ParameterList)}",
        ConstructorDeclarationSyntax x => $"ctor {(x.Modifiers.Any(SyntaxKind.StaticKeyword) ? "static" : "")}{Params(x.ParameterList)}",
        DestructorDeclarationSyntax => "dtor",
        OperatorDeclarationSyntax x => $"operator {x.OperatorToken.Text}{Params(x.ParameterList)}",
        ConversionOperatorDeclarationSyntax x => $"conversion {x.ImplicitOrExplicitKeyword.Text} {x.Type}{Params(x.ParameterList)}",
        PropertyDeclarationSyntax x => $"property {x.ExplicitInterfaceSpecifier}{x.Identifier.Text}",
        IndexerDeclarationSyntax x => $"indexer {x.ExplicitInterfaceSpecifier}{Params(x.ParameterList)}",
        EventDeclarationSyntax x => "event " + x.Identifier.Text,
        BaseFieldDeclarationSyntax x => (x is EventFieldDeclarationSyntax ? "event " : "field ") + string.Join(",", x.Declaration.Variables.Select(v => v.Identifier.Text)),
        _ => m.Kind().ToString(),
    };

    static string Params(BaseParameterListSyntax list) =>
        "(" + string.Join(",", list.Parameters.Select(p => string.Join(" ", p.Modifiers.Select(t => t.Text)) + " " + p.Type?.ToString())) + ")";

    /// <summary>The nodes whose bodies compile to methods: the member itself, or its accessors.</summary>
    static IEnumerable<SyntaxNode> BodyOwners(SyntaxNode member) => member switch
    {
        PropertyDeclarationSyntax { ExpressionBody: not null } p => [p],
        IndexerDeclarationSyntax { ExpressionBody: not null } x => [x],
        BasePropertyDeclarationSyntax { AccessorList: { } list } => list.Accessors.Where(a => a.Body is not null || a.ExpressionBody is not null),
        BaseMethodDeclarationSyntax => [member],
        _ => [],
    };

    static bool IsStateMachine(SyntaxNode n) =>
        (n is MethodDeclarationSyntax m && m.Modifiers.Any(SyntaxKind.AsyncKeyword))
        || n.DescendantNodes().Any(d => d is YieldStatementSyntax or AwaitExpressionSyntax);

    static bool HasClosure(SyntaxNode n) =>
        n.DescendantNodes().Any(d => d is AnonymousFunctionExpressionSyntax or LocalFunctionStatementSyntax or QueryExpressionSyntax);

    /// <summary>The member with bodies replaced; with <paramref name="stripCtors"/>, constructor bodies and initializers too.</summary>
    static SyntaxNode Strip(SyntaxNode node, bool stripCtors) => new BodyStripper(stripCtors).Visit(node);

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

using System.Collections.Immutable;
using System.Runtime.CompilerServices;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.CompileServer;

/// <summary>
/// Which files an edit that changes declarations (an additive L2 edit or any L3 rebuild) can change
/// analyzer results in. A new, changed or removed member alters how other code binds only where that
/// code spells its name, so on a module whose last analysis was clean the touched files plus the files
/// that spell one of the changed names are enough. Anything whose lookup is not by spelled name (operators,
/// conversions, indexers, base lists, modifiers, type parameters, assembly attributes, global usings, the
/// names the language looks up on its own) takes the whole module.
/// </summary>
internal static class AnalysisScope
{
    /// <summary>
    /// Names the language binds without the identifier appearing in source: <c>foreach</c>, <c>using</c>,
    /// <c>await</c>, deconstruction, collection initializers, queries, index and range patterns,
    /// interpolation handlers and the members every object has.
    /// </summary>
    static readonly ImmutableHashSet<string> ImplicitNames = ImmutableHashSet.Create(StringComparer.Ordinal,
        "GetEnumerator", "MoveNext", "Current", "Dispose", "DisposeAsync", "GetAsyncEnumerator", "MoveNextAsync",
        "GetAwaiter", "IsCompleted", "GetResult", "OnCompleted", "UnsafeOnCompleted", "Deconstruct", "Add",
        "Select", "SelectMany", "Where", "OrderBy", "OrderByDescending", "ThenBy", "ThenByDescending",
        "GroupBy", "Join", "GroupJoin", "Cast", "OfType", "Length", "Count", "Slice", "Item",
        "GetPinnableReference", "ToString", "Equals", "GetHashCode", "Invoke", "Append", "AppendLiteral",
        "AppendFormatted", "AppendFormat");

    static readonly ConditionalWeakTable<SyntaxTree, HashSet<string>> Spellings = new();
    static readonly ConditionalWeakTable<SyntaxTree, HashSet<string>> Namespaces = new();

    /// <summary>
    /// The trees to analyze for <paramref name="changes"/>: <paramref name="changed"/> plus every tree of
    /// <paramref name="all"/> that spells a name whose declaration changed; null when the edit needs the whole module.
    /// </summary>
    internal static IReadOnlyCollection<SyntaxTree>? Trees(IReadOnlyList<FileChange> changes, IReadOnlyCollection<SyntaxTree> changed, IEnumerable<SyntaxTree> all)
    {
        var seen = new HashSet<SyntaxTree>(changed);
        var unchanged = all.Where(tree => !seen.Contains(tree)).ToArray();
        // A namespace other files declare keeps existing when one file's declaration comes or goes.
        var existing = new HashSet<string>(unchanged.SelectMany(tree => DeclaredNamespaces(tree)), StringComparer.Ordinal);
        var names = new HashSet<string>(StringComparer.Ordinal);
        var walk = new Walk(names, existing);
        foreach (var (_, oldTree, newTree) in changes)
            if (!walk.Declarations(oldTree?.GetRoot(), newTree?.GetRoot())) return null;
        if (names.Overlaps(ImplicitNames)) return null;
        var trees = new List<SyntaxTree>(changed);
        foreach (var tree in unchanged)
            if (Spelled(tree).Overlaps(names)) trees.Add(tree);
        return trees;
    }

    /// <summary>Every namespace a tree declares, with its enclosing prefixes (<c>A.B</c> declares <c>A</c> too).</summary>
    static HashSet<string> DeclaredNamespaces(SyntaxTree tree) => Namespaces.GetValue(tree, static t =>
    {
        var set = new HashSet<string>(StringComparer.Ordinal);
        foreach (var ns in t.GetRoot().DescendantNodes().OfType<BaseNamespaceDeclarationSyntax>())
            for (var qualified = Qualified(ns); qualified.Length > 0; qualified = qualified.Contains('.') ? qualified[..qualified.LastIndexOf('.')] : "")
                set.Add(qualified);
        return set;
    });

    /// <summary>The namespace's full name including enclosing namespace declarations.</summary>
    static string Qualified(BaseNamespaceDeclarationSyntax ns) =>
        string.Join(".", ns.AncestorsAndSelf().OfType<BaseNamespaceDeclarationSyntax>().Reverse().Select(n => n.Name.ToString()));

    /// <summary>Every identifier a tree spells, computed once per tree instance (unchanged files keep theirs).</summary>
    static HashSet<string> Spelled(SyntaxTree tree) => Spellings.GetValue(tree, static t =>
    {
        var set = new HashSet<string>(StringComparer.Ordinal);
        foreach (var token in t.GetRoot().DescendantTokens())
            if (token.IsKind(SyntaxKind.IdentifierToken)) set.Add(token.ValueText);
        return set;
    });

    /// <summary>Collects the names whose declaration changed across a change set.</summary>
    sealed class Walk(HashSet<string> names, HashSet<string> existing)
    {
        /// <summary>Collects the changed names of one file; false when the change cannot be scoped by name.</summary>
        internal bool Declarations(SyntaxNode? oldRoot, SyntaxNode? newRoot)
        {
            if (oldRoot is CompilationUnitSyntax o && newRoot is CompilationUnitSyntax n)
                return SameFileHeader(o, n) && Members(o, n, null);
            // A file that appeared or vanished: it must not carry assembly attributes, global usings or extern aliases.
            if ((oldRoot ?? newRoot) is not CompilationUnitSyntax only || (oldRoot is not null && newRoot is not null)) return false;
            return only.AttributeLists.Count == 0 && only.Externs.Count == 0 && !only.Usings.Any(u => u.GlobalKeyword != default)
                && only.Members.All(member => AddAll(member, null));
        }

        /// <summary>Plain usings are file-local; attributes, extern aliases and global usings change how every file binds.</summary>
        static bool SameFileHeader(CompilationUnitSyntax o, CompilationUnitSyntax n)
        {
            var oldGlobals = o.Usings.Where(u => u.GlobalKeyword != default).ToArray();
            var newGlobals = n.Usings.Where(u => u.GlobalKeyword != default).ToArray();
            return SyntaxFactory.AreEquivalent(o.AttributeLists, n.AttributeLists, topLevel: false)
                && SyntaxFactory.AreEquivalent(o.Externs, n.Externs, topLevel: false)
                && oldGlobals.Length == newGlobals.Length
                && oldGlobals.Zip(newGlobals).All(pair => SyntaxFactory.AreEquivalent(pair.First, pair.Second, topLevel: false));
        }

        /// <summary>Members matched by kind, name and parameter types; one-sided members contribute all their names.</summary>
        bool Members(SyntaxNode o, SyntaxNode n, string? type)
        {
            var olds = EditClassifier.Keyed(EditClassifier.Members(o)).ToDictionary(pair => pair.Key, pair => pair.Node, StringComparer.Ordinal);
            foreach (var (key, member) in EditClassifier.Keyed(EditClassifier.Members(n)))
            {
                if (!olds.Remove(key, out var old)) { if (!AddAll(member, type)) return false; }
                else if (!Member(old, member, type)) return false;
            }
            return olds.Values.All(member => AddAll(member, type));
        }

        bool Member(SyntaxNode o, SyntaxNode n, string? type)
        {
            switch (o)
            {
                case BaseNamespaceDeclarationSyntax oNs when n is BaseNamespaceDeclarationSyntax nNs:
                    return SyntaxFactory.AreEquivalent(oNs.Name, nNs.Name, topLevel: false) && Members(oNs, nNs, null);
                case TypeDeclarationSyntax oType when n is TypeDeclarationSyntax nType:
                    if (!SyntaxFactory.AreEquivalent(EditClassifier.Header(oType), EditClassifier.Header(nType), topLevel: false))
                    {
                        // Attributes can change what analyzers apply to every part of the type; base lists, modifiers,
                        // type parameters and primary constructors change binding in ways no spelled name finds.
                        if (!SyntaxFactory.AreEquivalent(EditClassifier.Header(oType.WithAttributeLists(default)), EditClassifier.Header(nType.WithAttributeLists(default)), topLevel: false))
                            return false;
                        AddType(oType.Identifier.ValueText);
                    }
                    return Members(oType, nType, oType.Identifier.ValueText);
                case EnumDeclarationSyntax when !SyntaxFactory.AreEquivalent(o, n, topLevel: false):
                    return AddAll(o, type) && AddAll(n, type);
                case EnumDeclarationSyntax:
                    return true;
                default:
                    // Same declaration with different bodies changes nothing for other files.
                    return SyntaxFactory.AreEquivalent(EditClassifier.Strip(o, false), EditClassifier.Strip(n, false), topLevel: false)
                        || (AddAll(o, type) && AddAll(n, type));
            }
        }

        /// <summary>Adds every name a declaration introduces; false for kinds found by more than a spelled name.</summary>
        bool AddAll(SyntaxNode node, string? type)
        {
            switch (node)
            {
                case BaseNamespaceDeclarationSyntax ns:
                    // Only a namespace no other file declares is new (or gone) as a name.
                    if (!existing.Contains(Qualified(ns)))
                        foreach (var token in ns.Name.DescendantTokens().Where(t => t.IsKind(SyntaxKind.IdentifierToken))) names.Add(token.ValueText);
                    return ns.Members.All(member => AddAll(member, null));
                case TypeDeclarationSyntax t:
                    AddType(t.Identifier.ValueText);
                    return t.Members.All(member => AddAll(member, t.Identifier.ValueText));
                case EnumDeclarationSyntax e:
                    AddType(e.Identifier.ValueText);
                    foreach (var member in e.Members) names.Add(member.Identifier.ValueText);
                    return true;
                case DelegateDeclarationSyntax d: AddType(d.Identifier.ValueText); return true;
                case MethodDeclarationSyntax m: names.Add(m.Identifier.ValueText); return true;
                case PropertyDeclarationSyntax p: names.Add(p.Identifier.ValueText); return true;
                case EventDeclarationSyntax ev: names.Add(ev.Identifier.ValueText); return true;
                case BaseFieldDeclarationSyntax f:
                    foreach (var variable in f.Declaration.Variables) names.Add(variable.Identifier.ValueText);
                    return true;
                case ConstructorDeclarationSyntax:
                    // Callers spell the type, not a constructor name.
                    if (type is null) return false;
                    names.Add(type);
                    return true;
                default: return false; // operators, conversions, indexers, finalizers
            }
        }

        /// <summary>A type is also spelled without the <c>Attribute</c> suffix where it is applied.</summary>
        void AddType(string name)
        {
            names.Add(name);
            if (name.EndsWith("Attribute", StringComparison.Ordinal) && name.Length > "Attribute".Length) names.Add(name[..^"Attribute".Length]);
        }
    }
}

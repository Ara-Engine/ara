using System.Collections.Immutable;
using System.Runtime.CompilerServices;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.CompileServer;

/// <summary>
/// Which files an additive (L2) edit can change analyzer results in. A new member changes how
/// existing code binds only where that code names it, so on a module whose last analysis was
/// clean the touched files plus the files that spell one of the inserted names are enough. Anything
/// whose lookup is not by spelled name (operators, conversions, types, the names the language
/// looks up on its own) takes the full module.
/// </summary>
internal static class AdditiveAnalysisScope
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

    /// <summary>
    /// The trees to analyze for the additive edit <paramref name="inserts"/>: <paramref name="changed"/> plus every
    /// tree of <paramref name="all"/> that spells an inserted name; null when the edit needs the whole module.
    /// </summary>
    internal static IReadOnlyCollection<SyntaxTree>? Trees(IReadOnlyList<Insert> inserts, IReadOnlyCollection<SyntaxTree> changed, IEnumerable<SyntaxTree> all)
    {
        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (var insert in inserts)
        {
            switch (insert.Node)
            {
                case MethodDeclarationSyntax method: names.Add(method.Identifier.ValueText); break;
                case PropertyDeclarationSyntax property: names.Add(property.Identifier.ValueText); break;
                case FieldDeclarationSyntax field:
                    foreach (var variable in field.Declaration.Variables) names.Add(variable.Identifier.ValueText);
                    break;
                default: return null; // types, operators, conversions: not bound by a spelled member name
            }
        }
        if (names.Overlaps(ImplicitNames)) return null;
        var trees = new List<SyntaxTree>(changed);
        var seen = new HashSet<SyntaxTree>(changed);
        foreach (var tree in all)
            if (!seen.Contains(tree) && Spelled(tree).Overlaps(names)) trees.Add(tree);
        return trees;
    }

    /// <summary>Every identifier a tree spells, computed once per tree instance (unchanged files keep theirs).</summary>
    static HashSet<string> Spelled(SyntaxTree tree) => Spellings.GetValue(tree, static t =>
    {
        var set = new HashSet<string>(StringComparer.Ordinal);
        foreach (var token in t.GetRoot().DescendantTokens())
            if (token.IsKind(SyntaxKind.IdentifierToken)) set.Add(token.ValueText);
        return set;
    });
}

using System;
using System.Linq;
using System.Text;
using System.Xml.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.Generators;

/// <summary>
/// Reads XML doc comments as plain text (code spans as Markdown backticks). Works with
/// any <c>DocumentationMode</c>: when the compiler did not parse docs, the comment is
/// read from the declaration's trivia.
/// </summary>
public static class Docs
{
    /// <summary>The symbol's <c>&lt;summary&gt;</c>, or "".</summary>
    public static string Summary(ISymbol symbol) => Tag(Xml(symbol), "summary", null);

    /// <summary>Reads one declaration's summary without Roslyn merging partial-type comments.</summary>
    public static string Summary(SyntaxNode declaration) => Tag(Xml(declaration), "summary", null);

    /// <summary>The method's <c>&lt;param name="..."&gt;</c> text, or "".</summary>
    public static string Param(IMethodSymbol method, string name) => Tag(Xml(method), "param", name);

    static string? Xml(ISymbol symbol)
    {
        var xml = symbol.GetDocumentationCommentXml(expandIncludes: false);
        if (!string.IsNullOrWhiteSpace(xml)) return xml;
        var node = symbol.DeclaringSyntaxReferences.FirstOrDefault()?.GetSyntax();
        return node is null ? null : Xml(node);
    }

    static string? Xml(SyntaxNode? node)
    {
        if (node is VariableDeclaratorSyntax) node = node.Parent?.Parent; // field: the FieldDeclaration holds the trivia
        if (node is null) return null;
        var sb = new StringBuilder();
        foreach (var t in node.GetLeadingTrivia())
        {
            if (t.IsKind(SyntaxKind.SingleLineDocumentationCommentTrivia))
                foreach (var line in t.ToFullString().Split('\n'))
                    sb.AppendLine(StripSlashes(line));
            else if (t.IsKind(SyntaxKind.SingleLineCommentTrivia) && t.ToString().StartsWith("///", StringComparison.Ordinal))
                sb.AppendLine(StripSlashes(t.ToString()));
        }
        return sb.Length == 0 ? null : "<member>" + sb + "</member>";
    }

    static string StripSlashes(string line)
    {
        var l = line.TrimStart();
        return l.StartsWith("///", StringComparison.Ordinal) ? l.Substring(3) : l;
    }

    static string Tag(string? xml, string tag, string? name)
    {
        if (xml is null) return "";
        XElement root;
        try { root = XElement.Parse(xml.Trim().StartsWith("<member", StringComparison.Ordinal) ? xml : "<member>" + xml + "</member>"); }
        catch { return ""; }
        var el = root.Elements(tag).FirstOrDefault(e => name is null || (string?)e.Attribute("name") == name);
        return el is null ? "" : Collapse(Text(el));
    }

    static string Text(XElement e)
    {
        var sb = new StringBuilder();
        foreach (var n in e.Nodes())
        {
            switch (n)
            {
                case XText t: sb.Append(t.Value); break;
                case XElement c when c.Name == "c" || c.Name == "code": sb.Append('`').Append(c.Value).Append('`'); break;
                case XElement s when s.Name == "see" || s.Name == "seealso":
                    var cref = (string?)s.Attribute("cref") ?? (string?)s.Attribute("langword") ?? (string?)s.Attribute("href") ?? "";
                    var simple = cref.Contains(':') ? cref.Substring(cref.IndexOf(':') + 1) : cref;
                    simple = simple.Split('(')[0];
                    sb.Append('`').Append(s.Value.Length > 0 ? s.Value : simple.Split('.').Last()).Append('`');
                    break;
                case XElement p when p.Name == "paramref" || p.Name == "typeparamref": sb.Append('`').Append((string?)p.Attribute("name")).Append('`'); break;
                case XElement other: sb.Append(Text(other)); break;
            }
        }
        return sb.ToString();
    }

    static string Collapse(string s) => string.Join(" ", s.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries));
}

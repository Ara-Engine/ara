using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.Generators.Tests;

public sealed class DocSummaryTests
{
    [Theory]
    [InlineData("<summary> Current   amount. </summary>", "Current amount.")]
    [InlineData("<summary></summary>", "")]
    [InlineData("<summary>one\t two\n/// three</summary>", "one two three")]
    [InlineData("<summary>rock &amp; roll &lt; 7</summary>", "rock & roll < 7")]
    [InlineData("<summary>café 世界 🚀</summary>", "café 世界 🚀")]
    [InlineData("<summary><![CDATA[a < b]]></summary>", "a < b")]
    [InlineData("<summary>Use <c>Value</c> and <see langword=\"null\"/>.</summary>", "Use `Value` and `null`.")]
    [InlineData("<summary>Nested <b>bold <i>text</i></b>.</summary>", "Nested bold text.")]
    [InlineData("<summary>first</summary><summary>second</summary>", "first")]
    [InlineData("<summary class=\"detail\">Annotated.</summary><remarks>More.</remarks>", "Annotated.")]
    [InlineData("<summary xmlns=\"urn:other\">Namespaced.</summary>", "")]
    [InlineData("<summary>value</summary><broken>", "")]
    [InlineData("<summary>value &unknown;</summary>", "")]
    [InlineData("<summary>raw ]]> text</summary>", "")]
    public void Summary_keeps_plain_rich_and_invalid_xml_semantics(string comment, string expected)
    {
        foreach (var mode in new[] { DocumentationMode.None, DocumentationMode.Parse })
        {
            var tree = CSharpSyntaxTree.ParseText("/// " + comment + "\npublic struct Probe { }", new CSharpParseOptions(documentationMode: mode));
            var declaration = tree.GetRoot().DescendantNodes().OfType<StructDeclarationSyntax>().Single();
            Assert.Equal(expected, Docs.Summary(declaration));
            var compilation = CSharpCompilation.Create("Docs", [tree]);
            var symbol = compilation.GetSemanticModel(tree).GetDeclaredSymbol(declaration)!;
            Assert.Equal(expected, Docs.Summary(symbol));
        }
    }
}

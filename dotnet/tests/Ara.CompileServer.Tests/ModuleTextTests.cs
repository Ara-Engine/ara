using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Text;
using System.Text;

namespace Ara.CompileServer.Tests;

public sealed class ModuleTextTests
{
    const string Padding = "// ";
    static string Large(string code) => Padding + new string('x', 2500) + "\n" + code;

    [Fact]
    public void Large_file_edits_match_fresh_diagnostics_checksums_and_images_through_recovery()
    {
        string[] edits = ["", "public class C { public static int F() => 1; }", "public class C { public static int F() => 2; }",
            "// 😀\r\npublic class C { public static string F() => \"é😀\"; }", "// 😁\r\npublic class C { public static string F() => \"é😁\"; }",
            "#define X\n#if X\nclass A { }\n#else\nclass B { }\n#endif", "#undef X\n#if X\nclass A { }\n#else\nclass B { }\n#endif",
            "#line 100 \"mapped.cs\"\nclass C { int X => missing; }", "#line default\nclass C { int X => missing; }",
            "class C { int X => 1;", "class C { int X => 1; }", "/// <summary>Changed docs.</summary>\nclass C { }", ""];
        var before = Large("");
        var prior = ModuleCompiler.Parse("Code.cs", before);
        var compiler = new ModuleCompiler("TextProof", []);
        foreach (var edit in edits)
        {
            var after = Large(edit);
            var updated = ModuleText.Update("Code.cs", prior, before, after);
            Equal(updated, ModuleCompiler.Parse("Code.cs", after), compiler);
            prior = updated; before = after;
        }
    }

    [Theory]
    [InlineData(2047, 2047)]
    [InlineData(2048, 2048)]
    [InlineData(2049, 2049)]
    [InlineData(2047, 2049)]
    [InlineData(2049, 2047)]
    [InlineData(2048, 0)]
    [InlineData(0, 2048)]
    public void Threshold_crossings_insertions_and_removals_keep_fresh_build_behavior(int oldLength, int newLength)
    {
        static string Text(int length, int value)
        {
            if (length == 0) return "";
            var code = $"\nclass C {{ int Value => {value}; }}";
            return "//" + new string('x', length - code.Length - 2) + code;
        }
        var before = Text(oldLength, 1); var after = Text(newLength, 2);
        Equal(ModuleText.Update("Code.cs", ModuleCompiler.Parse("Code.cs", before), before, after),
            ModuleCompiler.Parse("Code.cs", after), new ModuleCompiler("Boundary", []));
    }

    [Fact]
    public void Change_ranges_work_for_self_predecessor_ancestors_and_unrelated_texts()
    {
        var before = Large("class C { int Value => 1; }");
        var original = ModuleCompiler.Parse("Code.cs", before);
        var second = before.Replace("=> 1", "=> 2", StringComparison.Ordinal);
        var tree = ModuleText.Update("Code.cs", original, before, second);
        var immediate = tree.GetText();
        var after = before.Replace("=> 1", "=> 3", StringComparison.Ordinal);
        var current = ModuleText.Update("Code.cs", tree, second, after).GetText();
        Assert.Empty(current.GetChangeRanges(current));
        Assert.Equal(1, Assert.Single(current.GetChangeRanges(immediate)).Span.Length);
        foreach (var old in new[] { current, immediate, original.GetText(), SourceText.From("unrelated", Encoding.UTF8) })
            Assert.Equal(after, old.WithChanges(current.GetTextChanges(old)).ToString());
        Assert.Same(tree, ModuleText.Update("Code.cs", tree, second, second));
        Assert.Equal("Renamed.cs", ModuleText.Update("Renamed.cs", tree, second, second).FilePath);
    }

    static void Equal(SyntaxTree actual, SyntaxTree expected, ModuleCompiler compiler)
    {
        Assert.Equal(expected.ToString(), actual.ToString());
        Assert.Equal(expected.FilePath, actual.FilePath);
        Assert.Equal(expected.Options, actual.Options);
        Assert.Equal(expected.GetText().Encoding, actual.GetText().Encoding);
        Assert.Equal(expected.GetText().GetChecksum().ToArray(), actual.GetText().GetChecksum().ToArray());
        Assert.Equal(expected.GetDiagnostics().Select(d => d.ToString()), actual.GetDiagnostics().Select(d => d.ToString()));
        var left = ModuleCompiler.Emit(compiler.Create([actual], false, []));
        var right = ModuleCompiler.Emit(compiler.Create([expected], false, []));
        Assert.Equal(right.Errors.Select(d => d.ToString()), left.Errors.Select(d => d.ToString()));
        Assert.Equal(right.Image is null, left.Image is null);
        if (right.Image is { } image)
        {
            Assert.NotNull(left.Image);
            Assert.Equal(image.Pe, left.Image.Pe);
            Assert.Equal(image.Pdb, left.Image.Pdb);
        }
    }
}

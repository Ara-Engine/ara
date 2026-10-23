using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CodeActions;
using Microsoft.CodeAnalysis.CodeFixes;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Text;

namespace Ara.Analyzers.Tests;

/// <summary>
/// A small Roslyn harness: compiles a gameplay-module source against the framework and
/// Ara.Runtime, runs analyzers, checks <c>[|marked|]</c> spans, and applies code fixes.
/// </summary>
internal static class Verify
{
    const string Usings = "global using System;\nglobal using System.Collections.Generic;\nglobal using System.Linq;\nglobal using System.Numerics;\nglobal using Ara;\n";

    static readonly ImmutableArray<MetadataReference> References =
    [
        .. ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!).Split(Path.PathSeparator)
            .Where(p => Path.GetFileName(p).StartsWith("System.", StringComparison.Ordinal) || Path.GetFileName(p) is "netstandard.dll")
            .Select(p => (MetadataReference)MetadataReference.CreateFromFile(p)),
        MetadataReference.CreateFromFile(typeof(Ara.Behaviour).Assembly.Location),
    ];

    static readonly CSharpParseOptions Parse = new(LanguageVersion.Latest, DocumentationMode.Parse);

    internal static CSharpCompilation Compile(string source) =>
        CSharpCompilation.Create("Game",
            [CSharpSyntaxTree.ParseText(source, Parse, "Game.cs"), CSharpSyntaxTree.ParseText(Usings, Parse, "Usings.cs")],
            References, new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));

    internal static ImmutableArray<Diagnostic> Analyze(string source, DiagnosticAnalyzer analyzer) =>
        Compile(source).WithAnalyzers([analyzer]).GetAnalyzerDiagnosticsAsync().GetAwaiter().GetResult();

    /// <summary>Strips <c>[|...|]</c> markers, returning the plain source and the marked spans.</summary>
    internal static (string Source, List<TextSpan> Spans) Markup(string markup)
    {
        var spans = new List<TextSpan>();
        var sb = new System.Text.StringBuilder();
        var starts = new Stack<int>();
        for (int i = 0; i < markup.Length; i++)
        {
            if (markup.AsSpan(i).StartsWith("[|")) { starts.Push(sb.Length); i++; continue; }
            if (markup.AsSpan(i).StartsWith("|]")) { var s = starts.Pop(); spans.Add(TextSpan.FromBounds(s, sb.Length)); i++; continue; }
            sb.Append(markup[i]);
        }
        return (sb.ToString(), spans);
    }

    /// <summary>Asserts that <paramref name="id"/> is reported exactly on the marked spans.</summary>
    internal static void Diagnostics(string markup, DiagnosticAnalyzer analyzer, string id)
    {
        var (source, spans) = Markup(markup);
        var found = Analyze(source, analyzer).Where(d => d.Id == id).OrderBy(d => d.Location.SourceSpan.Start).ToList();
        var expected = spans.OrderBy(s => s.Start).Select(s => source[s.Start..s.End]).ToList();
        Assert.Equal(expected, found.Select(d => source[d.Location.SourceSpan.Start..d.Location.SourceSpan.End]).ToList());
    }

    /// <summary>Applies the first fix for every <paramref name="id"/> diagnostic until none remain; returns the code.</summary>
    internal static string Fix(string source, DiagnosticAnalyzer analyzer, CodeFixProvider fix, string id)
    {
        using var workspace = new AdhocWorkspace();
        var project = workspace.AddProject("Game", LanguageNames.CSharp)
            .WithMetadataReferences(References)
            .WithParseOptions(Parse)
            .WithCompilationOptions(new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
        project = project.AddDocument("Usings.cs", Usings).Project;
        var doc = project.AddDocument("Game.cs", source);
        for (int round = 0; round < 50; round++)
        {
            var compilation = doc.Project.GetCompilationAsync().GetAwaiter().GetResult()!;
            var diag = compilation.WithAnalyzers([analyzer]).GetAnalyzerDiagnosticsAsync().GetAwaiter().GetResult()
                .Where(d => d.Id == id && d.Location.SourceTree?.FilePath == "Game.cs")
                .OrderBy(d => d.Location.SourceSpan.Start)
                .FirstOrDefault();
            if (diag is null) break;
            var actions = new List<CodeAction>();
            var ctx = new CodeFixContext(doc, diag, (a, _) => actions.Add(a), CancellationToken.None);
            fix.RegisterCodeFixesAsync(ctx).GetAwaiter().GetResult();
            Assert.True(actions.Count > 0, $"no fix offered for {diag}");
            var ops = actions[0].GetOperationsAsync(CancellationToken.None).GetAwaiter().GetResult();
            var changed = ops.OfType<ApplyChangesOperation>().Single().ChangedSolution;
            doc = changed.GetDocument(doc.Id)!;
        }
        return doc.GetTextAsync().GetAwaiter().GetResult().ToString();
    }

    /// <summary>Compiler errors of a source (after fixes, it should have none).</summary>
    internal static List<Diagnostic> Errors(string source) =>
        Compile(source).GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error).ToList();
}

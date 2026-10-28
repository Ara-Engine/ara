using System.Collections.Immutable;
using System.Text.Json;
using Ara.Analyzers.CodeFixes;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CodeActions;
using Microsoft.CodeAnalysis.CodeFixes;
using Microsoft.CodeAnalysis.Text;

namespace Ara.CompileServer;

/// <summary>Source supplied by the authority; the worker never writes project files.</summary>
internal sealed record FixFile(string Path, string Text);
/// <summary>One installed code-fix request over a complete module's sources.</summary>
internal sealed record FixRequest(FixFile[] Files, string[]? Codes = null, string Profile = "gameplay");
internal sealed record AppliedFix(string Code, string Title);
internal sealed record SourceChange(string Path, string Before, string After, AppliedFix[] Fixes);
internal sealed record FixResult(SourceChange[] Changes);

/// <summary>Runs the shipped providers deterministically and returns immutable source replacements.</summary>
internal static class CodeFixRunner
{
    internal static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);
    static readonly CodeFixProvider[] Providers = [new UnityFix(), new TransientFix(), new ApiAccessFix()];

    internal static async Task<FixResult> Run(FixRequest request)
    {
        using var workspace = new AdhocWorkspace();
        var compiler = new ModuleCompiler("AraFix", [typeof(Behaviour).Assembly.Location]);
        var project = workspace.AddProject("AraFix", LanguageNames.CSharp)
            .WithMetadataReferences(compiler.References).WithParseOptions(ModuleCompiler.ParseOptions)
            .WithCompilationOptions(ModuleCompiler.Options(false));
        project = project.AddDocument("Usings.g.cs", ModuleCompiler.GlobalUsings).Project;
        var documents = new List<(DocumentId Id, FixFile File)>();
        foreach (var file in request.Files.OrderBy(f => f.Path, StringComparer.Ordinal))
        {
            var document = project.AddDocument(file.Path, SourceText.From(file.Text), filePath: file.Path);
            project = document.Project;
            documents.Add((document.Id, file));
        }
        var allowed = (request.Codes ?? ["ARA0001", "ARA0101", "ARA0506"]).ToHashSet(StringComparer.Ordinal);
        var analysis = new AnalyzerSet();
        var changes = new List<SourceChange>();
        foreach (var (id, file) in documents)
        {
            var applied = new List<AppliedFix>();
            var document = project.GetDocument(id)!;
            // Re-analyze after each action: node spans and semantic diagnostics change.
            for (int round = 0; round < 512; round++)
            {
                var compilation = (await document.Project.GetCompilationAsync())!;
                var tree = (await document.GetSyntaxTreeAsync())!;
                var diagnostics = analysis.Analyze(compilation, [tree], request.Profile)
                    .Concat(compilation.GetDiagnostics().Where(d => d.Id == "CS1061" && allowed.Contains("ARA0001")))
                    .Where(d => d.Location.SourceTree == tree && (allowed.Contains(d.Id) || d.Id == "CS1061"))
                    .OrderBy(d => d.Location.SourceSpan.Start).ThenBy(d => d.Id, StringComparer.Ordinal);
                var before = (await document.GetTextAsync()).ToString();
                var changed = false;
                foreach (var diagnostic in diagnostics)
                {
                    var provider = Providers.FirstOrDefault(p => p.FixableDiagnosticIds.Contains(diagnostic.Id));
                    if (provider is null) continue;
                    var actions = new List<CodeAction>();
                    await provider.RegisterCodeFixesAsync(new CodeFixContext(document, diagnostic, (action, _) => actions.Add(action), CancellationToken.None));
                    foreach (var action in actions)
                    {
                        var operations = await action.GetOperationsAsync(CancellationToken.None);
                        var next = operations.OfType<ApplyChangesOperation>().SingleOrDefault()?.ChangedSolution.GetDocument(id);
                        if (next is null || (await next.GetTextAsync()).ToString() == before) continue;
                        document = next;
                        applied.Add(new AppliedFix(diagnostic.Id == "CS1061" ? "ARA0001" : diagnostic.Id, action.Title));
                        changed = true;
                        break;
                    }
                    if (changed) break;
                }
                if (!changed) break;
                if (round == 511) throw new InvalidOperationException("code-fix limit exceeded; split the source change");
            }
            project = document.Project;
            var after = (await document.GetTextAsync()).ToString();
            if (after != file.Text) changes.Add(new SourceChange(file.Path, file.Text, after, [.. applied]));
        }
        return new FixResult([.. changes]);
    }
}

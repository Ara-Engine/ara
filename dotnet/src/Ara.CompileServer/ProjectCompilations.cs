using System.Collections.Concurrent;
using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

namespace Ara.CompileServer;

/// <summary>Roslyn state for the active project's modules, retained across sequential preparation requests.</summary>
internal sealed class ProjectCompilations
{
    // Different modules may generate concurrently; one request owns each output.
    readonly ConcurrentDictionary<string, Entry> _modules = new(StringComparer.Ordinal);

    sealed record Entry(string Configuration, CSharpCompilation User, GeneratorDriver Driver,
        Dictionary<string, (string Text, SyntaxTree Tree)> Sources);

    /// <summary>Drop removed modules and other projects without evicting an active project's working set.</summary>
    internal void Retain(IEnumerable<string> outputs)
    {
        var active = new HashSet<string>(outputs, StringComparer.Ordinal);
        foreach (var path in _modules.Keys.Where(path => !active.Contains(path)).ToArray()) _modules.TryRemove(path, out _);
    }

    /// <summary>Read current file contents while offering this module's existing strings for exact comparison.</summary>
    internal ProjectSourceCapture ReadSources(string output, string project, SourceFile[] files)
    {
        _modules.TryGetValue(output, out var previous);
        return ProjectSourceCapture.Read(project, files, previous is null ? null
            : path => previous.Sources.TryGetValue(path, out var source) ? source.Text : null);
    }

    /// <summary>Reuses unchanged parsed text; dependency/configuration changes rebuild semantic and generator state.</summary>
    internal (GeneratorDriver Driver, Compilation Final, ImmutableArray<Diagnostic> Diagnostics, SyntaxTree[] Trees) Generate(
        string output, string configuration, ModuleCompiler compiler, bool hot,
        (string Path, string Text)[] sources, IEnumerable<MetadataReference> references)
    {
        _modules.TryGetValue(output, out var previous);
        var reusable = previous is not null && previous.Configuration == configuration;
        var unchanged = 0;
        var trees = new Dictionary<string, (string Text, SyntaxTree Tree)>(StringComparer.Ordinal);
        foreach (var source in sources)
        {
            // Parse options are fixed for this server. References, heat and the API
            // profile affect binding/generation, not parsing the same source text.
            var text = source.Text;
            SyntaxTree tree;
            if (previous is not null && previous.Sources.TryGetValue(source.Path, out var old))
            {
                tree = ModuleText.Update(source.Path, old.Tree, old.Text, source.Text);
                if (ReferenceEquals(tree, old.Tree))
                {
                    text = old.Text;
                    unchanged++;
                }
            }
            else tree = ModuleCompiler.Parse(source.Path, source.Text);
            trees.Add(source.Path, (text, tree));
        }
        CSharpCompilation user;
        if (reusable && previous!.Sources.Keys.SequenceEqual(trees.Keys, StringComparer.Ordinal))
        {
            user = previous!.User;
            if (trees.Count > 1 && unchanged == 0)
            {
                // No authored tree survives this edit. Replace them together while
                // retaining references, options and the final global-usings tree.
                var usings = user.SyntaxTrees.Last();
                user = user.RemoveAllSyntaxTrees().AddSyntaxTrees(trees.Values.Select(s => s.Tree).Append(usings));
            }
            else
                foreach (var (path, old) in previous.Sources)
                {
                    var now = trees[path];
                    if (!ReferenceEquals(old.Tree, now.Tree)) user = user.ReplaceSyntaxTree(old.Tree, now.Tree);
                }
        }
        else
        {
            // Membership/order changes must match a fresh build's tree order:
            // metadata tokens and deterministic image bytes depend on it.
            user = compiler.Create([.. trees.Values.Select(s => s.Tree),
                ModuleCompiler.Parse("<ara-global-usings>.cs", ModuleCompiler.GlobalUsings)], hot, references);
        }
        var driver = (reusable ? previous!.Driver : ModuleCompiler.Driver())
            .RunGeneratorsAndUpdateCompilation(user, out var final, out var diagnostics);
        _modules[output] = new(configuration, user, driver, trees);
        return (driver, final, diagnostics, [.. trees.Values.Select(s => s.Tree)]);
    }
}

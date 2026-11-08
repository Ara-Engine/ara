using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

namespace Ara.CompileServer;

/// <summary>Bounded, process-local Roslyn state for sequential project-session requests.</summary>
internal sealed class ProjectCompilations
{
    const int Capacity = 8;
    readonly Dictionary<string, Entry> _modules = new(StringComparer.Ordinal);
    long _used;

    sealed record Entry(string Configuration, CSharpCompilation User, GeneratorDriver Driver,
        Dictionary<string, (string Text, SyntaxTree Tree)> Sources, long Used);

    /// <summary>Reuses unchanged syntax and declarations; dependency/configuration changes start a new compilation.</summary>
    internal (GeneratorDriver Driver, Compilation Final, ImmutableArray<Diagnostic> Diagnostics, SyntaxTree[] Trees) Generate(
        string output, string configuration, ModuleCompiler compiler, bool hot,
        (string Path, string Text)[] sources, IEnumerable<MetadataReference> references)
    {
        _modules.TryGetValue(output, out var previous);
        var reusable = previous is not null && previous.Configuration == configuration;
        var trees = new Dictionary<string, (string Text, SyntaxTree Tree)>(StringComparer.Ordinal);
        foreach (var source in sources)
        {
            var tree = reusable && previous!.Sources.TryGetValue(source.Path, out var old) && old.Text == source.Text
                ? old.Tree : ModuleCompiler.Parse(source.Path, source.Text);
            trees.Add(source.Path, (source.Text, tree));
        }
        CSharpCompilation user;
        if (reusable && previous!.Sources.Keys.SequenceEqual(trees.Keys, StringComparer.Ordinal))
        {
            user = previous!.User;
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
        if (!_modules.ContainsKey(output) && _modules.Count == Capacity)
            _modules.Remove(_modules.MinBy(pair => pair.Value.Used).Key);
        _modules[output] = new(configuration, user, driver, trees, ++_used);
        return (driver, final, diagnostics, [.. trees.Values.Select(s => s.Tree)]);
    }
}

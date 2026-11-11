using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>Tree order reaches emitted metadata, so it must follow the file order, never a hash map's per-process order.</summary>
public sealed class TreeOrderTests
{
    [Fact]
    public void The_first_image_compiles_user_trees_in_path_order()
    {
        using var fixture = new Fixture();
        var _directory = fixture.Directory;
        // With 48 files, hash-map order matches path order with probability 1/48!.
        var names = Enumerable.Range(0, 48).Select(i => $"F{(i * 17) % 48:D2}").ToArray();
        foreach (var name in names)
            File.WriteAllText(Path.Combine(_directory, name + ".cs"), $"namespace Order; public static class {name} {{ public static int V => 1; }}");
        var module = new ModuleSession(0, "Game", _directory, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
        var initial = module.Initial(fixture);
        Assert.Equal(OutcomeKind.Image, initial.Kind);
        var order = initial.Next!.Final.SyntaxTrees.Select(tree => Path.GetFileNameWithoutExtension(tree.FilePath))
            .Where(name => name.Length == 3 && name[0] == 'F' && char.IsAsciiDigit(name[1])).ToArray();
        Assert.Equal(names.Order(StringComparer.Ordinal), order);
    }

    sealed class Fixture : IDisposable, ICompileContext
    {
        internal readonly string Directory = InstalledTools.Scratch("tree-order");
        public bool Enc => true;
        public EncCapabilities Caps => EncCapabilities.BodiesOnly;
        public ModuleState? Dependency(string name) => null;
        public bool HasDependents(ModuleSession module) => false;
        public void Dispose() => System.IO.Directory.Delete(Directory, recursive: true);
    }
}

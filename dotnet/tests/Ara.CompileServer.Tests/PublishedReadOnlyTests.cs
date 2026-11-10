using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

public sealed class PublishedReadOnlyTests
{
    const string Good = "public static class Rules { public static int Value() => 1; }";
    const string Broken = "deliberately broken saved dependency source";

    sealed class Context(ModuleSession module) : ICompileContext
    {
        public bool Enc => true;
        public EncCapabilities Caps => EncCapabilities.BodiesOnly;
        public ModuleState? Dependency(string name) => module.State;
        public bool HasDependents(ModuleSession candidate) => false;
        public MetadataReference? Reference(string name) => module.LoadedReference;
        public ImmutableDictionary<string, string>? CommittedTexts(string name) => module.LoadedTexts;
        public IEnumerable<string>? CommittedPaths(string name) => module.LoadedPaths;
    }

    static (string Directory, string Path, PublishedSnapshot Snapshot) Image()
    {
        var directory = InstalledTools.Scratch("readonly-prebuilt");
        var path = System.IO.Path.Combine(directory, "Code.cs");
        File.WriteAllText(path, Good);
        var compiler = new ModuleCompiler("Core", [typeof(Behaviour).Assembly.Location]);
        var (image, errors) = ModuleCompiler.Emit(compiler.Create([ModuleCompiler.Parse(path, Good)], false, []));
        Assert.Empty(errors);
        Assert.NotNull(image);
        var assembly = System.IO.Path.Combine(directory, "Core.dll");
        File.WriteAllBytes(assembly, image.Pe);
        File.WriteAllBytes(System.IO.Path.ChangeExtension(assembly, ".pdb"), image.Pdb);
        File.WriteAllText(path, Broken);
        return (directory, path, PublishedSnapshot.CaptureReadOnly(directory, assembly, "Core", false, [], "gameplay", _ => null));
    }

    [Fact]
    public void A_watched_dependent_initializes_and_edits_against_readonly_pe_without_compiling_bad_dependency_source()
    {
        var (directory, path, snapshot) = Image();
        var sink = new Capture();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        server.Watch(0, "Core", directory, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false, initial: false, published: snapshot);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.True(sink.Empty);
        Assert.Null(server.Dependency("Core"));
        Assert.Null(server.CommittedTexts("Core"));
        Assert.Null(server.CommittedPaths("Core"));
        Assert.True(server.HasOpaqueSourceOwnership);
        var reference = server.Reference("Core");
        Assert.IsAssignableFrom<PortableExecutableReference>(reference);
        server.OnWatcher(0, [path], 0);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.True(sink.Empty);
        var game = InstalledTools.Scratch("readonly-prebuilt-dependent");
        var gamePath = System.IO.Path.Combine(game, "Code.cs");
        const string source = "public static class UsesCore { public static int Value() => Rules.Value() + 10; }";
        File.WriteAllText(gamePath, source);
        server.Watch(1, "Game", game, false, [typeof(Behaviour).Assembly.Location], ["Core"], "gameplay", watch: false);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var first = Assert.Single(sink.Next().Reload());
        Assert.Equal((1u, 0u, AbiConst.ReloadInitial), (first.Slot, first.Generation, first.Kind));
        File.WriteAllText(gamePath, source.Replace("+ 10", "+ 20", StringComparison.Ordinal));
        server.Change(1, [gamePath], AbiConst.OriginHuman, 0);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var edited = Assert.Single(sink.Next().Reload());
        Assert.Equal((1u, 1u, AbiConst.ReloadL3, true), (edited.Slot, edited.Generation, edited.Kind, edited.Hot));
        Assert.Null(server.Dependency("Core"));
        Assert.Same(reference, server.Reference("Core"));
        Assert.Equal(Broken, File.ReadAllText(path));
        Assert.True(sink.Empty);
    }

    [Theory]
    [InlineData("initial")]
    [InlineData("edit")]
    [InlineData("heat")]
    [InlineData("rebuild")]
    [InlineData("cascade")]
    public void Readonly_work_refuses_to_invent_source_or_replace_the_explicit_image(string operation)
    {
        var (directory, path, snapshot) = Image();
        var module = new ModuleSession(0, "Core", directory, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
        Assert.True(module.ImportPublished(snapshot));
        var context = new Context(module);
        var reference = module.LoadedReference;
        Assert.False(module.SavedSourcesChanged(context));
        Assert.False(module.PublishedSourcesDrift);
        var result = operation switch
        {
            "initial" => module.Initial(context),
            "edit" => module.Compile([path], 0, context),
            "heat" => module.Full(Escalation.Requested, true, context),
            "rebuild" => module.Full(Escalation.Requested, null, context),
            _ => module.Cascade(context),
        };
        Assert.Equal(OutcomeKind.Errors, result.Kind);
        Assert.Equal(CompileIntent.CommittedCode, result.Intent);
        Assert.False(result.ValidatedInputs);
        Assert.Null(result.Next);
        Assert.Null(module.State);
        Assert.Null(module.LoadedTexts);
        Assert.Same(reference, module.LoadedReference);
        Assert.Equal(Broken, File.ReadAllText(path));
    }

    [Fact]
    public void Known_readonly_ownership_is_available_without_fabricating_source_bodies()
    {
        var (directory, path, snapshot) = Image();
        var module = new ModuleSession(0, "Core", directory, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", null);
        Assert.True(module.ImportPublished(snapshot with { OwnedPaths = [path] }));
        Assert.Null(module.LoadedTexts);
        Assert.Equal([path], module.LoadedPaths);
        Assert.False(module.SavedSourcesChanged(new Context(module)));
    }
}

using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

public sealed class PublishedSessionTests
{
    const string Good = "public static class Rules { public static int Value() => 1; }";

    sealed class Context(ModuleSession module) : ICompileContext
    {
        public bool Enc => true;
        public EncCapabilities Caps => EncCapabilities.BodiesOnly;
        public ModuleState? Dependency(string name) => name == module.Name ? module.State : null;
        public MetadataReference? Reference(string name) => name == module.Name ? module.LoadedReference : null;
        public ImmutableDictionary<string, string>? CommittedTexts(string name) => name == module.Name ? module.LoadedTexts : null;
        public bool HasDependents(ModuleSession candidate) => false;
    }

    static (ModuleSession Module, Context Context, string Path, PublishedSnapshot Snapshot) Loaded(bool hot)
    {
        var directory = InstalledTools.Scratch("published-session");
        var path = System.IO.Path.Combine(directory, "Code.cs");
        File.WriteAllText(path, Good);
        var compiler = new ModuleCompiler("Core", [typeof(Behaviour).Assembly.Location]);
        var user = compiler.Create([ModuleCompiler.Parse(path, Good)], hot, []);
        var (image, errors) = ModuleCompiler.Emit(user);
        Assert.Empty(errors);
        Assert.NotNull(image);
        var snapshot = new PublishedSnapshot("Core", hot, 0, "published-core", PublishedSnapshot.Hash(image.Pe),
            PublishedSnapshot.Hash(image.Pdb), image.Pe, image.Pdb, [new(path, Good, PublishedSnapshot.TextHash(Good))]);
        var module = new ModuleSession(0, "Core", directory, hot, [typeof(Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
        Assert.True(module.ImportPublished(snapshot));
        Assert.True(module.Loaded);
        Assert.Null(module.State);
        Assert.IsAssignableFrom<PortableExecutableReference>(module.LoadedReference);
        return (module, new Context(module), path, snapshot);
    }

    [Theory]
    [InlineData(null)]
    [InlineData(false)]
    [InlineData(true)]
    public void Full_prepares_captured_committed_text_without_accepting_broken_disk(bool? heat)
    {
        var (module, context, path, _) = Loaded(hot: false);
        var reference = module.LoadedReference;
        File.WriteAllText(path, "this is broken C#");
        var outcome = module.Full(Escalation.Requested, heat, context);
        Assert.Equal(OutcomeKind.Image, outcome.Kind);
        Assert.Equal(CompileIntent.CommittedCode, outcome.Intent);
        Assert.Equal(Good, outcome.Next!.Texts[path]);
        Assert.Equal(1u, outcome.Next.Generation);
        Assert.Equal(heat ?? false, outcome.Next.Hot);
        Assert.False(outcome.ValidatedInputs);
        Assert.False(module.MatchesCurrent(outcome, context));
        Assert.Null(module.State);
        Assert.Same(reference, module.LoadedReference);
        // No stale candidate is committed, and no errorful saved source hitchhikes.
        Assert.Equal(Good, module.LoadedTexts![path]);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void First_edit_emits_a_new_image_before_a_real_l1_baseline_exists(bool hot)
    {
        var (module, context, path, _) = Loaded(hot);
        var second = Good.Replace("=> 1;", "=> 2;", StringComparison.Ordinal);
        File.WriteAllText(path, second);
        var first = module.Compile([path], 0, context);
        Assert.Equal(OutcomeKind.Image, first.Kind);
        Assert.Equal(AbiConst.ReloadL3, first.ReloadKind);
        Assert.Equal(hot ? Escalation.Requested : Escalation.Cold, first.Escalation);
        Assert.True(first.Next!.Hot);
        Assert.Equal(1u, first.Next.Generation);
        Assert.NotNull(first.Next.Baseline);
        Assert.True(module.MatchesCurrent(first, context));
        Assert.Null(module.State);
        module.Commit(first);
        File.WriteAllText(path, second.Replace("=> 2;", "=> 3;", StringComparison.Ordinal));
        var delta = module.Compile([path], 0, context);
        Assert.Equal(OutcomeKind.Delta, delta.Kind);
        Assert.Equal(AbiConst.ReloadL1, delta.ReloadKind);
        Assert.Equal(1u, delta.Next!.Generation);
    }

    [Fact]
    public void A_save_between_capture_and_watch_is_detected_from_captured_texts()
    {
        var (module, _, path, _) = Loaded(hot: false);
        Assert.False(module.PublishedSourcesDrift);
        File.WriteAllText(path, Good.Replace("=> 1;", "=> 2;", StringComparison.Ordinal));
        Assert.True(module.PublishedSourcesDrift);
        Assert.Equal(Good, module.LoadedTexts![path]);
        Assert.Null(module.State);
    }

    [Fact]
    public void Coordinator_detects_a_save_before_the_watcher_exists_without_bootstrap_compilation()
    {
        var (_, _, path, snapshot) = Loaded(hot: false);
        File.WriteAllText(path, Good.Replace("=> 1;", "=> 2;", StringComparison.Ordinal));
        var sink = new Capture();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        server.Watch(0, "Core", System.IO.Path.GetDirectoryName(path)!, false, [typeof(Behaviour).Assembly.Location],
            [], "gameplay", watch: true, initial: false, published: snapshot);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var reload = Assert.Single(sink.Next().Reload());
        Assert.Equal((AbiConst.ReloadL3, 1u, true, Escalation.Cold), (reload.Kind, reload.Generation, reload.Hot, reload.Escalation));
    }

    [Fact]
    public void Invalid_snapshot_uses_ordinary_initialization()
    {
        var (_, _, path, snapshot) = Loaded(hot: false);
        var sink = new Capture();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        server.Watch(0, "Core", System.IO.Path.GetDirectoryName(path)!, false, [typeof(Behaviour).Assembly.Location],
            [], "gameplay", watch: false, initial: false, published: snapshot with { PeHash = "wrong" });
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var reload = Assert.Single(sink.Next().Reload());
        Assert.Equal((AbiConst.ReloadInitial, 0u, false), (reload.Kind, reload.Generation, reload.Hot));
    }

    [Fact]
    public void Unchanged_save_has_a_valid_published_outcome_without_creating_compiled_state()
    {
        var (module, context, path, _) = Loaded(hot: false);
        var reference = module.LoadedReference;
        var outcome = module.Compile([path], 0, context);
        Assert.Equal(OutcomeKind.None, outcome.Kind);
        Assert.Null(outcome.Next);
        Assert.True(module.MatchesCurrent(outcome, context));
        Assert.Null(module.State);
        Assert.Same(reference, module.LoadedReference);
    }

    [Fact]
    public void Peer_drift_after_capture_keeps_loaded_facts_and_rebuilds_at_l3_generation_one()
    {
        var (_, _, corePath, core) = Loaded(hot: false);
        var directory = InstalledTools.Scratch("published-peer-drift");
        var path = System.IO.Path.Combine(directory, "Code.cs");
        const string source = "public static class UsesCore { public static int Value() => Rules.Value() + 10; }";
        File.WriteAllText(path, source);
        var compiler = new ModuleCompiler("Game", [typeof(Behaviour).Assembly.Location]);
        var (image, errors) = ModuleCompiler.Emit(compiler.Create([ModuleCompiler.Parse(path, source)], false, [MetadataReference.CreateFromImage(core.Pe)]));
        Assert.Empty(errors);
        Assert.NotNull(image);
        var game = new PublishedSnapshot("Game", false, 0, "published-game", PublishedSnapshot.Hash(image.Pe),
            PublishedSnapshot.Hash(image.Pdb), image.Pe, image.Pdb, [new(path, source, PublishedSnapshot.TextHash(source))])
        {
            Dependencies = new Dictionary<string, string> { ["Core"] = core.Fingerprint },
        };
        var sink = new Capture();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        server.Watch(0, "Core", System.IO.Path.GetDirectoryName(corePath)!, false, [typeof(Behaviour).Assembly.Location],
            [], "gameplay", watch: false, initial: false, published: core);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.True(sink.Empty);
        File.WriteAllText(corePath, Good.Replace("=> 1;", "=> 2;", StringComparison.Ordinal));
        server.Change(0, [corePath], AbiConst.OriginHuman, 0);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.Equal(1u, Assert.Single(sink.Next().Reload()).Generation);
        server.Watch(1, "Game", directory, false, [typeof(Behaviour).Assembly.Location], ["Core"], "gameplay",
            watch: false, initial: false, published: game);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var rebuilt = Assert.Single(sink.Next().Reload());
        Assert.Equal((AbiConst.ReloadL3, 1u, false, Escalation.Dependency), (rebuilt.Kind, rebuilt.Generation, rebuilt.Hot, rebuilt.Escalation));
        Assert.Equal(source, server.CommittedTexts("Game")![path]);
        Assert.True(sink.Empty);
    }

    /// <summary>
    /// A published module rebuilt from its committed text, or from the files on disk, hands Roslyn the trees in
    /// path order: hash-map order changes per process and tree order reaches the emitted metadata.
    /// </summary>
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void A_published_rebuild_compiles_user_trees_in_path_order(bool fromDisk)
    {
        var directory = InstalledTools.Scratch("published-order");
        try
        {
            // 48 files: hash-map order matches path order with probability 1/48!.
            var names = Enumerable.Range(0, 48).Select(i => $"F{(i * 17) % 48:D2}").ToArray();
            var sources = names.Select(name => (Path: System.IO.Path.Combine(directory, name + ".cs"),
                Text: $"namespace Order; public static class {name} {{ public static int V => 1; }}")).ToArray();
            foreach (var (path, text) in sources) File.WriteAllText(path, text);
            var compiler = new ModuleCompiler("Core", [typeof(Behaviour).Assembly.Location]);
            var user = compiler.Create(sources.Select(source => ModuleCompiler.Parse(source.Path, source.Text)), false, []);
            var (image, errors) = ModuleCompiler.Emit(user);
            Assert.Empty(errors);
            Assert.NotNull(image);
            var snapshot = new PublishedSnapshot("Core", false, 0, "published-core", PublishedSnapshot.Hash(image.Pe),
                PublishedSnapshot.Hash(image.Pdb), image.Pe, image.Pdb,
                [.. sources.Select(source => new PublishedSource(source.Path, source.Text, PublishedSnapshot.TextHash(source.Text)))]);
            var module = new ModuleSession(0, "Core", directory, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
            Assert.True(module.ImportPublished(snapshot));
            var context = new Context(module);
            // Compile reads the folder (a saved edit makes the first rebuild); Full rebuilds the committed text.
            if (fromDisk) File.WriteAllText(sources[0].Path, sources[0].Text.Replace("=> 1;", "=> 2;", StringComparison.Ordinal));
            var outcome = fromDisk ? module.Compile(sources.Select(source => source.Path), 0, context) : module.Full(Escalation.Requested, null, context);
            Assert.Equal(OutcomeKind.Image, outcome.Kind);
            var order = outcome.Next!.Final.SyntaxTrees.Select(tree => System.IO.Path.GetFileNameWithoutExtension(tree.FilePath))
                .Where(name => name.Length == 3 && name[0] == 'F' && char.IsAsciiDigit(name[1])).ToArray();
            Assert.Equal(names.Order(StringComparer.Ordinal), order);
        }
        finally { Directory.Delete(directory, recursive: true); }
    }
}

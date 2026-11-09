using System.Collections.Immutable;
using System.Diagnostics;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

namespace Ara.CompileServer;

/// <summary>Where compile-server messages go (the host pipe, or a test).</summary>
internal interface IMessageSink
{
    /// <summary>Sends one message.</summary>
    void Send(MessageKind kind, byte[] payload);
}

/// <summary>Extra diagnostics run on every build (the Ara analyzers and the API allowlist).</summary>
internal interface IAnalysis
{
    /// <summary>Analyzer diagnostics for <paramref name="trees"/> of <paramref name="compilation"/> under an API profile.</summary>
    ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile);
}

/// <summary>What a compile needs from outside its module.</summary>
internal interface ICompileContext
{
    /// <summary>The host runtime can apply EnC deltas.</summary>
    bool Enc { get; }
    /// <summary>The host runtime's EnC capabilities (they gate L2).</summary>
    EncCapabilities Caps { get; }
    /// <summary>The state a referenced module is compiled against: prepared in the current cascade, or committed.</summary>
    ModuleState? Dependency(string name);
    /// <summary>A compiled or published dependency's exact committed metadata reference.</summary>
    MetadataReference? Reference(string name) => Dependency(name)?.Reference;
    /// <summary>A known compiled public surface; published surfaces may be unknown.</summary>
    string? Surface(string name) => Dependency(name)?.Surface;
    /// <summary>Committed source ownership, including loaded publications without compilations.</summary>
    ImmutableDictionary<string, string>? CommittedTexts(string name) => Dependency(name)?.Texts;
    /// <summary>Known committed ownership without inventing source bodies for read-only images.</summary>
    IEnumerable<string>? CommittedPaths(string name) => CommittedTexts(name)?.Keys;
    /// <summary>An opaque loaded image has no provable prior source ownership.</summary>
    bool HasOpaqueSourceOwnership => false;
    /// <summary>The identity of a still-published loaded image; freshly compiled states have none.</summary>
    string? PublishedFingerprint(string name) => null;
    /// <summary>True if another module references <paramref name="module"/> (its surface hash is needed).</summary>
    bool HasDependents(ModuleSession module);
    /// <summary>One immutable project inventory for a preparation wave; final publication validates a fresh scan.</summary>
    ProjectInventoryResult Inventory(ModuleInputs project) => project.Scan();
}

/// <summary>
/// One module: its committed <see cref="ModuleState"/> (what the running generation was built
/// from), the generator driver and the latest text on disk. Compiles return an
/// <see cref="Outcome"/>; the <see cref="Coordinator"/> commits them (all modules of a
/// cascade, or none). Called only from the coordinator's worker thread.
/// </summary>
/// <remarks>
/// Heat (R10 D1): modules start cold (optimized). The first edit to a cold module is one
/// L3 rebuild as Debug IL that makes it hot; after that body edits take L1 and additive
/// edits L2. A full rebuild can switch heat back ("cool all" / perf play).
/// </remarks>
internal sealed partial class ModuleSession
{
    /// <summary>Soft size cap (R10: about 5k lines).</summary>
    internal const int SoftCapLines = 5000;
    /// <summary>Past this, L3 reloads miss their latency target.</summary>
    internal const int HardCapLines = 8000;

    internal readonly uint Slot;
    internal readonly string Name;
    internal readonly string Dir;
    internal readonly string Profile;
    /// <summary>Names of the modules this one references.</summary>
    internal readonly IReadOnlyList<string> References;
    readonly IAnalysis? _analysis;
    readonly ModuleCompiler _compiler;
    readonly bool _startHot;
    SyntaxTree? _usings;
    SyntaxTree Usings => _usings ??= ModuleCompiler.Parse("<ara-global-usings>.cs", ModuleCompiler.GlobalUsings);
    GeneratorDriver _driver = ModuleCompiler.Driver();
    ModuleState? _state;
    int _sizeLevel;
    readonly ModuleInputs? _project;
    string? _graph;
    readonly Dictionary<string, string> _sourceOwners = new(ModuleState.Paths);
    // This is observed discovery, used only to reject mutations around opaque
    // dependencies; it is not claimed to be the source of their loaded images.
    readonly Dictionary<string, string> _observedSources = new(ModuleState.Paths);
    internal string WatchDirectory => _project is { } project ? Path.Combine(project.Project, project.AuthoringRoot) : Dir;

    // Latest text on disk; differs from the state's texts while the module has compile errors.
    readonly Dictionary<string, string> _disk = new(ModuleState.Paths);

    internal ModuleSession(uint slot, string name, string dir, bool hot, IEnumerable<string> assemblies,
        IReadOnlyList<string> references, string profile, IAnalysis? analysis)
    {
        Slot = slot;
        Name = name;
        Dir = Path.TrimEndingDirectorySeparator(ProjectInventory.ProjectPath(dir));
        _startHot = hot;
        References = references;
        Profile = profile;
        _analysis = analysis;
        _compiler = new ModuleCompiler(name, assemblies);
        _project = ModuleInputs.Read(Dir);
        if (_project is { } project)
        {
            var inventory = project.Scan();
            _graph = Graph(inventory);
            foreach (var source in inventory.Sources)
                _observedSources[ProjectInventory.ProjectPath(Path.Combine(project.Project, source.StoragePath))] = source.Module;
            foreach (var source in inventory.Sources.Where(source => source.Editor))
                _sourceOwners[ProjectInventory.ProjectPath(Path.Combine(project.Project, source.StoragePath))] = source.Module;
        }
    }

    /// <summary>The committed state (null until the first successful build).</summary>
    internal ModuleState? State => _state;
    internal uint InitialGeneration;
    /// <summary>Whether saved bytes differ from this module's last committed input snapshot.</summary>
    internal bool SavedSourcesChanged(ICompileContext context)
    {
        if (LoadedTexts is not { } texts) return false;
        var inventory = _project is { } project ? context.Inventory(project) : null;
        if (inventory is not null && (inventory.Diagnostics.Length != 0 || Graph(inventory) != _graph)) return false;
        return !SourcesMatch(texts, SourceFiles(inventory));
    }

    internal bool MatchesCurrent(Outcome outcome, ICompileContext context) =>
        outcome.Next is { } next ? MatchesCurrentInputs(next, context)
            : outcome.Kind == OutcomeKind.None && _published is { } published && MatchesPublishedInputs(published, context);

    bool MatchesCurrentInputs(ModuleState state, ICompileContext context)
    {
        var current = _project?.Scan();
        if (current is not null && (current.Diagnostics.Length != 0 || Graph(current) != _graph || OpaqueInventoryChanged(current, context) || OwnershipChanged(current, context))) return false;
        if (current is not null) ObserveUnloadedDomains(current, context);
        return MatchesInputs(state, context, SourceFiles(current));
    }

    /// <summary>Adopts an outcome's next state (a delta, image, relink or trivia-only change).</summary>
    internal void Commit(Outcome o)
    {
        if (o.Next is { } next) { _state = next; _published = null; }
        _diagnosticReferencesChanged = !o.ValidatedInputs;
    }

    bool _diagnosticReferencesChanged;

    /// <summary>
    /// After a referenced module committed new code without a cascade (an L1/L2 delta), points
    /// this module's compilations at it so the next edit sees its new members.
    /// </summary>
    internal void Rebind(string name, ModuleState dep)
    {
        if (_state is not { } s || !s.DepRefs.TryGetValue(name, out var old)) return;
        var now = dep.Reference;
        if (ReferenceEquals(old, now)) return;
        _diagnosticReferencesChanged = true;
        _state = s with
        {
            User = s.User.ReplaceReference(old, now),
            Final = s.Final.ReplaceReference(old, now),
            DepRefs = s.DepRefs.SetItem(name, now),
        };
    }

    internal IEnumerable<string> SourceFiles() => SourceFiles(_project?.Scan());

    string[] SourceFiles(ProjectInventoryResult? inventory) => _project is { } project
        ? [.. inventory!.Modules.Single(m => m.Name == Name).Sources.Select(s => ProjectInventory.ProjectPath(Path.Combine(project.Project, s.StoragePath)))]
        : [.. Directory.EnumerateFiles(Dir, "*.cs", SearchOption.AllDirectories).Where(p => !IsIgnored(p)).Order(ModuleState.Paths)];

    // Source membership belongs to its existing module. Only module definitions,
    // dependency edges or domains require rebuilding the player's module graph.
    internal static string Graph(ProjectInventoryResult inventory) => System.Text.Json.JsonSerializer.Serialize(inventory.Modules.Select(m => new { m.Name, m.Directory, m.Hot, m.Profile, m.References }));

    // A removal and an addition spanning owners can be a move even when both
    // modules already exist. Preserve L4 conservatively for that ambiguous case;
    // ordinary additions/removals in one module do not change the loaded graph.
    bool OwnershipChanged(ProjectInventoryResult inventory, ICompileContext context)
    {
        if (_project is not { } project) return false;
        var states = inventory.Modules.Select(module => (module.Name, Paths: context.CommittedPaths(module.Name)))
            .Where(module => module.Paths is not null).ToArray();
        var loaded = new HashSet<string>(states.Select(module => module.Name), StringComparer.Ordinal);
        var previous = new Dictionary<string, string>(ModuleState.Paths);
        // Editor-only domains may not be loaded by a player. Keep their discovered
        // membership while replacing loaded modules with their last-good sources.
        foreach (var (path, owner) in _sourceOwners)
            if (!loaded.Contains(owner)) previous[path] = owner;
        foreach (var module in states)
            foreach (var path in module.Paths!) previous[path] = module.Name;
        var current = inventory.Sources.ToDictionary(source => ProjectInventory.ProjectPath(Path.Combine(project.Project, source.StoragePath)), source => source.Module, ModuleState.Paths);
        var removed = previous.Where(pair => !current.ContainsKey(pair.Key)).Select(pair => pair.Value).ToHashSet(StringComparer.Ordinal);
        return current.Any(pair => !previous.ContainsKey(pair.Key) && removed.Any(owner => owner != pair.Value));
    }

    void ObserveUnloadedDomains(ProjectInventoryResult inventory, ICompileContext context)
    {
        if (_project is not { } project) return;
        _sourceOwners.Clear();
        foreach (var source in inventory.Sources)
            if (source.Editor && context.Reference(source.Module) is null)
                _sourceOwners[ProjectInventory.ProjectPath(Path.Combine(project.Project, source.StoragePath))] = source.Module;
    }

    Outcome? CheckInventory(Phases timing, ICompileContext context, out string[] files)
    {
        files = [];
        if (_project is not { } project)
        {
            files = SourceFiles(null);
            return context.HasOpaqueSourceOwnership && LoadedPaths is { } paths && !paths.ToHashSet(ModuleState.Paths).SetEquals(files)
                ? OpaqueInventoryFailure(timing) : null;
        }
        var inventory = context.Inventory(project);
        if (inventory.Diagnostics.Length > 0)
            return new(this, OutcomeKind.Errors) { Timing = timing, Extra = [.. inventory.Diagnostics.Select(d => ((string)d["code"]!, (string)d["severity"]!, (string)d["message"]!))] };
        if (OpaqueInventoryChanged(inventory, context)) return OpaqueInventoryFailure(timing);
        if (Graph(inventory) != _graph || OwnershipChanged(inventory, context))
        {
            // Compile the entire candidate gameplay graph before asking the player
            // to restart. Broken moves retain the running last-good generation.
            var candidate = ProjectCompiler.Compile(new(project.Project, project.AuthoringRoot), writeOutputs: false);
            if (candidate.Diagnostics.Any(d => (string?)d["severity"] == "error"))
                return new(this, OutcomeKind.Errors) { Timing = timing, Extra = [.. candidate.Diagnostics.Select(d => ((string)d["code"]!, (string)d["severity"]!, (string)d["message"]!))] };
            return new(this, OutcomeKind.Restart) { ReloadKind = AbiConst.ReloadL4, Timing = timing, Escalation = Escalation.Requested };
        }
        files = SourceFiles(inventory);
        // Gameplay membership comes from committed states. Only retain observed
        // unloaded editor sources here; refreshing them handles later editor-file
        // additions without copying the entire project into every module session.
        ObserveUnloadedDomains(inventory, context);
        return null;
    }

    bool OpaqueInventoryChanged(ProjectInventoryResult inventory, ICompileContext context)
    {
        if (!context.HasOpaqueSourceOwnership || _project is not { } project) return false;
        return Graph(inventory) != _graph || inventory.Sources.Length != _observedSources.Count
            || inventory.Sources.Any(source => !_observedSources.TryGetValue(ProjectInventory.ProjectPath(Path.Combine(project.Project, source.StoragePath)), out var owner) || owner != source.Module);
    }

    Outcome OpaqueInventoryFailure(Phases timing) => new(this, OutcomeKind.Errors)
    {
        Timing = timing, ValidatedInputs = false,
        Extra = [(AraCodes.LoadFailed, "error", "Source ownership or module graph changes require republishing the opaque prebuilt dependencies and reloading Play; their committed source ownership is unknown. Loaded code was retained.")],
    };

    internal bool IsIgnored(string path)
    {
        if (_project is not null) return !SourceFiles().Contains(ProjectInventory.ProjectPath(path), ModuleState.Paths);
        var rel = Path.GetRelativePath(Dir, path).Replace('\\', '/');
        if (rel.Split('/').Any(part => part.StartsWith(".", StringComparison.Ordinal)
            || part is "target" or "artifacts" or "node_modules" or "spikes")) return true;
        // A nested module owns its sources; creating a script beside it must not
        // compile that module twice or steal its hot-reload identity.
        var parent = Path.GetDirectoryName(path);
        while (parent is not null && !ModuleState.Paths.Equals(parent, Dir))
        {
            if (File.Exists(Path.Combine(parent, "ara.module.toml"))) return true;
            parent = Path.GetDirectoryName(parent);
        }
        return rel.StartsWith("bin/", StringComparison.OrdinalIgnoreCase) || rel.StartsWith("obj/", StringComparison.OrdinalIgnoreCase)
            || rel.Contains("/bin/", StringComparison.OrdinalIgnoreCase) || rel.Contains("/obj/", StringComparison.OrdinalIgnoreCase);
    }

    (ImmutableDictionary<string, MetadataReference> Refs, ImmutableDictionary<string, string?> Surfaces) Dependencies(ICompileContext ctx)
    {
        var refs = ImmutableDictionary.CreateBuilder<string, MetadataReference>();
        var surfaces = ImmutableDictionary.CreateBuilder<string, string?>();
        foreach (var name in References)
        {
            refs[name] = ctx.Reference(name) ?? throw new InvalidOperationException($"module `{Name}` references `{name}`, which has not built");
            surfaces[name] = ctx.Surface(name);
        }
        return (refs.ToImmutable(), surfaces.ToImmutable());
    }

    /// <summary>Reads every file and builds the first image (generation 0).</summary>
    internal Outcome Initial(ICompileContext ctx, CancellationToken ct = default)
    {
        var p = new Phases(0, Stopwatch.GetTimestamp());
        if (ReadOnlyPublished) return ReadOnlyWork(p);
        if (CheckInventory(p, ctx, out var files) is { } invalid) return invalid;
        var trees = ImmutableDictionary.CreateBuilder<string, SyntaxTree>(ModuleState.Paths);
        var texts = ImmutableDictionary.CreateBuilder<string, string>(ModuleState.Paths);
        _disk.Clear();
        foreach (var path in files)
        {
            var full = ProjectInventory.ProjectPath(path);
            var text = ReadWithRetry(full) ?? throw new FileNotFoundException("module source disappeared", full);
            _disk[full] = text;
            texts[full] = text;
            trees[full] = ModuleCompiler.Parse(full, text);
        }
        var hot = (LoadedHot ?? _startHot) && ctx.Enc;
        var (refs, surfaces) = Dependencies(ctx);
        var user = _compiler.Create([.. trees.Values, Usings], hot, refs.Values);
        var start = new ModuleState(user, user, ImmutableDictionary<string, SourceText>.Empty, trees.ToImmutable(), texts.ToImmutable(),
            null, hot, 0, null, surfaces, refs);
        p.Parse = p.Mark();
        var kind = Loaded ? AbiConst.ReloadL3 : AbiConst.ReloadInitial;
        return Build(user, start, kind, Loaded ? Escalation.Requested : Escalation.None, p, [.. trees.Values], null, ctx, true, ct);
    }

    /// <summary>A full image of the committed state, hot or cold (heat change, or the host asked for one).</summary>
    internal Outcome Full(Escalation escalation, bool? hot, ICompileContext ctx, CancellationToken ct = default)
    {
        if (_state is not { } s)
            return _published is null ? Initial(ctx, ct) : BuildPublished(ctx, new Phases(0, Stopwatch.GetTimestamp()),
                null, CompileIntent.CommittedCode, escalation, hot, ct);
        var h = (hot ?? s.Hot) && ctx.Enc;
        var user = h == s.Hot ? s.User : s.User.WithOptions(ModuleCompiler.Options(h));
        var current = MatchesCurrentInputs(s, ctx);
        var outcome = Build(user, s with { Hot = h }, AbiConst.ReloadL3, escalation, new Phases(0, Stopwatch.GetTimestamp()), [], null, ctx, false, ct);
        outcome.ValidatedInputs = current;
        outcome.Intent = CompileIntent.CommittedCode;
        return outcome;
    }

    /// <summary>
    /// A referenced module got a new generation: relink (reload this module's image and deltas
    /// against it) when every dependency kept the surface this code was compiled against,
    /// otherwise rebuild against the new code (L3, <see cref="Escalation.Dependency"/>).
    /// </summary>
    internal Outcome Cascade(ICompileContext ctx, CancellationToken ct = default, CompileIntent intent = CompileIntent.CurrentSources)
    {
        var timing = new Phases(0, Stopwatch.GetTimestamp());
        if (ReadOnlyPublished) return ReadOnlyWork(timing);
        string[] files = [];
        // Current project candidates may require L4. Explicit committed-code work
        // only validates that drift; it must not compile or request activation of it.
        if (intent == CompileIntent.CurrentSources && CheckInventory(timing, ctx, out files) is { } invalid) return invalid;
        _diagnosticReferencesChanged = true;
        if (_state is null && _published is not null)
            return BuildPublished(ctx, timing, intent == CompileIntent.CurrentSources ? files : null, intent, Escalation.Dependency, null, ct);
        var s = _state ?? throw new InvalidOperationException($"module `{Name}` has not built");
        var (refs, surfaces) = Dependencies(ctx);
        var user = s.User;
        var final = s.Final;
        foreach (var (name, now) in refs)
        {
            if (!s.DepRefs.TryGetValue(name, out var old)) continue;
            user = user.ReplaceReference(old, now);
            final = final.ReplaceReference(old, now);
        }
        var rebound = s with { User = user, Final = final, DepRefs = refs };
        // Rebuild/heat work refers to committed code throughout the cascade. A
        // dependent's unrelated disk edits must never hitchhike on that request.
        var (current, changes) = intent == CompileIntent.CurrentSources
            ? ReadSources(rebound, files) : (rebound, new List<FileChange>());
        if (intent == CompileIntent.CurrentSources && changes.Count > 0 && ctx.Enc && !current.Hot)
            current = current with { User = current.User.WithOptions(ModuleCompiler.Options(true)), Hot = true };
        var validated = intent == CompileIntent.CurrentSources || MatchesCurrentInputs(rebound, ctx);
        timing.Parse = timing.Mark();
        var same = changes.Count == 0 && References.All(n => s.DepSurfaces.TryGetValue(n, out var was) && was is not null && was == surfaces[n]);
        if (same)
        {
            var problems = Problems(final, [], final.GetDiagnostics(ct), []);
            if (problems.Any(d => d.Severity == DiagnosticSeverity.Error))
            {
                var failure = Errors(timing, problems);
                failure.ValidatedInputs = validated;
                failure.Intent = intent;
                return failure;
            }
            return new Outcome(this, OutcomeKind.Relink)
            {
                ReloadKind = AbiConst.ReloadL3,
                Escalation = Escalation.Relink,
                ValidatedInputs = validated,
                Intent = intent,
                Problems = problems,
                Next = s with { User = user, Final = final, DepRefs = refs, Generation = s.Generation + 1 },
            };
        }
        var outcome = Build(current.User, current, AbiConst.ReloadL3, Escalation.Dependency,
            timing, [.. changes.Where(c => c.New is not null).Select(c => c.New!)], null, ctx, false, ct);
        outcome.ValidatedInputs = validated;
        outcome.Intent = intent;
        return outcome;
    }

    static bool MatchesInputs(ModuleState state, ICompileContext ctx, string[] files)
    {
        return SourcesMatch(state, files)
            && state.DepRefs.All(pair => ReferenceEquals(pair.Value, ctx.Reference(pair.Key)));
    }

    static bool SourcesMatch(ModuleState state, string[] files) =>
        SourcesMatch(state.Texts, files);

    static bool SourcesMatch(ImmutableDictionary<string, string> texts, string[] files) =>
        files.Length == texts.Count && files.All(path => texts.TryGetValue(path, out var text) && ReadWithRetry(path) == text);

    /// <summary>Reads source without denying edits or replacements by another world's editor.</summary>
    internal static string? ReadWithRetry(string path)
    {
        var sw = Stopwatch.StartNew();
        while (true)
        {
            try
            {
                if (!File.Exists(path)) return null;
                using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                using var reader = new StreamReader(stream);
                return reader.ReadToEnd();
            }
            catch (IOException) when (sw.ElapsedMilliseconds < 200) { Thread.Yield(); }
        }
    }

    // One immutable membership snapshot per evaluation. Read every current source
    // once: watcher overflow, missed notices and exact last-good restoration must
    // still validate actual bytes rather than trusting timestamps or queued paths.
    (ModuleState State, List<FileChange> Changes) ReadSources(ModuleState state, string[] files)
    {
        _disk.Clear();
        foreach (var path in files)
            _disk[path] = ReadWithRetry(path) ?? throw new FileNotFoundException("module source disappeared while capturing inputs", path);
        var changes = new List<FileChange>();
        var trees = state.Trees.ToBuilder();
        var texts = state.Texts.ToBuilder();
        foreach (var path in _disk.Keys.Union(state.Texts.Keys, ModuleState.Paths))
        {
            var had = state.Texts.TryGetValue(path, out var oldText);
            var has = _disk.TryGetValue(path, out var newText);
            if (had && has && oldText == newText) continue;
            var oldTree = had ? state.Trees[path] : null;
            var newTree = has ? ModuleCompiler.Parse(path, newText!) : null;
            changes.Add(new FileChange(path, oldTree, newTree));
            if (newTree is null) { trees.Remove(path); texts.Remove(path); }
            else { trees[path] = newTree; texts[path] = newText!; }
        }
        var user = state.User;
        foreach (var change in changes) user = Apply(user, change);
        return (state with { User = user, Trees = trees.ToImmutable(), Texts = texts.ToImmutable() }, changes);
    }

    /// <summary>
    /// Classifies and compiles one change set: an L1/L2 delta, an L3 image, nothing (a
    /// trivia-only change), or errors (the last good state stays committed).
    /// </summary>
    internal Outcome Compile(IEnumerable<string> paths, long tEvent, ICompileContext ctx, CancellationToken ct = default)
    {
        var p = new Phases(tEvent, Stopwatch.GetTimestamp());
        if (ReadOnlyPublished) return ReadOnlyWork(p);
        if (CheckInventory(p, ctx, out var files) is { } invalid) return invalid;
        if (_state is not { } s)
            return _published is null ? Initial(ctx, ct) : BuildPublished(ctx, p, files, CompileIntent.CurrentSources,
                _published.Hot ? Escalation.Requested : Escalation.Cold, null, ct, ownEdit: true);
        var (nextState, changes) = ReadSources(s, files);
        if (changes.Count == 0)
        {
            var problems = Problems(s.Final, [], s.Final.GetDiagnostics(ct), []);
            if (problems.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, problems);
            var validated = References.All(name => s.DepRefs.TryGetValue(name, out var reference) && ReferenceEquals(reference, ctx.Reference(name)));
            if (validated) _diagnosticReferencesChanged = false;
            return new Outcome(this, OutcomeKind.None) { Next = s, ValidatedInputs = validated, Problems = problems };
        }
        var next = nextState.User;
        var changed = changes.Where(c => c.New is not null).Select(c => c.New!).ToList();

        var escalation = !ctx.Enc || !s.Hot ? Escalation.Cold : Escalation.None;
        var cls = escalation == Escalation.None ? EditClassifier.Classify(changes, ctx.Caps) : null;
        if (cls is not null) escalation = cls.Escalation;
        p.Parse = p.Mark();
        (Compilation, ImmutableArray<Diagnostic>, ImmutableDictionary<string, SourceText>)? reuse = null;
        if (escalation == Escalation.None && cls is not null)
        {
            // The generator reads declarations, attributes and doc comments only. Body edits keep
            // its output; for inserts and doc edits it runs, and any change in its output is L3.
            if (cls.Additive || EditClassifier.DocsChanged(changes))
            {
                var g = Generate(next, ct);
                p.Generate = p.Mark();
                if (g.Diags.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, Problems(g.Final, g.Diags, [], changed));
                if (!Same(g.Generated, s.Generated)) (escalation, reuse) = (Escalation.Generated, g);
            }
        }
        if (escalation == Escalation.None && cls is not null)
        {
            var final = s.Final;
            foreach (var c in changes) final = Apply(final, c);
            nextState = nextState with { Final = final };
            if (cls.Bodies.Count == 0 && !cls.Additive)
            {
                var checkedProblems = Problems(final, [], final.GetDiagnostics(ct), changed);
                if (checkedProblems.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, checkedProblems);
                return new Outcome(this, OutcomeKind.None) { Next = nextState, Problems = checkedProblems, ValidatedInputs = true };
            }
            ModuleDelta? delta = null;
            ImmutableArray<Diagnostic> errors = [];
            try
            {
                var (edits, added) = EditClassifier.ToSemanticEdits(cls, s.Final, final);
                (delta, errors) = ModuleCompiler.EmitDelta(final, s.Baseline!, edits, added, ct);
            }
            catch (Exception e) when (e is not OperationCanceledException)
            {
                Trace.WriteLine($"module `{Name}`: EmitDifference threw, taking L3: {e.Message}");
            }
            p.Emit = p.Mark();
            var problems = Problems(final, [], errors, changed);
            p.Analyze = p.Mark();
            if (problems.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, problems);
            if (delta is not null)
            {
                var (_, surfaces) = Dependencies(ctx);
                return new Outcome(this, OutcomeKind.Delta)
                {
                    ReloadKind = cls.Additive ? AbiConst.ReloadL2 : AbiConst.ReloadL1,
                    Delta = delta,
                    Changed = cls.Bodies.Count + cls.Inserts.Count,
                    Problems = problems,
                    ValidatedInputs = true,
                    Timing = p,
                    Next = nextState with { Baseline = delta.Next, Surface = cls.Additive ? null : s.Surface, DepSurfaces = surfaces },
                };
            }
            escalation = Escalation.EmitFailed;
        }

        // L3. A cold module's first edit makes it hot (when the host has EnC).
        if (escalation == Escalation.Cold && ctx.Enc && !s.Hot)
        {
            next = next.WithOptions(ModuleCompiler.Options(true));
            nextState = nextState with { User = next, Hot = true };
            reuse = null; // new options: the generator runs for the Debug-IL compilation
        }
        return Build(next, nextState, AbiConst.ReloadL3, escalation, p, changed, reuse, ctx, false, ct);
    }

    static TComp Apply<TComp>(TComp c, FileChange change) where TComp : Compilation => (change.Old, change.New) switch
    {
        ({ } o, { } n) => (TComp)c.ReplaceSyntaxTree(o, n),
        (null, { } n) => (TComp)c.AddSyntaxTrees(n),
        ({ } o, null) => (TComp)c.RemoveSyntaxTrees(o),
        _ => c,
    };

    /// <summary>Same generated output: the driver hands back the same text objects for cached outputs, so most compare by reference.</summary>
    static bool Same(ImmutableDictionary<string, SourceText> a, ImmutableDictionary<string, SourceText> b) =>
        a.Count == b.Count && a.All(kv => b.TryGetValue(kv.Key, out var v) && (ReferenceEquals(v, kv.Value) || v.ContentEquals(kv.Value)));

    Outcome Errors(Phases p, List<Diagnostic> problems) =>
        new(this, OutcomeKind.Errors) { Problems = problems, Timing = p, ValidatedInputs = true };

    /// <summary>Runs the generator over a user compilation.</summary>
    (Compilation Final, ImmutableArray<Diagnostic> Diags, ImmutableDictionary<string, SourceText> Generated) Generate(CSharpCompilation user, CancellationToken ct)
    {
        _driver = _driver.RunGeneratorsAndUpdateCompilation(user, out var final, out var diags, ct);
        var generated = _driver.GetRunResult().Results
            .SelectMany(r => r.GeneratedSources)
            .ToImmutableDictionary(s => s.HintName, s => s.SourceText);
        return (final, diags, generated);
    }

    /// <summary>Errors and Ara diagnostics: generator, compiler (from emit) and analyzers (skipped when there are errors).</summary>
    List<Diagnostic> Problems(Compilation final, ImmutableArray<Diagnostic> generator, ImmutableArray<Diagnostic> emit,
        IReadOnlyCollection<SyntaxTree> changed, Task<ImmutableArray<Diagnostic>>? analysis = null)
    {
        var list = generator.Concat(emit).Where(Diagnostics.Reportable).ToList();
        if (list.All(d => d.Severity != DiagnosticSeverity.Error))
        {
            if (analysis is not null) list.AddRange(analysis.GetAwaiter().GetResult().Where(Diagnostics.Reportable));
            else if (_analysis is not null && AnalysisTrees(final, changed) is { Count: > 0 } trees) list.AddRange(_analysis.Analyze(final, trees, Profile).Where(Diagnostics.Reportable));
        }
        return list;
    }

    IReadOnlyCollection<SyntaxTree> AnalysisTrees(Compilation final, IReadOnlyCollection<SyntaxTree> changed) =>
        _diagnosticReferencesChanged ? final.SyntaxTrees.Where(tree => _disk.ContainsKey(tree.FilePath)).ToArray() : changed;

    /// <summary>
    /// Generator (unless reused), full emit and analyzers (in parallel with the emit, over
    /// <paramref name="analyze"/>), then the EnC baseline. <paramref name="start"/> is the next
    /// state without its compilations, image facts and generation.
    /// </summary>
    Outcome Build(CSharpCompilation user, ModuleState start, uint kind, Escalation escalation, Phases p,
        IReadOnlyCollection<SyntaxTree> analyze,
        (Compilation Final, ImmutableArray<Diagnostic> Diags, ImmutableDictionary<string, SourceText> Generated)? reuse,
        ICompileContext ctx, bool surface, CancellationToken ct)
    {
        var (final, genDiags, generated) = reuse ?? Generate(user, ct);
        p.Generate += p.Mark();
        if (genDiags.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, Problems(final, genDiags, [], analyze));
        var analysisTrees = AnalysisTrees(final, analyze);
        var analysis = _analysis is not null && analysisTrees.Count > 0 ? Task.Run(() => _analysis.Analyze(final, analysisTrees, Profile), ct) : null;
        var (image, errors) = ModuleCompiler.Emit(final, ct);
        p.Emit = p.Mark();
        var problems = Problems(final, genDiags, errors, analyze, analysis);
        p.Analyze = p.Mark();
        if (image is null || problems.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, problems);
        var o = new Outcome(this, OutcomeKind.Image)
        {
            ValidatedInputs = true,
            ReloadKind = kind,
            Escalation = escalation,
            Image = image,
            Problems = problems,
            Timing = p,
            Next = start with
            {
                User = user,
                Final = final,
                Generated = generated,
                Baseline = start.Hot ? ModuleCompiler.Baseline(final, image) : null,
                Generation = kind == AbiConst.ReloadInitial ? InitialGeneration : (_state?.Generation ?? _published?.Generation ?? 0) + 1,
                Surface = surface || ctx.HasDependents(this) ? ModuleCompiler.Surface(final) : null,
                DepSurfaces = Dependencies(ctx).Surfaces,
            },
        };
        CheckSize(o, start.Texts);
        return o;
    }

    /// <summary>Reports the module once each time it crosses the soft cap (info) or 8k lines (warning).</summary>
    void CheckSize(Outcome o, ImmutableDictionary<string, string> texts)
    {
        var lines = texts.Values.Sum(t => t.Count(c => c == '\n'));
        var level = lines > HardCapLines ? 2 : lines > SoftCapLines ? 1 : 0;
        if (level == _sizeLevel) return;
        _sizeLevel = level;
        if (level == 2)
            o.Extra.Add((Diagnostics.ModuleTooLarge, "warning", $"module `{Name}` has {lines} lines; L3 reloads will miss their target past {HardCapLines}; split it into smaller modules"));
        else if (level == 1)
            o.Extra.Add((Diagnostics.ModuleLarge, "info", $"module `{Name}` has {lines} lines, past the soft cap of {SoftCapLines}; consider splitting it"));
    }

    /// <summary>
    /// Runs the delta and full-image paths once without sending anything (S2: the first
    /// EmitDifference costs ~150 ms cold, a second full emit ~200 ms), so the first real edit is warm.
    /// </summary>
    internal void Prewarm(ICompileContext ctx)
    {
        if (_state is not { } s) return;
        var pick = s.Trees.FirstOrDefault(kv => kv.Value.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>()
            .Any(m => m.Body is not null && !m.Body.DescendantNodes().OfType<AnonymousFunctionExpressionSyntax>().Any()));
        if (pick.Value is not { } tree) return;
        var method = tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().First(m => m.Body is not null);
        var edited = ModuleCompiler.Parse(pick.Key, tree.GetRoot().ReplaceNode(method.Body!, method.Body!.AddStatements(SyntaxFactory.EmptyStatement())).ToFullString());
        var change = new FileChange(pick.Key, tree, edited);
        var cls = EditClassifier.Classify([change], ctx.Caps);
        var next = s.User.ReplaceSyntaxTree(tree, edited);
        var warm = _driver;
        if (s.Hot && s.Baseline is not null && cls.Escalation == Escalation.None)
        {
            var final = s.Final.ReplaceSyntaxTree(tree, edited);
            var (edits, added) = EditClassifier.ToSemanticEdits(cls, s.Final, final);
            ModuleCompiler.EmitDelta(final, s.Baseline, edits, added);
        }
        var (debugFinal, _, _) = Generate(next.WithOptions(ModuleCompiler.Options(true)), default);
        ModuleCompiler.Emit(debugFinal);
        _driver = warm;
    }
}

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
    /// <summary>True if another module references <paramref name="module"/> (its surface hash is needed).</summary>
    bool HasDependents(ModuleSession module);
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
internal sealed class ModuleSession
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
    readonly SyntaxTree _usings = ModuleCompiler.Parse("<ara-global-usings>.cs", ModuleCompiler.GlobalUsings);
    GeneratorDriver _driver = ModuleCompiler.Driver();
    ModuleState? _state;
    int _sizeLevel;

    // Latest text on disk; differs from the state's texts while the module has compile errors.
    readonly Dictionary<string, string> _disk = new(ModuleState.Paths);

    internal ModuleSession(uint slot, string name, string dir, bool hot, IEnumerable<string> assemblies,
        IReadOnlyList<string> references, string profile, IAnalysis? analysis)
    {
        Slot = slot;
        Name = name;
        Dir = Path.TrimEndingDirectorySeparator(Path.GetFullPath(dir));
        _startHot = hot;
        References = references;
        Profile = profile;
        _analysis = analysis;
        _compiler = new ModuleCompiler(name, assemblies);
    }

    /// <summary>The committed state (null until the first successful build).</summary>
    internal ModuleState? State => _state;

    /// <summary>Adopts an outcome's next state (a delta, image, relink or trivia-only change).</summary>
    internal void Commit(Outcome o)
    {
        if (o.Next is { } next) _state = next;
    }

    /// <summary>
    /// After a referenced module committed new code without a cascade (an L1/L2 delta), points
    /// this module's compilations at it so the next edit sees its new members.
    /// </summary>
    internal void Rebind(string name, ModuleState dep)
    {
        if (_state is not { } s || !s.DepRefs.TryGetValue(name, out var old)) return;
        var now = dep.Reference;
        if (ReferenceEquals(old, now)) return;
        _state = s with
        {
            User = s.User.ReplaceReference(old, now),
            Final = s.Final.ReplaceReference(old, now),
            DepRefs = s.DepRefs.SetItem(name, now),
        };
    }

    internal IEnumerable<string> SourceFiles() =>
        Directory.EnumerateFiles(Dir, "*.cs", SearchOption.AllDirectories).Where(p => !IsIgnored(p));

    internal bool IsIgnored(string path)
    {
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
            var dep = ctx.Dependency(name) ?? throw new InvalidOperationException($"module `{Name}` references `{name}`, which has not built");
            refs[name] = dep.Reference;
            surfaces[name] = dep.Surface;
        }
        return (refs.ToImmutable(), surfaces.ToImmutable());
    }

    /// <summary>Reads every file and builds the first image (generation 0).</summary>
    internal Outcome Initial(ICompileContext ctx, CancellationToken ct = default)
    {
        var p = new Phases(0, Stopwatch.GetTimestamp());
        var trees = ImmutableDictionary.CreateBuilder<string, SyntaxTree>(ModuleState.Paths);
        var texts = ImmutableDictionary.CreateBuilder<string, string>(ModuleState.Paths);
        foreach (var path in SourceFiles())
        {
            var full = Path.GetFullPath(path);
            var text = File.ReadAllText(full);
            _disk[full] = text;
            texts[full] = text;
            trees[full] = ModuleCompiler.Parse(full, text);
        }
        var hot = _startHot && ctx.Enc;
        var (refs, surfaces) = Dependencies(ctx);
        var user = _compiler.Create([.. trees.Values, _usings], hot, refs.Values);
        var start = new ModuleState(user, user, ImmutableDictionary<string, SourceText>.Empty, trees.ToImmutable(), texts.ToImmutable(),
            null, hot, 0, null, surfaces, refs);
        p.Parse = p.Mark();
        var kind = _state is null ? AbiConst.ReloadInitial : AbiConst.ReloadL3;
        return Build(user, start, kind, _state is null ? Escalation.None : Escalation.Requested, p, [.. trees.Values], null, ctx, true, ct);
    }

    /// <summary>A full image of the committed state, hot or cold (heat change, or the host asked for one).</summary>
    internal Outcome Full(Escalation escalation, bool? hot, ICompileContext ctx, CancellationToken ct = default)
    {
        if (_state is not { } s) return Initial(ctx, ct);
        var h = (hot ?? s.Hot) && ctx.Enc;
        var user = h == s.Hot ? s.User : s.User.WithOptions(ModuleCompiler.Options(h));
        return Build(user, s with { Hot = h }, AbiConst.ReloadL3, escalation, new Phases(0, Stopwatch.GetTimestamp()), [], null, ctx, false, ct);
    }

    /// <summary>
    /// A referenced module got a new generation: relink (reload this module's image and deltas
    /// against it) when every dependency kept the surface this code was compiled against,
    /// otherwise rebuild against the new code (L3, <see cref="Escalation.Dependency"/>).
    /// </summary>
    internal Outcome Cascade(ICompileContext ctx, CancellationToken ct = default)
    {
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
        var same = References.All(n => s.DepSurfaces.TryGetValue(n, out var was) && was is not null && was == surfaces[n]);
        if (same)
        {
            return new Outcome(this, OutcomeKind.Relink)
            {
                ReloadKind = AbiConst.ReloadL3,
                Escalation = Escalation.Relink,
                Next = s with { User = user, Final = final, DepRefs = refs, Generation = s.Generation + 1 },
            };
        }
        return Build(user, s with { DepRefs = refs }, AbiConst.ReloadL3, Escalation.Dependency,
            new Phases(0, Stopwatch.GetTimestamp()), [], null, ctx, false, ct);
    }

    static string? ReadWithRetry(string path)
    {
        var sw = Stopwatch.StartNew();
        while (true)
        {
            try { return File.Exists(path) ? File.ReadAllText(path) : null; }
            catch (IOException) when (sw.ElapsedMilliseconds < 200) { Thread.Yield(); }
        }
    }

    /// <summary>
    /// Classifies and compiles one change set: an L1/L2 delta, an L3 image, nothing (a
    /// trivia-only change), or errors (the last good state stays committed).
    /// </summary>
    internal Outcome Compile(IEnumerable<string> paths, long tEvent, ICompileContext ctx, CancellationToken ct = default)
    {
        var p = new Phases(tEvent, Stopwatch.GetTimestamp());
        if (_state is not { } s) return Initial(ctx, ct);
        foreach (var path in paths)
        {
            if (!path.EndsWith(".cs", StringComparison.OrdinalIgnoreCase) || IsIgnored(path)) continue;
            var full = Path.GetFullPath(path);
            if (ReadWithRetry(full) is { } text) _disk[full] = text;
            else _disk.Remove(full);
        }
        var changes = new List<FileChange>();
        var trees = s.Trees.ToBuilder();
        var texts = s.Texts.ToBuilder();
        foreach (var path in _disk.Keys.Union(s.Texts.Keys, ModuleState.Paths).ToList())
        {
            var had = s.Texts.TryGetValue(path, out var oldText);
            var has = _disk.TryGetValue(path, out var newText);
            if (had && has && oldText == newText) continue;
            var oldTree = had ? s.Trees[path] : null;
            var newTree = has ? ModuleCompiler.Parse(path, newText!) : null;
            changes.Add(new FileChange(path, oldTree, newTree));
            if (newTree is null) { trees.Remove(path); texts.Remove(path); }
            else { trees[path] = newTree; texts[path] = newText!; }
        }
        if (changes.Count == 0) return new Outcome(this, OutcomeKind.None);
        var next = s.User;
        foreach (var c in changes) next = Apply(next, c);
        var changed = changes.Where(c => c.New is not null).Select(c => c.New!).ToList();
        var nextState = s with { User = next, Trees = trees.ToImmutable(), Texts = texts.ToImmutable() };

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
                return new Outcome(this, OutcomeKind.None) { Next = nextState }; // trivia or doc comments only
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
        new(this, OutcomeKind.Errors) { Problems = problems, Timing = p };

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
            else if (_analysis is not null && changed.Count > 0) list.AddRange(_analysis.Analyze(final, changed, Profile).Where(Diagnostics.Reportable));
        }
        return list;
    }

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
        var analysis = _analysis is not null && analyze.Count > 0 ? Task.Run(() => _analysis.Analyze(final, analyze, Profile), ct) : null;
        var (image, errors) = ModuleCompiler.Emit(final, ct);
        p.Emit = p.Mark();
        var problems = Problems(final, genDiags, errors, analyze, analysis);
        p.Analyze = p.Mark();
        if (image is null || problems.Any(d => d.Severity == DiagnosticSeverity.Error)) return Errors(p, problems);
        var o = new Outcome(this, OutcomeKind.Image)
        {
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
                Generation = kind == AbiConst.ReloadInitial ? 0 : (_state?.Generation ?? 0) + 1,
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

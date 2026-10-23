using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Diagnostics;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.CompileServer;

/// <summary>Where a session's messages go (the host pipe, or a test).</summary>
internal interface IMessageSink
{
    /// <summary>Sends one message.</summary>
    void Send(MessageKind kind, byte[] payload);
}

/// <summary>Extra diagnostics run on every build (the Ara analyzers).</summary>
internal interface IAnalysis
{
    /// <summary>Analyzer diagnostics for <paramref name="trees"/> of <paramref name="compilation"/>.</summary>
    ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees);
}

/// <summary>
/// One watched module: its retained compilation (what the running generation was built
/// from), the generator driver, the EnC baseline, the module's heat and its file watcher.
/// All work runs on the session's worker thread, one item at a time.
/// </summary>
/// <remarks>
/// Heat (R10 D1): modules start cold (optimized). The first edit to a cold module is one
/// L3 rebuild as Debug IL that makes it hot; after that body-only edits take L1.
/// <see cref="SetHeat"/> recompiles optimized again ("cool all" / perf play).
/// </remarks>
internal sealed class ModuleSession : IDisposable
{
    /// <summary>Soft size cap (R10: about 5k lines).</summary>
    internal const int SoftCapLines = 5000;
    /// <summary>Past this, L3 reloads miss their latency target.</summary>
    internal const int HardCapLines = 8000;
    const double SettleMs = 1.0;

    readonly IMessageSink _out;
    readonly IAnalysis? _analysis;
    readonly uint _slot;
    readonly string _name;
    readonly string _dir;
    readonly bool _enc;
    readonly ModuleCompiler _compiler;
    readonly BlockingCollection<Action> _work = [];
    readonly BlockingCollection<(string Path, long Ticks)> _events = [];
    readonly CancellationTokenSource _stop = new();
    readonly Thread _worker;
    readonly Thread _eventPump;
    FileSystemWatcher? _watcher;

    // Applied state: exactly what the running generation was built from.
    Dictionary<string, SyntaxTree> _trees = new(StringComparer.OrdinalIgnoreCase);
    Dictionary<string, string> _texts = new(StringComparer.OrdinalIgnoreCase);
    CSharpCompilation _user = null!;
    Compilation _final = null!;
    ImmutableDictionary<string, string> _generated = ImmutableDictionary<string, string>.Empty;
    EmitBaseline? _baseline;
    GeneratorDriver _driver = ModuleCompiler.Driver();
    bool _hot;
    uint _generation;
    int _sizeLevel;
    readonly SyntaxTree _usings = ModuleCompiler.Parse("<ara-global-usings>.cs", ModuleCompiler.GlobalUsings);

    // Latest text on disk; differs from _texts while the module has compile errors.
    readonly Dictionary<string, string> _disk = new(StringComparer.OrdinalIgnoreCase);

    internal ModuleSession(IMessageSink output, uint slot, string name, string dir, bool hot, bool enc,
        IEnumerable<string> references, IAnalysis? analysis)
    {
        _out = output;
        _slot = slot;
        _name = name;
        _dir = Path.GetFullPath(dir);
        _enc = enc;
        _hot = hot && enc;
        _analysis = analysis;
        _compiler = new ModuleCompiler(name, references);
        _worker = new Thread(Work) { Name = $"ara-compile:{name}", IsBackground = true };
        _eventPump = new Thread(Pump) { Name = $"ara-watch:{name}", IsBackground = true };
    }

    /// <summary>
    /// Starts watching (first, so no save is missed), builds the first image, then warms
    /// the L1/L3 paths. Edits made meanwhile queue behind the warm-up.
    /// </summary>
    internal void Start(bool watch)
    {
        _worker.Start();
        _work.Add(() =>
        {
            if (watch) StartWatching();
            if (LoadAndBuild()) Prewarm();
        });
    }

    /// <summary>Recompiles hot (Debug IL, EnC) or cold (optimized), at L3 cost.</summary>
    internal void SetHeat(bool hot) => _work.Add(() =>
    {
        hot &= _enc;
        if (hot == _hot) return;
        _hot = hot;
        _user = _user.WithOptions(ModuleCompiler.Options(hot));
        BuildFull(AbiConst.ReloadL3, Escalation.Requested, Stopwatch.GetTimestamp(), analyze: []);
    });

    /// <summary>Sends a full image of the applied state (the host could not apply a delta).</summary>
    internal void Rebuild(uint escalation) =>
        _work.Add(() => BuildFull(AbiConst.ReloadL3, (Escalation)escalation, Stopwatch.GetTimestamp(), analyze: []));

    /// <summary>Queues a change set as if the watcher had reported it (tests, tools).</summary>
    internal void Change(IEnumerable<string> paths)
    {
        var list = paths.ToList();
        var t = Stopwatch.GetTimestamp();
        _work.Add(() => OnChange(list, t));
    }

    /// <summary>Lets the session's queued work drain (tests).</summary>
    internal void WaitIdle()
    {
        using var done = new ManualResetEventSlim();
        _work.Add(done.Set);
        done.Wait(_stop.Token);
    }

    void Work()
    {
        try
        {
            foreach (var item in _work.GetConsumingEnumerable(_stop.Token))
            {
                try { item(); }
                catch (Exception e) { Log(2, $"module `{_name}`: {e}"); }
                finally { CompileState(false); }
            }
        }
        catch (OperationCanceledException) { }
    }

    void Log(uint level, string text) => _out.Send(MessageKind.Log, new Protocol.Writer().U32(level).Str(text).ToArray());

    bool _compiling;

    void CompileState(bool active)
    {
        if (_compiling == active) return;
        _compiling = active;
        _out.Send(MessageKind.CompileState, new Protocol.Writer().Bool(active).ToArray());
    }

    static ulong Micros(long since) => (ulong)Math.Max(0, Stopwatch.GetElapsedTime(since).Ticks / 10);

    IEnumerable<string> SourceFiles() =>
        Directory.EnumerateFiles(_dir, "*.cs", SearchOption.AllDirectories).Where(p => !IsIgnored(p));

    bool IsIgnored(string path)
    {
        var rel = Path.GetRelativePath(_dir, path).Replace('\\', '/');
        return rel.StartsWith("bin/", StringComparison.OrdinalIgnoreCase) || rel.StartsWith("obj/", StringComparison.OrdinalIgnoreCase)
            || rel.Contains("/bin/", StringComparison.OrdinalIgnoreCase) || rel.Contains("/obj/", StringComparison.OrdinalIgnoreCase);
    }

    bool LoadAndBuild()
    {
        var t0 = Stopwatch.GetTimestamp();
        CompileState(true);
        foreach (var path in SourceFiles())
        {
            var full = Path.GetFullPath(path);
            var text = File.ReadAllText(full);
            _disk[full] = text;
            _texts[full] = text;
            _trees[full] = ModuleCompiler.Parse(full, text);
        }
        _user = _compiler.Create([.. _trees.Values, _usings], _hot);
        return BuildFull(AbiConst.ReloadInitial, Escalation.None, t0);
    }

    /// <summary>Runs the generator over a user compilation.</summary>
    (Compilation Final, ImmutableArray<Diagnostic> Diags, ImmutableDictionary<string, string> Generated) Generate(CSharpCompilation user)
    {
        _driver = _driver.RunGeneratorsAndUpdateCompilation(user, out var final, out var diags);
        var generated = _driver.GetRunResult().Results
            .SelectMany(r => r.GeneratedSources)
            .ToImmutableDictionary(s => s.HintName, s => s.SourceText.ToString());
        return (final, diags, generated);
    }

    /// <summary>Errors and Ara diagnostics of a build: generator, compiler (from emit) and analyzers.</summary>
    List<Diagnostic> Problems(Compilation final, ImmutableArray<Diagnostic> generator, ImmutableArray<Diagnostic> emit, IReadOnlyCollection<SyntaxTree> changed)
    {
        var list = generator.Concat(emit).Where(Diagnostics.Reportable).ToList();
        if (_analysis is not null && list.All(d => d.Severity != DiagnosticSeverity.Error))
            list.AddRange(_analysis.Analyze(final, changed).Where(Diagnostics.Reportable));
        return list;
    }

    /// <summary>Per-phase timings of one reload, logged at info level.</summary>
    sealed class Phases(string what)
    {
        long _t = Stopwatch.GetTimestamp();
        readonly List<string> _parts = [];

        public void Mark(string phase)
        {
            var now = Stopwatch.GetTimestamp();
            _parts.Add($"{phase} {Stopwatch.GetElapsedTime(_t, now).TotalMilliseconds:F1}");
            _t = now;
        }

        public override string ToString() => $"{what}: {string.Join(", ", _parts)} ms";
    }

    /// <summary>
    /// Emits the applied state (plus any pending heat) as a full image and sends it.
    /// <paramref name="analyze"/> are the trees to run the analyzers on (null: all);
    /// <paramref name="generated"/> reuses a generator run for the same compilation.
    /// </summary>
    bool BuildFull(uint kind, Escalation escalation, long t0, IReadOnlyCollection<SyntaxTree>? analyze = null,
        (Compilation Final, ImmutableArray<Diagnostic> Diags, ImmutableDictionary<string, string> Generated)? generated = null,
        Phases? phases = null)
    {
        CompileState(true);
        phases ??= new Phases($"module `{_name}` full build");
        var (final, genDiags, gen) = generated ?? Generate(_user);
        phases.Mark("generate");
        var (image, errors) = genDiags.Any(d => d.Severity == DiagnosticSeverity.Error) ? (null, []) : ModuleCompiler.Emit(final);
        phases.Mark("emit");
        var problems = Problems(final, genDiags, errors, analyze ?? [.. _trees.Values]);
        phases.Mark("analyze");
        SendDiagnostics(problems, t0);
        if (image is null || problems.Any(d => d.Severity == DiagnosticSeverity.Error)) return false;
        _final = final;
        _generated = gen;
        _baseline = _hot ? ModuleCompiler.Baseline(final, image) : null;
        phases.Mark("baseline");
        if (kind != AbiConst.ReloadInitial) _generation++;
        CheckSize();
        _out.Send(MessageKind.Image, new Protocol.Writer()
            .U32(_slot).U32(_generation).Bool(_hot).U32(kind).U32((uint)escalation).U64(Micros(t0))
            .Bytes(image.Pe).Bytes(image.Pdb).ToArray());
        phases.Mark($"send ({(image.Pe.Length + image.Pdb.Length) / 1024} KB)");
        Log(0, phases.ToString());
        return true;
    }

    void SendDiagnostics(List<Diagnostic> problems, long t0, params (string Code, string Severity, string Message)[] extra)
    {
        if (problems.Count == 0 && extra.Length == 0) return;
        var errors = (uint)problems.Count(d => d.Severity == DiagnosticSeverity.Error);
        var json = Diagnostics.Array(w =>
        {
            foreach (var d in problems.Take(50)) Diagnostics.Write(w, d);
            foreach (var (code, sev, msg) in extra) Diagnostics.Write(w, code, sev, msg, _dir);
        });
        _out.Send(MessageKind.Diagnostics, new Protocol.Writer().U32(_slot).U32(errors).Str(json).U64(Micros(t0)).ToArray());
    }

    /// <summary>Reports the module once each time it crosses the soft cap (info) or 8k lines (warning).</summary>
    void CheckSize()
    {
        var lines = _texts.Values.Sum(t => t.Count(c => c == '\n'));
        var level = lines > HardCapLines ? 2 : lines > SoftCapLines ? 1 : 0;
        if (level == _sizeLevel) return;
        _sizeLevel = level;
        if (level == 0) return;
        SendDiagnostics([], Stopwatch.GetTimestamp(), level == 2
            ? (Diagnostics.ModuleTooLarge, "warning", $"module `{_name}` has {lines} lines; L3 reloads will miss their target past {HardCapLines}; split it into smaller modules")
            : (Diagnostics.ModuleLarge, "info", $"module `{_name}` has {lines} lines, past the soft cap of {SoftCapLines}; consider splitting it"));
    }

    void StartWatching()
    {
        _eventPump.Start();
        _watcher = new FileSystemWatcher(_dir, "*.cs")
        {
            IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
            InternalBufferSize = 64 * 1024,
        };
        FileSystemEventHandler on = (_, e) => _events.Add((e.FullPath, Stopwatch.GetTimestamp()));
        _watcher.Changed += on;
        _watcher.Created += on;
        _watcher.Deleted += on;
        _watcher.Renamed += (_, e) =>
        {
            var t = Stopwatch.GetTimestamp();
            _events.Add((e.OldFullPath, t));
            _events.Add((e.FullPath, t));
        };
        _watcher.EnableRaisingEvents = true;
    }

    /// <summary>Coalesces file events (a truncate+write pair reads once) and queues one change set.</summary>
    void Pump()
    {
        var settle = (long)(SettleMs * Stopwatch.Frequency / 1000);
        try
        {
            while (!_stop.IsCancellationRequested)
            {
                var first = _events.Take(_stop.Token);
                var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { first.Path };
                // Spin, don't sleep: Windows timed waits round up to the 15.6 ms timer tick (S2 finding 8).
                var until = Stopwatch.GetTimestamp() + settle;
                while (Stopwatch.GetTimestamp() < until)
                {
                    if (_events.TryTake(out var e)) { paths.Add(e.Path); until = Stopwatch.GetTimestamp() + settle; }
                    else Thread.Yield();
                }
                _work.Add(() => OnChange(paths, first.Ticks));
            }
        }
        catch (OperationCanceledException) { }
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

    /// <summary>Classifies and compiles one change set: an L1 delta, an L3 image, or diagnostics.</summary>
    internal void OnChange(IEnumerable<string> paths, long tEvent)
    {
        var phases = new Phases($"module `{_name}` edit");
        foreach (var p in paths)
        {
            if (!p.EndsWith(".cs", StringComparison.OrdinalIgnoreCase) || IsIgnored(p)) continue;
            var full = Path.GetFullPath(p);
            if (ReadWithRetry(full) is { } text) _disk[full] = text;
            else _disk.Remove(full);
        }
        var changes = new List<FileChange>();
        var nextTrees = new Dictionary<string, SyntaxTree>(_trees, StringComparer.OrdinalIgnoreCase);
        var nextTexts = new Dictionary<string, string>(_texts, StringComparer.OrdinalIgnoreCase);
        foreach (var path in _disk.Keys.Union(_texts.Keys).ToList())
        {
            var had = _texts.TryGetValue(path, out var oldText);
            var has = _disk.TryGetValue(path, out var newText);
            if (had && has && oldText == newText) continue;
            var oldTree = had ? _trees[path] : null;
            var newTree = has ? ModuleCompiler.Parse(path, newText!) : null;
            changes.Add(new FileChange(path, oldTree, newTree));
            if (newTree is null) { nextTrees.Remove(path); nextTexts.Remove(path); }
            else { nextTrees[path] = newTree; nextTexts[path] = newText!; }
        }
        if (changes.Count == 0) return;
        CompileState(true);
        phases.Mark("read+parse");
        var next = _user;
        foreach (var c in changes)
        {
            next = (c.Old, c.New) switch
            {
                ({ } o, { } n) => next.ReplaceSyntaxTree(o, n),
                (null, { } n) => next.AddSyntaxTrees(n),
                ({ } o, null) => next.RemoveSyntaxTrees(o),
                _ => next,
            };
        }
        var changedTrees = changes.Where(c => c.New is not null).Select(c => c.New!).ToList();

        // Body-only edits cannot change the generator's output (it reads declarations,
        // attributes and doc comments only), so L1 skips the generator and keeps the
        // generated trees: the edited user trees are swapped into the last final compilation.
        var escalation = !_enc || !_hot ? Escalation.Cold : Escalation.None;
        List<BodyEdit> bodyEdits = [];
        if (escalation == Escalation.None) (escalation, bodyEdits) = EditClassifier.Classify(changes);
        if (escalation == Escalation.None && EditClassifier.DocsChanged(changes)) escalation = Escalation.Generated;
        phases.Mark("classify");
        if (escalation == Escalation.None)
        {
            var final = _final;
            foreach (var c in changes) final = final.ReplaceSyntaxTree(c.Old!, c.New!);
            if (bodyEdits.Count == 0)
            {
                Adopt(next, final, _generated, nextTrees, nextTexts); // trivia-only change
                return;
            }
            var edits = EditClassifier.ToSemanticEdits(bodyEdits, _final, final);
            ModuleDelta? delta = null;
            ImmutableArray<Diagnostic> errors = [];
            try { (delta, errors) = ModuleCompiler.EmitDelta(final, _baseline!, edits); }
            catch (Exception e) { Log(1, $"EmitDifference threw, taking L3: {e.Message}"); }
            phases.Mark("emit delta");
            var problems = Problems(final, [], errors, changedTrees);
            phases.Mark("analyze");
            if (problems.Any(d => d.Severity == DiagnosticSeverity.Error))
            {
                SendDiagnostics(problems, tEvent);
                return; // a compile error in an edited body: the last good code keeps running
            }
            if (delta is not null)
            {
                SendDiagnostics(problems, tEvent);
                Adopt(next, final, _generated, nextTrees, nextTexts);
                _baseline = delta.Next;
                _out.Send(MessageKind.Delta, new Protocol.Writer()
                    .U32(_slot).U32(_generation).U32((uint)edits.Count).U64(Micros(tEvent))
                    .Bytes(delta.Metadata).Bytes(delta.Il).Bytes(delta.Pdb).ToArray());
                phases.Mark("send");
                Log(0, "L1 " + phases);
                return;
            }
            escalation = Escalation.EmitFailed;
        }

        // L3: run the generator over the new user compilation.
        var (genFinal, genDiags, generated) = Generate(next);
        phases.Mark("generate");
        if (genDiags.Any(d => d.Severity == DiagnosticSeverity.Error))
        {
            SendDiagnostics(Problems(genFinal, genDiags, [], changedTrees), tEvent);
            return;
        }

        // A cold module's first edit makes it hot (when the host has EnC).
        var previous = (_user, _trees, _texts, _hot);
        var reuse = ((Compilation, ImmutableArray<Diagnostic>, ImmutableDictionary<string, string>)?)(genFinal, genDiags, generated);
        if (escalation == Escalation.Cold && _enc && !_hot)
        {
            _hot = true;
            next = next.WithOptions(ModuleCompiler.Options(true));
            reuse = null; // new options: run the generator for the Debug-IL compilation
        }
        (_user, _trees, _texts) = (next, nextTrees, nextTexts);
        if (!BuildFull(AbiConst.ReloadL3, escalation, tEvent, changedTrees, reuse, phases))
            (_user, _trees, _texts, _hot) = previous; // the errors were sent; the last good state stays applied
    }

    bool SameGenerated(ImmutableDictionary<string, string> generated) =>
        generated.Count == _generated.Count && generated.All(kv => _generated.TryGetValue(kv.Key, out var v) && v == kv.Value);

    void Adopt(CSharpCompilation user, Compilation final, ImmutableDictionary<string, string> generated,
        Dictionary<string, SyntaxTree> trees, Dictionary<string, string> texts)
    {
        (_user, _final, _generated, _trees, _texts) = (user, final, generated, trees, texts);
    }

    /// <summary>
    /// Runs each reload path once without sending anything (S2: the first EmitDifference
    /// costs ~150 ms cold, a second full emit ~200 ms), so the first real edit is warm.
    /// </summary>
    void Prewarm()
    {
        var t0 = Stopwatch.GetTimestamp();
        var pick = _trees.FirstOrDefault(kv => kv.Value.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().Any(m => m.Body is not null && !m.Body.DescendantNodes().OfType<AnonymousFunctionExpressionSyntax>().Any()));
        if (pick.Value is not { } tree) return;
        var method = tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().First(m => m.Body is not null);
        var edited = ModuleCompiler.Parse(pick.Key, tree.GetRoot().ReplaceNode(method.Body!, method.Body!.AddStatements(SyntaxFactory.EmptyStatement())).ToFullString());
        var (esc, edits) = EditClassifier.Classify([new FileChange(pick.Key, tree, edited)]);
        var next = _user.ReplaceSyntaxTree(tree, edited);
        var warm = _driver;
        if (_hot && _baseline is not null && esc == Escalation.None)
        {
            var (final, _, _) = Generate(next);
            ModuleCompiler.EmitDelta(final, _baseline, EditClassifier.ToSemanticEdits(edits, _final, final));
        }
        var (debugFinal, _, _) = Generate(next.WithOptions(ModuleCompiler.Options(true)));
        ModuleCompiler.Emit(debugFinal);
        _driver = warm;
        Log(0, $"module `{_name}`: prewarmed in {Stopwatch.GetElapsedTime(t0).TotalMilliseconds:F0} ms");
    }

    /// <summary>Stops watching and the worker.</summary>
    public void Dispose()
    {
        _stop.Cancel();
        _watcher?.Dispose();
    }
}

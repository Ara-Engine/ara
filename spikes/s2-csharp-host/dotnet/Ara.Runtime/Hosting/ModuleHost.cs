using System.Collections.Concurrent;
using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Runtime.CompilerServices;
using System.Runtime.Loader;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.Hosting;

/// <summary>Collectible context for one generation of a gameplay module. Ara.Runtime resolves to the host's copy.</summary>
internal sealed class ModuleLoadContext(string name) : AssemblyLoadContext(name, isCollectible: true)
{
    static readonly Assembly Runtime = typeof(Host).Assembly;

    protected override Assembly? Load(AssemblyName n) => n.Name == Runtime.GetName().Name ? Runtime : null;
}

/// <summary>
/// Owns one gameplay module: its retained compilation, the loaded generation (ALC + assembly + EnC
/// baseline), the file watcher and the compile thread.
///
/// Flow: the watcher queues changed paths; the compile thread reads and parses them, classifies the
/// change, emits an L1 delta or an L3 image, stages it, and blocks until the tick thread applies it in
/// <see cref="TickBoundary"/>. Only the tick thread touches the running module, and only between ticks,
/// so no system is ever on the stack while code is replaced.
/// </summary>
internal sealed class ModuleHost
{
    internal static ModuleHost? Current;

    const double SettleMs = 1.0;

    readonly string _dir;
    readonly bool _debugIL;
    readonly ModuleCompiler _compiler;

    // Applied state: exactly what the running generation was built from. Written only by the tick
    // thread (in TickBoundary) while the compile thread is blocked waiting for it.
    Dictionary<string, SyntaxTree> _trees = new(StringComparer.OrdinalIgnoreCase);
    Dictionary<string, string> _texts = new(StringComparer.OrdinalIgnoreCase);
    CSharpCompilation _comp = null!;
    EmitBaseline? _baseline;
    ModuleLoadContext _alc = null!;
    Assembly _asm = null!;
    uint _generation;
    uint _deltas;
    readonly List<WeakReference> _retired = [];
    internal int AlcsCreated { get; private set; }

    // Latest text read from disk; differs from _texts while the module has compile errors.
    readonly Dictionary<string, string> _disk = new(StringComparer.OrdinalIgnoreCase);

    readonly BlockingCollection<(string path, ulong t)> _events = [];
    readonly CancellationTokenSource _stop = new();
    readonly ManualResetEventSlim _applied = new(false);
    Thread? _compileThread;
    FileSystemWatcher? _watcher;
    Pending? _pending;

    abstract class Pending { internal ReloadReport Report; }
    sealed class PendingDelta : Pending
    {
        internal required ModuleDelta Delta;
        internal required CSharpCompilation Comp;
        internal required Dictionary<string, SyntaxTree> Trees;
        internal required Dictionary<string, string> Texts;
        internal bool Reregister;
    }
    sealed class PendingSwap : Pending
    {
        internal required Loaded Next;
        internal required CSharpCompilation Comp;
        internal required Dictionary<string, SyntaxTree> Trees;
        internal required Dictionary<string, string> Texts;
    }
    sealed class PendingError : Pending;

    /// <summary>A loaded generation, staged off the tick thread.</summary>
    sealed record Loaded(ModuleLoadContext Alc, Assembly Asm, SystemTable Table, EmitBaseline? Baseline, long EmitBytes);

    ModuleHost(string dir, bool debugIL)
    {
        _dir = Path.GetFullPath(dir);
        _debugIL = debugIL;
        _compiler = new ModuleCompiler(debugIL);
    }

    internal bool HasPending => Volatile.Read(ref _pending) is not null;

    /// <summary>Compiles every .cs file under <paramref name="dir"/>, loads it and installs its systems.</summary>
    internal static bool Load(string dir, bool debugIL, bool watch)
    {
        var host = new ModuleHost(dir, debugIL);
        var sw = Stopwatch.StartNew();
        foreach (var path in Directory.EnumerateFiles(host._dir, "*.cs", SearchOption.AllDirectories))
        {
            var text = File.ReadAllText(path);
            host._disk[path] = text;
            host._texts[path] = text;
            host._trees[path] = host._compiler.Parse(path, text);
        }
        var parseMs = sw.Elapsed.TotalMilliseconds;
        host._comp = host._compiler.Create(host._trees.Values);
        var (image, errors) = ModuleCompiler.Emit(host._comp);
        var emitMs = sw.Elapsed.TotalMilliseconds - parseMs;
        if (image is null)
        {
            foreach (var e in errors.Take(20)) Host.Log(2, e.ToString());
            return false;
        }
        var loaded = host.LoadGeneration(image, host._comp);
        host.Install(loaded);
        var lines = host._texts.Values.Sum(t => t.Count(c => c == '\n'));
        Host.Log(0, $"module '{dir}': {host._trees.Count} files, {lines} lines, {(debugIL ? "Debug IL (hot)" : "Release IL (cold)")}, " +
                    $"refs={host._compiler.References.Length} from {host._compiler.ReferenceSource}; " +
                    $"parse {parseMs:F0} ms, create+emit {emitMs:F0} ms, total {sw.Elapsed.TotalMilliseconds:F0} ms; " +
                    $"EnC {(MetadataUpdater.IsSupported ? "supported" : "NOT supported")}");
        Current = host;
        if (watch) host.StartWatching();
        return true;
    }

    /// <summary>Loads an image into a fresh collectible ALC, registers its systems and pre-JITs them.</summary>
    Loaded LoadGeneration(ModuleImage image, CSharpCompilation comp)
    {
        var sw = Stopwatch.StartNew();
        var (g0, g1, g2, pause) = (GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2), GC.GetTotalPauseDuration());
        var alc = new ModuleLoadContext($"game-{AlcsCreated++}");
        Assembly asm;
        if (Environment.GetEnvironmentVariable("ARA_LOAD_FROM_FILE") == "1")
        {
            var dir = Path.Combine(Path.GetTempPath(), "ara-s2", Environment.ProcessId.ToString());
            Directory.CreateDirectory(dir);
            var dll = Path.Combine(dir, $"Game.{AlcsCreated}.dll");
            File.WriteAllBytes(dll, image.Pe);
            File.WriteAllBytes(Path.ChangeExtension(dll, ".pdb"), image.Pdb);
            asm = alc.LoadFromAssemblyPath(dll);
        }
        else if (Environment.GetEnvironmentVariable("ARA_LOAD_NO_PDB") == "1")
            asm = alc.LoadFromStream(new MemoryStream(image.Pe));
        else
            asm = alc.LoadFromStream(new MemoryStream(image.Pe), new MemoryStream(image.Pdb));
        var tLoad = sw.Elapsed.TotalMilliseconds;
        var gcInfo = $"GCs during load: gen0 +{GC.CollectionCount(0) - g0} gen1 +{GC.CollectionCount(1) - g1} gen2 +{GC.CollectionCount(2) - g2}, pause +{(GC.GetTotalPauseDuration() - pause).TotalMilliseconds:F1} ms";
        var table = Register(asm);
        var tRegister = sw.Elapsed.TotalMilliseconds;
        foreach (var e in table.Entries) RuntimeHelpers.PrepareMethod(e.System.Method.MethodHandle);
        var tPrepare = sw.Elapsed.TotalMilliseconds;
        var baseline = _debugIL ? ModuleCompiler.CreateBaseline(comp, image) : null;
        Host.Log(0, $"stage: load {tLoad:F1} ms, register {tRegister - tLoad:F1} ms, prepare {tPrepare - tRegister:F1} ms, baseline {sw.Elapsed.TotalMilliseconds - tPrepare:F1} ms; {gcInfo}");
        return new Loaded(alc, asm, table, baseline, image.Pe.Length + image.Pdb.Length);
    }

    static SystemTable Register(Assembly asm)
    {
        var register = asm.GetExportedTypes()
            .Select(t => t.GetMethod("Register", BindingFlags.Public | BindingFlags.Static, [typeof(SystemRegistry)]))
            .Single(m => m is not null)!;
        var registry = new SystemRegistry();
        register.CreateDelegate<Action<SystemRegistry>>()(registry);
        return new SystemTable([.. registry.Entries]);
    }

    void Install(Loaded l)
    {
        _alc = l.Alc;
        _asm = l.Asm;
        _baseline = l.Baseline;
        SystemTable.Current = l.Table;
    }

    void StartWatching()
    {
        _compileThread = new Thread(CompileLoop) { Name = "ara-compile", IsBackground = true };
        _compileThread.Start();
        _watcher = new FileSystemWatcher(_dir, "*.cs")
        {
            IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
            InternalBufferSize = 64 * 1024,
        };
        FileSystemEventHandler onChange = (_, e) => _events.Add((e.FullPath, Host.Now()));
        _watcher.Changed += onChange;
        _watcher.Created += onChange;
        _watcher.Deleted += onChange;
        _watcher.Renamed += (_, e) => { var t = Host.Now(); _events.Add((e.OldFullPath, t)); _events.Add((e.FullPath, t)); };
        _watcher.EnableRaisingEvents = true;
    }

    void CompileLoop()
    {
        var settleTicks = (long)(SettleMs * Stopwatch.Frequency / 1000);
        while (!_stop.IsCancellationRequested)
        {
            (string path, ulong t) first;
            try { first = _events.Take(_stop.Token); }
            catch (OperationCanceledException) { return; }
            var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { first.path };
            // Settle briefly so a truncate+write pair is read once. Spin rather than sleep: Windows
            // timed waits round up to the 15.6 ms timer tick.
            var until = Stopwatch.GetTimestamp() + settleTicks;
            while (Stopwatch.GetTimestamp() < until)
            {
                if (_events.TryTake(out var e)) { paths.Add(e.path); until = Stopwatch.GetTimestamp() + settleTicks; }
                else Thread.Yield();
            }
            try { Process(paths, first.t); }
            catch (Exception ex) { Host.Log(2, "reload failed: " + ex); }
        }
    }

    /// <summary>Reads, classifies and compiles one batch of changed paths, then waits for the tick thread to apply it.</summary>
    void Process(HashSet<string> paths, ulong tEvent)
    {
        var tStart = Host.Now();
        foreach (var path in paths)
        {
            if (!path.EndsWith(".cs", StringComparison.OrdinalIgnoreCase)) continue;
            var full = Path.GetFullPath(path);
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
            var newTree = has ? _compiler.Parse(path, newText!) : null;
            changes.Add(new FileChange(path, oldTree, newTree));
            if (newTree is null) { nextTrees.Remove(path); nextTexts.Remove(path); }
            else { nextTrees[path] = newTree; nextTexts[path] = newText!; }
        }
        if (changes.Count == 0) return;

        var report = new ReloadReport { TEvent = tEvent, TParsed = Host.Now() };
        Host.Log(0, $"process: settle {Host.Ms(tEvent, tStart):F1} ms, read+diff+parse {Host.Ms(tStart, report.TParsed):F1} ms ({paths.Count} paths, {changes.Count} changed)");
        var next = _comp;
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

        // L1: body-only edits of a hot module, no debugger.
        var escalation = !_debugIL ? Escalation.ColdModule
            : Debugger.IsAttached ? Escalation.Debugger
            : Escalation.None;
        List<BodyEdit> bodyEdits = [];
        if (escalation == Escalation.None) (escalation, bodyEdits) = EditClassifier.Classify(changes);
        report.TClassified = Host.Now();
        if (escalation == Escalation.None && bodyEdits.Count == 0) return; // trivia-only change

        if (escalation == Escalation.None)
        {
            var edits = EditClassifier.ToSemanticEdits(bodyEdits, _comp, next);
            ModuleDelta? delta = null;
            try { (delta, _) = ModuleCompiler.EmitDelta(next, _baseline!, edits); }
            catch (Exception ex) { Host.Log(1, "EmitDifference threw, escalating to L3: " + ex.Message); }
            report.TEmitted = Host.Now();
            if (delta is not null)
            {
                report.Kind = 1;
                report.ChangedMembers = (uint)edits.Count;
                report.EmitBytes = (ulong)(delta.Metadata.Length + delta.Il.Length + delta.Pdb.Length);
                report.TStaged = Host.Now();
                Stage(new PendingDelta
                {
                    Report = report, Delta = delta, Comp = next, Trees = nextTrees, Texts = nextTexts,
                    Reregister = EditClassifier.TouchesRegister(bodyEdits),
                });
                return;
            }
            escalation = Escalation.EmitFailed;
        }

        // L3: full emit into a new collectible ALC.
        report.Escalation = (uint)escalation;
        var (image, errors) = ModuleCompiler.Emit(next);
        report.TEmitted = Host.Now();
        if (image is null)
        {
            foreach (var e in errors.Take(5)) Host.Log(1, "compile error (last good code keeps running): " + e);
            report.Kind = 100;
            report.ErrorCount = (uint)errors.Length;
            report.TStaged = report.TEmitted;
            Stage(new PendingError { Report = report });
            return;
        }
        var loaded = LoadGeneration(image, next);
        report.Kind = 3;
        report.EmitBytes = (ulong)loaded.EmitBytes;
        report.TStaged = Host.Now();
        Stage(new PendingSwap { Report = report, Next = loaded, Comp = next, Trees = nextTrees, Texts = nextTexts });
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

    void Stage(Pending p)
    {
        _applied.Reset();
        Volatile.Write(ref _pending, p);
        try { _applied.Wait(_stop.Token); }
        catch (OperationCanceledException) { }
    }

    /// <summary>
    /// Called by the tick thread between ticks. Applies a staged delta (ApplyUpdate) or swap (new
    /// system table, old ALC unloaded). ECS state is untouched: it lives in Rust columns.
    /// Returns bit 1 if code changed and bit 2 if the system list changed.
    /// </summary>
    internal uint TickBoundary()
    {
        var p = Interlocked.Exchange(ref _pending, null);
        if (p is null) return 0;
        uint flags = 0;
        var before = SystemTable.Current;
        switch (p)
        {
            case PendingDelta d:
                MetadataUpdater.ApplyUpdate(_asm, d.Delta.Metadata, d.Delta.Il, d.Delta.Pdb);
                _baseline = d.Delta.Next;
                (_comp, _trees, _texts) = (d.Comp, d.Trees, d.Texts);
                _deltas++;
                if (d.Reregister) SystemTable.Current = Register(_asm);
                SystemTable.Current.ClearFaults();
                flags = 1;
                break;
            case PendingSwap s:
                var old = _alc;
                Install(s.Next);
                (_comp, _trees, _texts) = (s.Comp, s.Trees, s.Texts);
                _generation++;
                _deltas = 0;
                old.Unload();
                _retired.Add(new WeakReference(old));
                flags = 1;
                break;
        }
        if (!before.SameShapeAs(SystemTable.Current)) flags |= 2;
        p.Report.Generation = _generation;
        p.Report.DeltaCount = _deltas;
        p.Report.TApplied = Host.Now();
        Host.ReportReload(p.Report);
        _applied.Set();
        return flags;
    }

    internal int AlcsAlive() => _retired.Count(w => w.IsAlive);

    /// <summary>
    /// Warms the L1 and L3 paths (classifier, EmitDifference, full emit, ALC load) without applying
    /// anything, so the first real edit runs on a warm compiler. Logs each step's cold cost.
    /// </summary>
    internal void Prewarm()
    {
        var sw = Stopwatch.StartNew();
        double Lap() { var ms = sw.Elapsed.TotalMilliseconds; sw.Restart(); return ms; }

        // A body-only change to the first method with a block body: classify + EmitDifference (discarded).
        var (path, tree) = _trees.First(kv => kv.Value.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().Any(m => m.Body is not null));
        var method = tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().First(m => m.Body is not null);
        var newRoot = tree.GetRoot().ReplaceNode(method.Body!, method.Body!.AddStatements(SyntaxFactory.EmptyStatement()));
        var newTree = _compiler.Parse(path, newRoot.ToFullString());
        var (esc, edits) = EditClassifier.Classify([new FileChange(path, tree, newTree)]);
        var classifyMs = Lap();
        var next = _comp.ReplaceSyntaxTree(tree, newTree);
        double deltaMs = 0;
        if (_debugIL && esc == Escalation.None)
        {
            var (delta, errs) = ModuleCompiler.EmitDelta(next, _baseline!, EditClassifier.ToSemanticEdits(edits, _comp, next));
            deltaMs = Lap();
            if (delta is null) Host.Log(1, "prewarm: delta failed: " + string.Join("; ", errs));
            // A second delta measures the warm path.
            ModuleCompiler.EmitDelta(next, _baseline!, EditClassifier.ToSemanticEdits(edits, _comp, next));
        }
        var warmDeltaMs = Lap();
        var (image, _) = ModuleCompiler.Emit(next);
        var emitMs = Lap();
        var l = LoadGeneration(image!, next);
        l.Alc.Unload();
        _retired.Add(new WeakReference(l.Alc));
        var loadMs = Lap();
        if (Environment.GetEnvironmentVariable("ARA_LOAD_PROBE") == "1") LoadProbe(image!);
        Host.Log(0, $"prewarm: classify {classifyMs:F1} ms, first delta {deltaMs:F1} ms, warm delta {warmDeltaMs:F1} ms, " +
                    $"full emit {emitMs:F1} ms, ALC load+register+prepare {loadMs:F1} ms");
    }

    /// <summary>Experiment: what does loading an image cost, by context kind and image size?</summary>
    void LoadProbe(ModuleImage image)
    {
        var tiny = ModuleCompiler.Emit(_compiler.Create([_compiler.Parse("tiny.cs", "namespace T; public static class X { public static int F() => 1; }")])).image!;
        foreach (var (label, img) in new[] { ("module", image), ("tiny", tiny) })
        foreach (var collectible in new[] { true, false })
        foreach (var withPdb in new[] { true, false })
        {
            var times = new List<double>();
            for (int i = 0; i < 5; i++)
            {
                var sw = Stopwatch.StartNew();
                var alc = new AssemblyLoadContext($"probe-{label}-{i}", collectible);
                if (withPdb) alc.LoadFromStream(new MemoryStream(img.Pe), new MemoryStream(img.Pdb));
                else alc.LoadFromStream(new MemoryStream(img.Pe));
                times.Add(sw.Elapsed.TotalMilliseconds);
                if (collectible) alc.Unload();
            }
            Host.Log(0, $"load probe: {label} ({img.Pe.Length / 1024} KB PE) collectible={collectible} pdb={withPdb}: {string.Join(", ", times.Select(t => t.ToString("F1")))} ms");
        }
    }

    /// <summary>Stops watching, drops the module and reports how many retired ALCs failed to unload.</summary>
    internal int Shutdown()
    {
        _stop.Cancel();
        _watcher?.Dispose();
        _applied.Set();
        _compileThread?.Join(2000);
        SystemTable.Current = new SystemTable([]);
        _alc.Unload();
        _retired.Add(new WeakReference(_alc));
        _alc = null!;
        _asm = null!;
        _pending = null;
        // Unload can lag behind tiered JIT bookkeeping, so retry with short pauses for up to ~2 s.
        var sw = Stopwatch.StartNew();
        for (int i = 0; i < 40 && _retired.Any(w => w.IsAlive); i++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
            if (_retired.Any(w => w.IsAlive)) Thread.Sleep(50);
        }
        var leaked = AlcsAlive();
        Host.Log(leaked == 0 ? 0u : 1u, $"shutdown: {_retired.Count} ALCs retired, {leaked} still alive after {sw.ElapsedMilliseconds} ms of GC");
        return leaked;
    }
}

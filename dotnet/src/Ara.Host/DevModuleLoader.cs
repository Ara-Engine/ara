using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Runtime.CompilerServices;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>
/// The dev-build module loader. Each module generation lives in its own collectible
/// ALC; images and EnC deltas come from the out-of-process compile server (or, without
/// one, from a prebuilt assembly). Everything is staged off the tick thread (ALC load,
/// EnC chain, registration delegate, pre-JIT) and applied by the runtime at the next tick
/// boundary; the previous generation's ALC is unloaded once the new one is installed.
/// </summary>
/// <remarks>
/// A module that references another resolves it to that module's latest staged generation,
/// fixed when its own ALC is created. A <see cref="MessageKind.Reload"/> carrying several
/// modules (a dependency cascade) is staged and swapped as one update. A relink entry (no
/// image) reloads the module's last image and re-applies its EnC deltas, in order.
/// </remarks>
public sealed class DevModuleLoader : IModuleLoader
{
    sealed class Module(uint slot, string name, IReadOnlyList<string> references)
    {
        public readonly uint Slot = slot;
        public readonly string Name = name;
        public readonly IReadOnlyList<string> References = references;
        // Installed (tick thread, under the loader lock).
        public ModuleLoadContext? Alc;
        public Assembly? Asm;
        public uint Generation;
        // Staged view (compile-client thread): the image and the deltas the server built on.
        public byte[] Pe = [];
        public byte[] Pdb = [];
        public readonly List<(byte[] Md, byte[] Il, byte[] Pdb)> Chain = [];
        public Assembly? Staged;
        public readonly ManualResetEventSlim Ready = new(false);
        public bool Failed;
    }

    sealed record Retired(WeakReference Alc, uint Slot, string Name, int Gen2)
    {
        public bool Reported;
    }

    readonly List<Module> _modules = [];
    readonly List<Retired> _retired = [];
    readonly object _lock = new();
    int _created;
    CompileClient? _client;

    /// <inheritdoc/>
    public bool CanReload => _client is not null;

    /// <inheritdoc/>
    public int AlcsCreated => _created;

    /// <inheritdoc/>
    public int AlcsAlive
    {
        get { lock (_lock) return _retired.Count(r => r.Alc.IsAlive); }
    }

    /// <inheritdoc/>
    public int Load(string configJson)
    {
        using var doc = JsonDocument.Parse(configJson);
        var root = doc.RootElement;
        var server = root.TryGetProperty("compileServer", out var cs) && cs.ValueKind == JsonValueKind.String ? cs.GetString() : null;
        var waiting = new List<Module>();
        var rc = 0;
        foreach (var spec in root.GetProperty("modules").EnumerateArray())
        {
            var name = spec.GetProperty("name").GetString() ?? "Module";
            var dir = spec.GetProperty("dir").GetString() ?? "";
            var hot = spec.TryGetProperty("hot", out var h) && h.GetBoolean();
            var watch = spec.TryGetProperty("watch", out var w) && w.GetBoolean();
            var assembly = spec.TryGetProperty("assembly", out var a) && a.ValueKind == JsonValueKind.String ? a.GetString() : null;
            var refs = spec.TryGetProperty("references", out var r) ? r.EnumerateArray().Select(x => x.GetString() ?? "").ToList() : [];
            var profile = spec.TryGetProperty("profile", out var p) ? p.GetString() ?? "gameplay" : "gameplay";
            Module m;
            lock (_lock)
            {
                m = new Module((uint)_modules.Count, name, refs);
                _modules.Add(m);
            }
            if (assembly is not null && !watch)
            {
                // Modules arrive in dependency order: what this one references is staged (or failed) first.
                foreach (var dep in refs.Select(Find)) dep?.Ready.Wait(TimeSpan.FromMinutes(3));
                var report = new ReloadReport { Kind = AbiConst.ReloadInitial, TReceived = Host.Now() };
                try
                {
                    var pdbPath = Path.ChangeExtension(assembly, ".pdb");
                    Stage([(m, File.ReadAllBytes(assembly), File.Exists(pdbPath) ? File.ReadAllBytes(pdbPath) : [], 0, hot, report)]);
                }
                catch (Exception e)
                {
                    Diag.Report(AraCodes.LoadFailed, "error", $"module `{name}`: cannot load `{assembly}`: {e.Message}");
                    m.Failed = true;
                    m.Ready.Set();
                    rc = 1;
                }
                continue;
            }
            if (server is null)
            {
                Diag.Report(AraCodes.LoadFailed, "error", $"module `{name}` has no prebuilt assembly and no compile server is configured");
                m.Failed = true;
                m.Ready.Set();
                rc = 1;
                continue;
            }
            try
            {
                _client ??= new CompileClient(server, this);
                _client.Watch(m.Slot, name, dir, hot, refs, profile, watch);
                waiting.Add(m);
            }
            catch (Exception e)
            {
                Diag.Report(AraCodes.CompileServer, "error", $"compile server: {e.Message}");
                return 1;
            }
        }
        foreach (var m in waiting)
        {
            if (!m.Ready.Wait(TimeSpan.FromMinutes(3)))
            {
                Diag.Report(AraCodes.CompileServer, "error", $"module `{m.Name}`: the compile server sent no image within 3 minutes");
                rc = 1;
            }
            else if (m.Failed) rc = 1;
        }
        return rc;
    }

    Module? Find(uint slot)
    {
        lock (_lock) return slot < _modules.Count ? _modules[(int)slot] : null;
    }

    Module? Find(string name)
    {
        lock (_lock) return _modules.FirstOrDefault(m => m.Name == name);
    }

    /// <summary>
    /// Loads each image (or relinks: the module's last image plus its deltas) into a fresh
    /// collectible ALC, resolving referenced modules to their staged generations, runs the
    /// generated <c>Register</c> and pre-JITs the entry points (compile-client thread), then
    /// queues one swap for the next tick boundary. If any module fails, none is staged.
    /// </summary>
    void Stage(IReadOnlyList<(Module M, byte[] Pe, byte[] Pdb, uint Generation, bool Hot, ReloadReport Report)> batch)
    {
        Dictionary<string, Assembly> view;
        lock (_lock) view = _modules.Where(m => m.Staged is not null).ToDictionary(m => m.Name, m => m.Staged!);
        var staged = new List<(Module M, ModuleLoadContext Alc, Assembly Asm, ModuleSwap Swap, bool Relink, byte[] Pe, byte[] Pdb)>();
        try
        {
            foreach (var (m, pe, pdb, generation, hot, r) in batch)
            {
                var report = r;
                var relink = pe.Length == 0;
                var (image, symbols) = relink ? (m.Pe, m.Pdb) : (pe, pdb);
                if (image.Length == 0) throw new InvalidOperationException($"module `{m.Name}` has no image to relink");
                var alc = new ModuleLoadContext($"{m.Name}#{generation}", Closure(m).Where(view.ContainsKey).ToDictionary(n => n, n => view[n]));
                Interlocked.Increment(ref _created);
                staged.Add((m, alc, null!, null!, relink, image, symbols));
                var t0 = Host.Now();
                var asm = symbols.Length > 0 ? alc.LoadFromStream(new MemoryStream(image), new MemoryStream(symbols)) : alc.LoadFromStream(new MemoryStream(image));
                if (relink)
                    foreach (var (md, il, dpdb) in m.Chain) MetadataUpdater.ApplyUpdate(asm, md, il, dpdb);
                var t1 = Host.Now();
                var attr = asm.GetCustomAttribute<AraModuleAttribute>()
                    ?? throw new InvalidOperationException("the assembly has no [AraModule]; is Ara.Generators referenced?");
                var register = attr.Entry.GetMethod("Register", [typeof(ModuleBuilder)])?.CreateDelegate<Action<ModuleBuilder>>()
                    ?? throw new InvalidOperationException($"{attr.Entry} has no Register(ModuleBuilder)");
                Prepare(asm, attr.Entry);
                var t2 = Host.Now();
                var builder = new ModuleBuilder();
                register(builder);
                report.TStaged = Host.Now();
                report.LoadUs = Micros(t1 - t0);
                report.PrejitUs = Micros(t2 - t1);
                report.RegisterUs = Micros(report.TStaged - t2);
                report.Bytes = (ulong)(relink ? 0 : image.Length + symbols.Length);
                view[m.Name] = asm;
                var swap = new ModuleSwap
                {
                    Slot = m.Slot,
                    Name = m.Name,
                    Module = builder,
                    Hot = hot,
                    Report = report,
                    Installed = () => Install(m, alc, asm, generation),
                    Rejected = _ => Reject(m, alc),
                };
                staged[^1] = (m, alc, asm, swap, relink, image, symbols);
                Host.Log(0, $"module `{m.Name}` generation {generation} staged{(relink ? " (relink)" : "")}: load {(t1 - t0) / 1e6:F1} ms, pre-JIT {(t2 - t1) / 1e6:F1} ms, register {(report.TStaged - t2) / 1e6:F1} ms ({image.Length / 1024} KB PE)");
            }
        }
        catch
        {
            lock (_lock)
                foreach (var s in staged)
                {
                    s.Alc.Unload();
                    _retired.Add(new Retired(new WeakReference(s.Alc), s.M.Slot, s.M.Name, GC.CollectionCount(2)) { Reported = true });
                }
            foreach (var (m, _, _, _, _, _) in batch)
            {
                m.Failed |= !m.Ready.IsSet;
                m.Ready.Set();
            }
            throw;
        }
        lock (_lock)
            foreach (var s in staged)
            {
                s.M.Staged = s.Asm;
                if (!s.Relink)
                {
                    (s.M.Pe, s.M.Pdb) = (s.Pe, s.Pdb);
                    s.M.Chain.Clear();
                }
            }
        ScriptRuntime.Enqueue(new SwapUpdate { Modules = [.. staged.Select(s => s.Swap)] });
        foreach (var s in staged) s.M.Ready.Set();
        CheckLeaks();
    }

    /// <summary>The names of every module <paramref name="m"/> references, directly or through another module.</summary>
    HashSet<string> Closure(Module m)
    {
        var names = new HashSet<string>(StringComparer.Ordinal);
        var stack = new Stack<string>(m.References);
        while (stack.TryPop(out var n))
            if (names.Add(n) && Find(n) is { } dep)
                foreach (var r in dep.References) stack.Push(r);
        return names;
    }

    static uint Micros(ulong ns) => (uint)Math.Min(uint.MaxValue, ns / 1000);

    /// <summary>Tick thread: the new generation is live; unload the previous one.</summary>
    void Install(Module m, ModuleLoadContext alc, Assembly asm, uint generation)
    {
        lock (_lock)
        {
            var old = m.Alc;
            (m.Alc, m.Asm, m.Generation) = (alc, asm, generation);
            if (old is not null)
            {
                old.Unload();
                _retired.Add(new Retired(new WeakReference(old), m.Slot, m.Name, GC.CollectionCount(2)));
            }
        }
    }

    /// <summary>Tick thread: registration refused the generation; modules staged later keep their view of it.</summary>
    void Reject(Module m, ModuleLoadContext alc)
    {
        lock (_lock)
        {
            alc.Unload();
            _retired.Add(new Retired(new WeakReference(alc), m.Slot, m.Name, GC.CollectionCount(2)) { Reported = true });
            m.Staged = m.Asm;
        }
    }

    /// <summary>
    /// A retired generation still alive after two full collections is pinned (a static
    /// subscription, a cache holding its types): report it once, with an L4 restart request.
    /// Only natural collections are counted; nothing is forced on the game's behalf.
    /// </summary>
    void CheckLeaks()
    {
        var gen2 = GC.CollectionCount(2);
        List<Retired> leaked;
        lock (_lock)
        {
            leaked = _retired.Where(r => !r.Reported && gen2 >= r.Gen2 + 2 && r.Alc.IsAlive).ToList();
            foreach (var r in leaked) r.Reported = true;
            _retired.RemoveAll(r => r.Reported && !r.Alc.IsAlive);
        }
        foreach (var r in leaked)
        {
            Diag.Report(AraCodes.GenerationLeaked, "warning",
                $"module `{r.Name}`: a retired generation is still alive after two full collections; something pins its types (static events, caches). Restart the play process (L4) to reclaim it");
            ScriptRuntime.Enqueue(new ErrorUpdate { Slot = r.Slot, Report = new ReloadReport { Kind = AbiConst.ReloadL4, TReceived = Host.Now(), TStaged = Host.Now() } });
        }
    }

    /// <summary>
    /// JITs the generated entry points, layouts and registration lambdas in parallel, so
    /// neither <c>Register</c> nor the first tick on new code pays for them.
    /// </summary>
    static void Prepare(Assembly asm, Type entry)
    {
        const BindingFlags all = BindingFlags.Static | BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        var generated = asm.GetTypes().SelectMany(t => t.GetMethods(all))
            .Where(m => m.Name.StartsWith("__Ara", StringComparison.Ordinal));
        var lambdas = entry.GetNestedTypes(BindingFlags.NonPublic).SelectMany(t => t.GetMethods(all)).Where(m => !m.IsAbstract && !m.ContainsGenericParameters);
        var entries = generated.Concat(lambdas).ToArray();
        // The JIT is thread-safe: spread the entry points over a few pool threads (the
        // boosted tick threads keep priority).
        Parallel.ForEach(entries, new ParallelOptions { MaxDegreeOfParallelism = Math.Max(1, System.Environment.ProcessorCount / 4) },
            m => RuntimeHelpers.PrepareMethod(m.MethodHandle));
    }

    /// <summary>A <see cref="MessageKind.Reload"/>: stage every module of it as one swap.</summary>
    internal void OnReload(Protocol.Reader r)
    {
        var transfer = r.Since();
        var received = Host.Now();
        var count = r.U32();
        var batch = new List<(Module, byte[], byte[], uint, bool, ReloadReport)>();
        for (int i = 0; i < count; i++)
        {
            var (slot, generation, hot, kind, escalation, origin, t, pe, pdb) =
                (r.U32(), r.U32(), r.Bool(), r.U32(), r.U32(), r.U32(), r.Timing(), r.Bytes(), r.Bytes());
            if (Find(slot) is not { } m) return;
            batch.Add((m, pe, pdb, generation, hot, Report(kind, escalation, origin, t, transfer, received)));
        }
        try { Stage(batch); }
        catch (Exception e)
        {
            var names = string.Join(", ", batch.Select(b => $"`{b.Item1.Name}`"));
            Diag.Report(AraCodes.LoadFailed, "error", $"module {names}: {e.Message} (the last good code keeps running)");
        }
    }

    static ReloadReport Report(uint kind, uint escalation, uint origin, CompileTiming t, uint transfer, ulong received) => new()
    {
        Kind = kind,
        Escalation = escalation,
        Origin = origin,
        ServerUs = t.ServerUs,
        DetectUs = t.DetectUs,
        ParseUs = t.ParseUs,
        GenerateUs = t.GenerateUs,
        EmitUs = t.EmitUs,
        AnalyzeUs = t.AnalyzeUs,
        TransferUs = transfer,
        TReceived = received,
    };

    /// <summary>A <see cref="MessageKind.Delta"/>: queue it for the tick boundary.</summary>
    internal void OnDelta(Protocol.Reader r)
    {
        var transfer = r.Since();
        var received = Host.Now();
        var (slot, generation, kind, changed, origin, t, md, il, pdb) =
            (r.U32(), r.U32(), r.U32(), r.U32(), r.U32(), r.Timing(), r.Bytes(), r.Bytes(), r.Bytes());
        if (Find(slot) is not { } m) return;
        m.Chain.Add((md, il, pdb));
        if (Debugger.IsAttached)
        {
            // The runtime refuses ApplyUpdate with a debugger attached (R10 finding 1.2): take L3.
            _client?.Rebuild(slot, AbiConst.EscDebugger);
            return;
        }
        var report = Report(kind, 0, origin, t, transfer, received);
        report.ChangedMembers = changed;
        report.Bytes = (ulong)(md.Length + il.Length + pdb.Length);
        report.TStaged = received;
        ScriptRuntime.Enqueue(new DeltaUpdate
        {
            Slot = slot,
            Report = report,
            Apply = () =>
            {
                Assembly asm;
                lock (_lock)
                {
                    if (m.Asm is null || m.Generation != generation)
                        throw new InvalidOperationException($"delta for generation {generation}, but {m.Generation} is running");
                    asm = m.Asm;
                }
                MetadataUpdater.ApplyUpdate(asm, md, il, pdb);
            },
            Failed = _ => _client?.Rebuild(slot, AbiConst.EscRuntimeRejected),
        });
    }

    /// <summary>A <see cref="MessageKind.Diagnostics"/>: report them; errors record a failed reload.</summary>
    internal void OnDiagnostics(Protocol.Reader r)
    {
        var (slot, errors, json, serverUs, origin) = (r.U32(), r.U32(), r.Str(), r.U64(), r.U32());
        using (var doc = JsonDocument.Parse(json))
            foreach (var d in doc.RootElement.EnumerateArray())
                Host.Diagnostic(System.Text.Encoding.UTF8.GetBytes(d.GetRawText()));
        if (errors == 0 || Find(slot) is not { } m) return;
        if (!m.Ready.IsSet)
        {
            m.Failed = true;
            m.Ready.Set();
            return;
        }
        ScriptRuntime.Enqueue(new ErrorUpdate
        {
            Slot = slot,
            Report = new ReloadReport { Kind = AbiConst.ReloadError, ErrorCount = errors, ServerUs = serverUs, Origin = origin, TReceived = Host.Now(), TStaged = Host.Now() },
        });
    }

    /// <summary>A <see cref="MessageKind.Restart"/> (L4): report it; every affected module records an L4 report.</summary>
    internal void OnRestart(Protocol.Reader r)
    {
        var (slot, reason) = (r.U32(), r.Str());
        Diag.Report(AraCodes.RestartRequired, "error", reason);
        List<Module> affected;
        lock (_lock) affected = _modules.Where(m => slot == uint.MaxValue || m.Slot == slot).ToList();
        foreach (var m in affected)
            ScriptRuntime.Enqueue(new ErrorUpdate { Slot = m.Slot, Report = new ReloadReport { Kind = AbiConst.ReloadL4, TReceived = Host.Now(), TStaged = Host.Now() } });
    }

    /// <inheritdoc/>
    public int SetHeat(uint slot, bool hot)
    {
        if (_client is null || Find(slot) is null) return -1;
        _client.SetHeat(slot, hot);
        return 0;
    }

    /// <inheritdoc/>
    public int RequestCompile(string json)
    {
        if (_client is null) return -1;
        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;
        if (Find(root.GetProperty("module").GetString() ?? "") is not { } m) return -2;
        var paths = root.GetProperty("paths").EnumerateArray().Select(p => p.GetString() ?? "").Where(p => p.Length > 0).ToList();
        _client.Change(m.Slot, paths, root.GetProperty("origin").GetUInt32());
        return 0;
    }

    /// <inheritdoc/>
    public int Shutdown()
    {
        _client?.Dispose();
        _client = null;
        lock (_lock)
        {
            foreach (var m in _modules)
            {
                if (m.Alc is { } alc)
                {
                    alc.Unload();
                    _retired.Add(new Retired(new WeakReference(alc), m.Slot, m.Name, 0));
                }
                (m.Alc, m.Asm, m.Staged) = (null, null, null);
            }
            _modules.Clear();
        }
        // Unload can lag behind finalizers and JIT bookkeeping; retry briefly (S2).
        for (int i = 0; i < 40 && AlcsAlive > 0; i++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
            if (AlcsAlive > 0) Thread.Sleep(50);
        }
        return AlcsAlive;
    }
}

using System.Diagnostics;
using System.Collections.Concurrent;
using System.Text.Json;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using Ara.Hosting;

namespace Ara.CompileServer;

/// <summary>
/// The compile server's scheduler: modules and their reference graph, the work queues and
/// the one worker thread that compiles (decision A13, D20).
/// </summary>
/// <remarks>
/// <list type="bullet">
/// <item>Human work (file-watcher saves, heat changes, rebuilds) runs first, in order, with
/// the process at normal priority. Queued saves of one module merge.</item>
/// <item>Agent edits (<see cref="Change"/> with <c>OriginAgent</c>, and watcher events on paths
/// an agent claimed) are coalesced per module until it has been quiet for
/// <see cref="AgentQuiet"/>, run only when no human work is queued, at low priority, and are
/// cancelled and requeued when a human edit arrives. A human save of the same module
/// absorbs them.</item>
/// <item>An image (L3) of a module cascades to every module that references it, in
/// dependency order: each relinks (same image, new ALC) or rebuilds against the new code.
/// The whole cascade is sent as one <see cref="MessageKind.Reload"/> or, if any module fails
/// to compile, nothing is applied.</item>
/// </list>
/// </remarks>
internal sealed class Coordinator : ICompileContext, IDisposable
{
    /// <summary>Agent edits to a module are coalesced until it has been quiet this long.</summary>
    internal TimeSpan AgentQuiet = TimeSpan.FromMilliseconds(150);
    /// <summary>How long watcher events on a path an agent claimed count as agent edits.</summary>
    static readonly long ClaimTicks = 2 * Stopwatch.Frequency;

    enum WorkKind { Initial, Change, Heat, Rebuild }

    sealed class Work(WorkKind kind, uint slot, uint origin, long tEvent)
    {
        internal readonly WorkKind Kind = kind;
        internal readonly uint Slot = slot;
        internal readonly uint Origin = origin;
        internal long TEvent = tEvent;
        internal long LastRequest = Stopwatch.GetTimestamp();
        internal readonly HashSet<string> Paths = new(ModuleState.Paths);
        internal bool Hot;
        internal Escalation Escalation;
    }

    readonly IMessageSink _out;
    readonly IAnalysis? _analysis;
    readonly object _lock = new();
    readonly List<ModuleSession> _modules = []; // dependency order (the host watches them that way)
    readonly Dictionary<uint, ModuleSession> _bySlot = [];
    readonly Dictionary<string, ModuleSession> _byName = new(StringComparer.Ordinal);
    readonly LinkedList<Work> _human = new();
    readonly Dictionary<uint, Work> _agent = [];
    readonly Dictionary<string, long> _claims = new(ModuleState.Paths);
    readonly Dictionary<string, Guid> _references = new(ModuleState.Paths);
    readonly List<FileSystemWatcher> _referenceWatchers = [];
    readonly AutoResetEvent _wake = new(false);
    readonly CancellationTokenSource _stop = new();
    readonly Thread _worker;
    readonly ModuleWatcher _watcher;
    CancellationTokenSource? _agentRun;
    Dictionary<ModuleSession, Outcome>? _cascade;
    bool _busy;
    bool _compiling;
    bool _restartSent;
    readonly CompileProducer _diagnostics = new("warm");
    readonly ConcurrentDictionary<uint, CompileAttempt> _attempts = new();
    readonly ConcurrentDictionary<(string Module, string Profile), byte> _bound = new();

    /// <inheritdoc/>
    public bool Enc { get; private set; }

    /// <inheritdoc/>
    public EncCapabilities Caps { get; private set; } = EncCapabilities.BodiesOnly;

    internal Coordinator(IMessageSink output, IAnalysis? analysis)
    {
        _out = output;
        _analysis = analysis;
        _watcher = new ModuleWatcher(OnWatcher);
        _worker = new Thread(Loop) { Name = "ara-compile", IsBackground = true };
        _worker.Start();
    }

    /// <summary>The host's EnC support and capabilities (an empty list allows L1 only).</summary>
    internal void Hello(bool enc, string capabilities)
    {
        Enc = enc;
        Caps = enc && capabilities.Length > 0 ? EncCapabilities.Parse(capabilities) : EncCapabilities.BodiesOnly;
    }

    /// <summary>
    /// Adds a module (after the modules it references), builds its first image and watches its
    /// folder. A reference to a module that is not watched yet is refused with a diagnostic.
    /// </summary>
    internal void Watch(uint slot, string name, string dir, bool hot, IReadOnlyList<string> assemblies,
        IReadOnlyList<string> references, string profile, bool watch)
    {
        lock (_lock)
        {
            var missing = references.Where(r => !_byName.ContainsKey(r)).ToList();
            if (missing.Count > 0)
            {
                var attempt = Begin(slot, name, profile, track: false);
                SendDiagnostics(slot, dir, [], [(AraCodes.UnknownModule, "error",
                    $"module `{name}` references {string.Join(", ", missing.Select(m => $"`{m}`"))}, which the compile server does not have; modules load after the modules they reference, and references cannot form a cycle")], 0, AbiConst.OriginHuman, supplied: attempt);
                return;
            }
            var m = new ModuleSession(slot, name, dir, hot, assemblies, references, profile, _analysis);
            _modules.Add(m);
            _bySlot[slot] = m;
            _byName[name] = m;
            if (watch) _watcher.Watch(slot, m.WatchDirectory);
            foreach (var a in assemblies) WatchReference(a);
            _human.AddLast(new Work(WorkKind.Initial, slot, AbiConst.OriginHuman, 0));
        }
        _wake.Set();
    }

    /// <summary>
    /// Files of a module changed (or are about to). Agent changes also claim their paths, so
    /// the watcher's events for them count as agent edits.
    /// </summary>
    internal void Change(uint slot, IReadOnlyCollection<string> paths, uint origin, long tEvent)
    {
        if (origin == AbiConst.OriginAgent)
        {
            var until = Stopwatch.GetTimestamp() + ClaimTicks;
            lock (_lock)
                foreach (var p in paths) _claims[ProjectInventory.ProjectPath(p)] = until;
        }
        var w = new Work(WorkKind.Change, slot, origin, tEvent);
        w.Paths.UnionWith(paths.Select(ProjectInventory.ProjectPath));
        Enqueue(w);
    }

    void OnWatcher(uint slot, IReadOnlyCollection<string> paths, long tEvent)
    {
        uint origin;
        lock (_lock)
        {
            var now = Stopwatch.GetTimestamp();
            origin = paths.Any(p => _claims.TryGetValue(p, out var until) && until > now) ? AbiConst.OriginAgent : AbiConst.OriginHuman;
        }
        var w = new Work(WorkKind.Change, slot, origin, tEvent);
        w.Paths.UnionWith(paths);
        Enqueue(w);
    }

    /// <summary>Recompiles a module hot (Debug IL, EnC) or cold (optimized), at L3 cost.</summary>
    internal void SetHeat(uint slot, bool hot) => Enqueue(new Work(WorkKind.Heat, slot, AbiConst.OriginHuman, 0) { Hot = hot });

    /// <summary>Sends a full image of a module's committed state (a delta could not be applied).</summary>
    internal void Rebuild(uint slot, Escalation escalation) =>
        Enqueue(new Work(WorkKind.Rebuild, slot, AbiConst.OriginHuman, 0) { Escalation = escalation });

    void Enqueue(Work w)
    {
        lock (_lock)
        {
            if (w.Kind == WorkKind.Change && w.Origin == AbiConst.OriginAgent)
            {
                var queuedHuman = _human.FirstOrDefault(h => h.Kind == WorkKind.Change && h.Slot == w.Slot);
                if (queuedHuman is not null) queuedHuman.Paths.UnionWith(w.Paths); // a queued human save absorbs it
                else if (_agent.TryGetValue(w.Slot, out var a))
                {
                    a.Paths.UnionWith(w.Paths);
                    a.LastRequest = Stopwatch.GetTimestamp();
                }
                else _agent[w.Slot] = w;
            }
            else
            {
                var same = w.Kind == WorkKind.Change ? _human.FirstOrDefault(h => h.Kind == WorkKind.Change && h.Slot == w.Slot) : null;
                if (same is not null)
                {
                    same.Paths.UnionWith(w.Paths);
                    same.TEvent = Math.Min(same.TEvent, w.TEvent);
                }
                else _human.AddLast(w);
                var target = same ?? w;
                if (w.Kind == WorkKind.Change && _agent.Remove(w.Slot, out var pending)) target.Paths.UnionWith(pending.Paths);
                _agentRun?.Cancel(); // human work preempts a running agent compile
            }
        }
        _wake.Set();
    }

    /// <summary>Blocks until no work is queued or running (tests, tools).</summary>
    internal bool WaitIdle(TimeSpan timeout)
    {
        var sw = Stopwatch.StartNew();
        while (sw.Elapsed < timeout)
        {
            lock (_lock)
                if (_human.Count == 0 && _agent.Count == 0 && !_busy) return true;
            Thread.Sleep(5);
        }
        return false;
    }

    void Loop()
    {
        var quiet = (long)(AgentQuiet.TotalSeconds * Stopwatch.Frequency);
        while (!_stop.IsCancellationRequested)
        {
            Work? w = null;
            var agent = false;
            var ct = _stop.Token;
            var wait = Timeout.Infinite;
            lock (_lock)
            {
                quiet = (long)(AgentQuiet.TotalSeconds * Stopwatch.Frequency);
                if (_human.First is { } first)
                {
                    w = first.Value;
                    _human.RemoveFirst();
                }
                else if (_agent.Count > 0)
                {
                    var now = Stopwatch.GetTimestamp();
                    var ready = _agent.Values.Where(a => now - a.LastRequest >= quiet).MinBy(a => a.TEvent);
                    if (ready is not null)
                    {
                        _agent.Remove(ready.Slot);
                        w = ready;
                        agent = true;
                        _agentRun = CancellationTokenSource.CreateLinkedTokenSource(_stop.Token);
                        ct = _agentRun.Token;
                    }
                    else
                    {
                        var next = _agent.Values.Min(a => a.LastRequest + quiet) - now;
                        wait = (int)Math.Clamp(next * 1000 / Stopwatch.Frequency + 1, 1, 1000);
                    }
                }
                _busy = w is not null;
            }
            if (w is null)
            {
                _wake.WaitOne(wait);
                continue;
            }
            Priority(agent);
            try
            {
                Run(w, ct);
            }
            catch (OperationCanceledException) when (agent && !_stop.IsCancellationRequested)
            {
                CancelAttempts();
                Log(0, $"agent compile of module `{_bySlot[w.Slot].Name}` yielded to a human edit; requeued");
                Enqueue(w);
            }
            catch (Exception e) when (e is not OperationCanceledException)
            {
                if (_bySlot.TryGetValue(w.Slot, out var failed))
                    SendDiagnostics(failed.Slot, failed.Dir, [], [(AraCodes.CompileServer, "error", e.Message)], 0, w.Origin, validated: false);
                Log(2, $"module {w.Slot}: {e}");
            }
            finally
            {
                CancelAttempts();
                lock (_lock)
                {
                    _busy = false;
                    _agentRun = null;
                }
                CompileState(false);
                Priority(null);
            }
        }
    }

    static readonly Process Self = Process.GetCurrentProcess();

    /// <summary>Human compiles at normal priority; agent compiles and idle time below normal (D20, A13).</summary>
    static void Priority(bool? agent)
    {
        try
        {
            var cls = agent == false ? ProcessPriorityClass.Normal : ProcessPriorityClass.BelowNormal;
            if (Self.PriorityClass != cls) Self.PriorityClass = cls;
            Thread.CurrentThread.Priority = agent == true ? ThreadPriority.BelowNormal : ThreadPriority.Normal;
        }
        catch (Exception) { /* not allowed on this platform: priorities are a latency aid only */ }
    }

    void Run(Work w, CancellationToken ct)
    {
        if (!_bySlot.TryGetValue(w.Slot, out var m)) return;
        if (w.Kind == WorkKind.Heat && m.State is { } current && current.Hot == (w.Hot && Enc)) return;
        Begin(m.Slot, m.Name, m.Profile);
        CompileState(true);
        switch (w.Kind)
        {
            case WorkKind.Initial:
                var unbuilt = m.References.Where(r => _byName[r].State is null).ToList();
                if (unbuilt.Count > 0)
                {
                    SendDiagnostics(m.Slot, m.Dir, [], [(AraCodes.DependencyFailed, "error",
                        $"module `{m.Name}` is not built because {string.Join(", ", unbuilt.Select(u => $"`{u}`"))} did not build")], 0, w.Origin);
                    return;
                }
                Handle(Evaluated(m.Initial(this, ct), ct), w.Origin, ct);
                m.Prewarm(this);
                break;
            case WorkKind.Change:
                Handle(Evaluated(m.Compile(w.Paths, w.TEvent, this, ct), ct), w.Origin, ct);
                break;
            case WorkKind.Heat:
                if (m.State is { } s && s.Hot == (w.Hot && Enc)) return;
                Handle(Evaluated(m.Full(Escalation.Requested, w.Hot, this, ct), ct), w.Origin, ct);
                break;
            case WorkKind.Rebuild:
                Handle(Evaluated(m.Full(w.Escalation, null, this, ct), ct), w.Origin, ct);
                break;
        }
    }

    static Outcome Evaluated(Outcome outcome, CancellationToken cancellation)
    {
        cancellation.ThrowIfCancellationRequested();
        return outcome;
    }

    void Handle(Outcome o, uint origin, CancellationToken cancellation)
    {
        var m = o.Module;
        switch (o.Kind)
        {
            case OutcomeKind.None:
                if (o.Next is not null) { m.Commit(o); RebindDependents(m); }
                SendDiagnostics(m.Slot, m.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
                return;
            case OutcomeKind.Errors:
                SendDiagnostics(m.Slot, m.Dir, o.Problems, o.Extra, o.Timing.ToTiming().ServerUs, origin, o.ValidatedInputs);
                return;
            case OutcomeKind.Restart:
                _out.Send(MessageKind.Restart, new Protocol.Writer().U32(m.Slot).Str("module/source domain boundary changed; restart Play from the current snapshot to load the new inventory (L4)").ToArray());
                return;
            case OutcomeKind.Delta:
                m.Commit(o);
                RebindDependents(m);
                SendDiagnostics(m.Slot, m.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
                SendDelta(o, origin);
                return;
            case OutcomeKind.Image:
                Cascade(o, origin, cancellation);
                return;
        }
    }

    void RebindDependents(ModuleSession m)
    {
        foreach (var d in _modules.Where(x => x.References.Contains(m.Name))) d.Rebind(m.Name, m.State!);
    }

    /// <summary>Every module that references <paramref name="root"/>, directly or not, in dependency order.</summary>
    List<ModuleSession> Dependents(ModuleSession root)
    {
        var names = new HashSet<string>(StringComparer.Ordinal) { root.Name };
        var list = new List<ModuleSession>();
        foreach (var m in _modules)
        {
            if (m == root || !m.References.Any(names.Contains)) continue;
            names.Add(m.Name);
            if (m.State is not null) list.Add(m);
        }
        return list;
    }

    /// <summary>
    /// Relinks or rebuilds every dependent against the new image; commits and sends all of
    /// them as one reload, or nothing if any dependent fails to compile.
    /// </summary>
    void Cascade(Outcome root, uint origin, CancellationToken cancellation)
    {
        var order = Dependents(root.Module);
        _cascade = new Dictionary<ModuleSession, Outcome> { [root.Module] = root };
        try
        {
            var outcomes = new List<Outcome> { root };
            foreach (var d in order)
            {
                Begin(d.Slot, d.Name, d.Profile);
                var o = d.Cascade(this, cancellation);
                if (o.Kind == OutcomeKind.Errors)
                {
                    o.Extra.Add((AraCodes.DependencyFailed, "error",
                        $"module `{d.Name}` does not compile against the change to `{root.Module.Name}`; both keep their last good version"));
                    SendDiagnostics(d.Slot, d.Dir, o.Problems, o.Extra, 0, origin);
                    SendDiagnostics(root.Module.Slot, root.Module.Dir, root.Problems.Where(p => p.Severity != Microsoft.CodeAnalysis.DiagnosticSeverity.Error).ToList(),
                        [(AraCodes.DependencyFailed, "error", $"the change to `{root.Module.Name}` breaks module `{d.Name}` (see its diagnostics); nothing was applied")],
                        root.Timing.ToTiming().ServerUs, origin);
                    return;
                }
                _cascade[d] = o;
                outcomes.Add(o);
            }
            cancellation.ThrowIfCancellationRequested();
            foreach (var o in outcomes) o.Module.Commit(o);
            foreach (var o in outcomes) SendDiagnostics(o.Module.Slot, o.Module.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
            SendReload(outcomes, origin);
        }
        finally
        {
            _cascade = null;
        }
    }

    /// <inheritdoc/>
    public ModuleState? Dependency(string name)
    {
        if (!_byName.TryGetValue(name, out var m)) return null;
        return _cascade is not null && _cascade.TryGetValue(m, out var o) && o.Next is { } prepared ? prepared : m.State;
    }

    /// <inheritdoc/>
    public bool HasDependents(ModuleSession module) => _modules.Any(m => m.References.Contains(module.Name));

    void SendReload(List<Outcome> outcomes, uint origin)
    {
        var w = new Protocol.Writer().Now().U32((uint)outcomes.Count);
        foreach (var o in outcomes)
        {
            var s = o.Next!;
            w.U32(o.Module.Slot).U32(s.Generation).Bool(s.Hot).U32(o.ReloadKind).U32((uint)o.Escalation).U32(origin)
                .Timing(o.Timing.ToTiming()).Bytes(o.Image?.Pe ?? []).Bytes(o.Image?.Pdb ?? []);
        }
        _out.Send(MessageKind.Reload, w.ToArray());
        foreach (var o in outcomes)
            Log(0, $"module `{o.Module.Name}` generation {o.Next!.Generation} ({Kind(o)}, {(Escalation)o.Escalation}): {o.Timing}" +
                (o.Image is { } i ? $", {(i.Pe.Length + i.Pdb.Length) / 1024} KB" : ""));
    }

    void SendDelta(Outcome o, uint origin)
    {
        var d = o.Delta!;
        _out.Send(MessageKind.Delta, new Protocol.Writer().Now()
            .U32(o.Module.Slot).U32(o.Next!.Generation).U32(o.ReloadKind).U32((uint)o.Changed).U32(origin)
            .Timing(o.Timing.ToTiming()).Bytes(d.Metadata).Bytes(d.Il).Bytes(d.Pdb).ToArray());
        Log(0, $"module `{o.Module.Name}` {Kind(o)} ({o.Changed} members): {o.Timing}");
    }

    static string Kind(Outcome o) => o.ReloadKind switch
    {
        AbiConst.ReloadInitial => "initial",
        AbiConst.ReloadL1 => "L1",
        AbiConst.ReloadL2 => "L2",
        _ => o.Kind == OutcomeKind.Relink ? "relink" : "L3",
    };

    void SendDiagnostics(uint slot, string dir, List<Microsoft.CodeAnalysis.Diagnostic> problems,
        List<(string Code, string Severity, string Message)> extra, ulong serverUs, uint origin, bool validated = true, CompileAttempt? supplied = null)
    {
        var attempt = supplied;
        if (attempt is null) _attempts.TryRemove(slot, out attempt);
        if (problems.Count == 0 && extra.Count == 0 && attempt is null) return;
        var errors = (uint)(problems.Count(d => d.Severity == Microsoft.CodeAnalysis.DiagnosticSeverity.Error) + extra.Count(e => e.Severity == "error"));
        var rows = problems.Select(Diagnostics.Row).Concat(extra.Select(e => Diagnostics.Row(e.Code, e.Severity, e.Message, dir))).ToArray();
        var result = errors > 0 ? "errors" : validated ? "success" : "cancelled";
        var json = JsonSerializer.Serialize(attempt?.Complete(rows, result, validated) ?? rows);
        _out.Send(MessageKind.Diagnostics, new Protocol.Writer().U32(slot).U32(errors).Str(json).U64(serverUs).U32(origin).ToArray());
    }

    CompileAttempt Begin(uint slot, string module, string profile, bool track = true)
    {
        var attempt = _diagnostics.Next(module, profile);
        if (track) _attempts[slot] = attempt;
        var rows = new List<System.Text.Json.Nodes.JsonObject>();
        if (_bound.TryAdd((module, profile), 0)) rows.Add((attempt with { Request = attempt.Request with { Attempt = 0 } }).Marker("bound"));
        rows.Add(attempt.Marker("started"));
        _out.Send(MessageKind.Diagnostics, new Protocol.Writer().U32(slot).U32(0).Str(JsonSerializer.Serialize(rows)).U64(0).U32(0).ToArray());
        return attempt;
    }

    void CancelAttempts()
    {
        foreach (var (slot, attempt) in _attempts)
            if (_attempts.TryRemove(slot, out _))
                _out.Send(MessageKind.Diagnostics, new Protocol.Writer().U32(slot).U32(0)
                    .Str(JsonSerializer.Serialize(new[] { attempt.Marker("completed", "cancelled") })).U64(0).U32(0).ToArray());
    }

    void CompileState(bool active)
    {
        if (_compiling == active) return;
        _compiling = active;
        _out.Send(MessageKind.CompileState, new Protocol.Writer().Bool(active).ToArray());
    }

    void Log(uint level, string text) => _out.Send(MessageKind.Log, new Protocol.Writer().U32(level).Str(text).ToArray());

    /// <summary>
    /// Watches an engine assembly every module compiles against (Ara.Runtime). It lives in the
    /// host's default context, which cannot unload: when it changes, nothing can reload in this
    /// process (L4) until the play process restarts.
    /// </summary>
    void WatchReference(string path)
    {
        path = Path.GetFullPath(path);
        if (_references.ContainsKey(path) || !File.Exists(path)) return;
        _references[path] = Mvid(path) ?? Guid.Empty;
        var w = new FileSystemWatcher(Path.GetDirectoryName(path)!, Path.GetFileName(path))
        {
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
        };
        FileSystemEventHandler on = (_, _) => ThreadPool.QueueUserWorkItem(_ => CheckReference(path));
        w.Changed += on;
        w.Created += on;
        w.EnableRaisingEvents = true;
        _referenceWatchers.Add(w);
    }

    void CheckReference(string path)
    {
        Guid? now = null;
        for (int i = 0; i < 20 && now is null; i++)
        {
            now = Mvid(path);
            if (now is null) Thread.Sleep(25); // still being written
        }
        lock (_lock)
        {
            if (now is null || now == _references[path] || _restartSent) return;
            _restartSent = true;
        }
        var reason = $"`{Path.GetFileName(path)}` changed on disk (engine bindings); it is loaded in the host's default context, which cannot reload. Restart the play process (L4); the last good code keeps running until then";
        Log(1, reason);
        _out.Send(MessageKind.Restart, new Protocol.Writer().U32(uint.MaxValue).Str(reason).ToArray());
    }

    static Guid? Mvid(string path)
    {
        try
        {
            using var pe = new PEReader(File.OpenRead(path));
            var md = pe.GetMetadataReader();
            return md.GetGuid(md.GetModuleDefinition().Mvid);
        }
        catch (Exception e) when (e is IOException or BadImageFormatException or InvalidOperationException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    /// <summary>Stops the worker and every watcher.</summary>
    public void Dispose()
    {
        _stop.Cancel();
        _wake.Set();
        _watcher.Dispose();
        foreach (var w in _referenceWatchers) w.Dispose();
    }
}

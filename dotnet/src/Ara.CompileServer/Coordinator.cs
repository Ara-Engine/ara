using System.Diagnostics;
using System.Collections.Concurrent;
using System.Text.Json;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

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
    readonly Func<long> _clock;

    enum WorkKind { Initial, Change, Heat, Rebuild, Recover, PublishedRebind, Verify }

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
        internal ulong Recovery;
    }

    readonly IMessageSink _out;
    readonly IAnalysis? _analysis;
    readonly object _lock = new();
    readonly List<ModuleSession> _modules = []; // dependency order (the host watches them that way)
    readonly Dictionary<uint, ModuleSession> _bySlot = [];
    readonly Dictionary<string, ModuleSession> _byName = new(StringComparer.Ordinal);
    readonly LinkedList<Work> _human = new();
    readonly Dictionary<uint, Work> _agent = [];
    readonly SourceClaims _claims = new();
    readonly Dictionary<string, Guid> _references = new(ModuleState.Paths);
    readonly List<FileSystemWatcher> _referenceWatchers = [];
    readonly List<Task> _watchStarts = [];
    readonly HashSet<string> _referenceWatched = new(ModuleState.Paths);
    readonly HashSet<string> _referenceStarting = new(ModuleState.Paths);
    readonly Dictionary<uint, WatchProgress> _watchProgress = [];

    /// <summary>What a module's watcher start has already done, so a retry resumes from the failed step instead of repeating it.</summary>
    sealed class WatchProgress
    {
        internal bool Module;
        internal readonly HashSet<string> References = new(ModuleState.Paths);
        internal bool Verified;
    }

    /// <summary>How the folder and reference watchers are created; tests replace these to make a start fail.</summary>
    internal Func<string, string, FileSystemWatcher> ReferenceWatcherFactory = (directory, file) => new FileSystemWatcher(directory, file);
    internal Action<ModuleWatcher, uint, string> ModuleWatchStart = (watcher, slot, directory) => watcher.Watch(slot, directory);

    /// <summary>Blocks until every watcher start requested so far has finished (tests).</summary>
    internal void WaitForWatchStarts()
    {
        Task[] starts;
        lock (_watchStarts) starts = [.. _watchStarts];
        Task.WaitAll(starts, TimeSpan.FromSeconds(30));
    }

    /// <summary>Folder watchers started so far (one per module); a retry must not add a second.</summary>
    internal int ModuleWatcherCount => _watcher.Count;

    /// <summary>Reference-assembly watchers started so far (one per distinct assembly).</summary>
    internal int ReferenceWatcherCount { get { lock (_referenceWatchers) return _referenceWatchers.Count; } }
    readonly AutoResetEvent _wake = new(false);
    readonly CancellationTokenSource _stop = new();
    readonly Thread _worker;
    readonly ModuleWatcher _watcher;
    CancellationTokenSource? _agentRun;
    Task? _agentCancellation;
    Dictionary<ModuleSession, Outcome>? _cascade;
    Dictionary<(string Project, string AuthoringRoot), ProjectInventoryResult>? _waveInventories;
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

    internal Coordinator(IMessageSink output, IAnalysis? analysis, Func<long>? clock = null)
    {
        _clock = clock ?? Stopwatch.GetTimestamp;
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
    /// Adds a module after its references, adopting captured publication facts or building the
    /// first image, then watches its folder. Unknown module references are refused.
    /// </summary>
    internal void Watch(uint slot, string name, string dir, bool hot, IReadOnlyList<string> assemblies,
        IReadOnlyList<string> references, string profile, bool watch, bool initial = true, uint generation = 0,
        PublishedSnapshot? published = null)
    {
        ModuleSession m;
        lock (_lock)
        {
            var missing = references.Where(r => !_byName.ContainsKey(r)).ToList();
            if (missing.Count > 0)
            {
                var attempt = Begin(slot, name, profile, track: false);
                SendDiagnostics(slot, dir, [], [(AraCodes.UnknownModule, "error",
                    $"module `{name}` references {string.Join(", ", missing.Select(m => $"`{m}`"))}, which the compile server does not have; modules load after the modules they reference, and references cannot form a cycle")], 0, AbiConst.OriginHuman, validated: false, supplied: attempt);
                return;
            }
            m = new ModuleSession(slot, name, dir, hot, assemblies, references, profile, _analysis) { InitialGeneration = generation };
            if (published is not null && !m.ImportPublished(published, this)) initial = true;
            _modules.Add(m);
            _bySlot[slot] = m;
            _byName[name] = m;
            // The MVID baselines are taken now, before the first compile reads the references, so a rebuild of a
            // referenced assembly after this point shows as drift (only the watchers start later).
            foreach (var a in assemblies) RecordReference(a);
            if (initial) Enqueue(new Work(WorkKind.Initial, slot, AbiConst.OriginHuman, 0));
            else if (m.PublishedReferencesDrift(this))
                Enqueue(new Work(WorkKind.PublishedRebind, slot, AbiConst.OriginHuman, Stopwatch.GetTimestamp()));
            else if (m.PublishedSourcesDrift)
                Enqueue(new Work(WorkKind.Change, slot, AbiConst.OriginHuman, Stopwatch.GetTimestamp()));
        }
        _wake.Set();
        // Watchers start after the first compile is queued and off this thread: a FileSystemWatcher takes
        // ~200 ms to start on macOS, which used to delay the first compile and the next Watch message.
        lock (_watchStarts) _watchStarts.Add(Task.Run(() => StartWatching(slot, assemblies, watch)));
    }

    /// <summary>
    /// Watches the module folder and its reference assemblies, then queues a verification behind the first compile.
    /// Whatever happened to the sources between the request and the watcher going live (a save, a delete, a file
    /// moved in) is in neither the watcher's events nor, necessarily, the first compile's reads, so the verification
    /// compares the sources' actual bytes with what the module compiled and compiles any difference as a change.
    /// It does not trust modification times, which are coarse on some file systems and unchanged by a rename.
    /// A failure to start is logged and retried once; the module is never silently left unwatched.
    /// </summary>
    internal void StartWatching(uint slot, IReadOnlyList<string> assemblies, bool watch)
    {
        string? name = null;
        WatchProgress progress;
        lock (_watchProgress) progress = _watchProgress.TryGetValue(slot, out var known) ? known : _watchProgress[slot] = new();
        for (var attempt = 1; attempt <= 2 && !_stop.IsCancellationRequested; attempt++)
        {
            try
            {
                ModuleSession m;
                lock (_lock) m = _bySlot[slot];
                name = m.Name;
                var watched = watch && !m.ReadOnlyPublished;
                // Each step is recorded once it has completed, so the retry resumes after the last good one.
                if (watched && !progress.Module)
                {
                    ModuleWatchStart(_watcher, slot, m.WatchDirectory);
                    progress.Module = true;
                }
                foreach (var a in assemblies)
                {
                    if (progress.References.Contains(a)) continue;
                    WatchReference(a);
                    progress.References.Add(a);
                }
                if (watched && !progress.Verified)
                {
                    Enqueue(new Work(WorkKind.Verify, slot, AbiConst.OriginHuman, Stopwatch.GetTimestamp()));
                    progress.Verified = true;
                }
                return;
            }
            catch (Exception error) when (!_stop.IsCancellationRequested)
            {
                Log(attempt == 1 ? 0u : 1u, $"module `{name ?? slot.ToString()}`: file watching did not start{(attempt == 1 ? ", retrying" : "; edits to it will not reload until the project reopens")}: {error.Message}");
                if (attempt == 1) Thread.Sleep(200);
            }
        }
    }

    /// <summary>Explicit recovery always validates current inputs and emits one atomic fresh-image swap.</summary>
    internal void Recover(ulong ticket) =>
        Enqueue(new Work(WorkKind.Recover, 0, AbiConst.OriginHuman, Stopwatch.GetTimestamp()) { Recovery = ticket });

    /// <summary>
    /// Compile changed bytes, or register/retire a source.replace version before/after materialization.
    /// </summary>
    internal void Change(uint slot, IReadOnlyCollection<string> paths, uint origin, long tEvent, SourceClaim? claim = null)
    {
        var canonical = paths.Select(ProjectInventory.ProjectPath).ToArray();
        if (origin == AbiConst.OriginAgent && claim is not null)
        {
            var completed = new List<string>();
            foreach (var path in canonical)
            {
                var hash = claim.Phase == "materialized" ? SourceClaims.Hash(path) : null;
                lock (_lock) if (_claims.Update(path, claim, hash)) completed.Add(path);
            }
            if (completed.Count == 0) return;
            canonical = completed.ToArray();
        }
        else if (origin == AbiConst.OriginAgent)
        {
            var current = canonical.Select(path => (Path: path, Hash: SourceClaims.Hash(path))).ToArray();
            lock (_lock)
                foreach (var (path, hash) in current) _claims.Current(path, hash);
        }
        var w = new Work(WorkKind.Change, slot, origin, tEvent);
        w.Paths.UnionWith(canonical);
        Enqueue(w);
    }

    internal void OnWatcher(uint slot, IReadOnlyCollection<string> paths, long tEvent)
    {
        lock (_lock) if (_bySlot.TryGetValue(slot, out var module) && module.ReadOnlyPublished) return;
        uint origin;
        var canonical = paths.Select(ProjectInventory.ProjectPath).ToArray();
        var current = canonical.Where(path => ModuleWatcher.IsSourceInput(path) && !Directory.Exists(path))
            .Select(path => (Path: path, Hash: SourceClaims.Hash(path))).ToArray();
        lock (_lock)
        {
            // Evaluate every path, including a human overwrite that invalidates an old echo.
            var matches = current.Select(p => _claims.Matches(p.Path, p.Hash)).ToArray();
            origin = matches.Length > 0 && matches.All(match => match) ? AbiConst.OriginAgent : AbiConst.OriginHuman;
        }
        var w = new Work(WorkKind.Change, slot, origin, tEvent);
        // Keep directory changes for module/editor-boundary validation, without
        // letting their metadata echoes change the authorship of source bytes.
        w.Paths.UnionWith(canonical);
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
            w.LastRequest = _clock();
            if (w.Kind == WorkKind.Change && w.Origin == AbiConst.OriginAgent)
            {
                var queuedHuman = _human.FirstOrDefault(h => h.Kind == WorkKind.Change && h.Slot == w.Slot);
                if (queuedHuman is not null) queuedHuman.Paths.UnionWith(w.Paths); // a queued human save absorbs it
                else if (_agent.TryGetValue(w.Slot, out var a))
                {
                    a.Paths.UnionWith(w.Paths);
                    a.LastRequest = _clock();
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
                // Request cancellation immediately, but run provider callbacks outside
                // the scheduler lock. The worker observes this task before becoming idle.
                if (_agentRun is not null) _agentCancellation ??= _agentRun.CancelAsync();
            }
        }
        _wake.Set();
    }

    /// <summary>Blocks until no work is queued or running (tests, tools).</summary>
    internal bool WaitIdle(TimeSpan timeout)
    {
        var sw = Stopwatch.StartNew();
        lock (_lock)
        {
            while (true)
            {
                if (_human.Count == 0 && _agent.Count == 0 && !_busy) return true;
                var remaining = timeout - sw.Elapsed;
                if (remaining <= TimeSpan.Zero) return false;
                Monitor.Wait(_lock, remaining);
            }
        }
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
                    var now = _clock();
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
            try
            {
                Priority(agent);
                Run(w, ct);
            }
            catch (OperationCanceledException error) when (_stop.IsCancellationRequested && PendingAnalysis.IsCancellation(error, ct))
            {
                // Closing Play cancels outstanding human and agent work alike.
                AnalysisFailure(error);
            }
            catch (OperationCanceledException error) when (agent && !_stop.IsCancellationRequested && PendingAnalysis.IsCancellation(error, ct))
            {
                AnalysisFailure(error);
                CancelAttempts();
                Log(0, $"agent compile of module `{_bySlot[w.Slot].Name}` yielded to a human edit; requeued");
                Enqueue(w);
            }
            catch (Exception e)
            {
                if (_bySlot.TryGetValue(w.Slot, out var failed))
                    SendDiagnostics(failed.Slot, failed.Dir, [], [(AraCodes.CompileServer, "error", e.Message)], 0, w.Origin, validated: false);
                Log(2, $"module {w.Slot}: {e}");
                AnalysisFailure(e);
                if (w.Recovery != 0) RecoveryFinished(w.Recovery, "failed");
            }
            finally
            {
                CancellationTokenSource? run;
                Task? cancelled;
                lock (_lock)
                {
                    run = _agentRun;
                    cancelled = _agentCancellation;
                    _agentRun = null;
                    _agentCancellation = null;
                }
                try { cancelled?.GetAwaiter().GetResult(); }
                catch (Exception error) { Log(2, $"agent cancellation callback failed: {error}"); }
                run?.Dispose();
                CancelAttempts();
                lock (_lock)
                {
                    _busy = false;
                    Monitor.PulseAll(_lock);
                }
                CompileState(false);
                Priority(null);
            }
        }
    }

    void AnalysisFailure(Exception primary)
    {
        if (primary.Data[PendingAnalysis.FailureKey] is Exception failure)
            Log(2, $"analyzer request also failed while retaining the original failure: {failure}");
        if (primary.Data[PendingAnalysis.DiagnosticsKey] is ImmutableArray<Diagnostic> diagnostics)
            foreach (var diagnostic in diagnostics) Log(2, diagnostic.ToString());
    }

    readonly HashSet<string> _priorityFailures = new(StringComparer.Ordinal);

    /// <summary>Restores the helper's launch budget for humans and backgrounds agent/idle work (D20, A13).</summary>
    void Priority(bool? agent)
    {
        if (CompilePriority.Apply(agent) is { } failure && _priorityFailures.Add(failure))
        {
            try { Log(1, failure); }
            catch (Exception error) when (error is IOException or ObjectDisposedException) { /* the host is already closing */ }
        }
    }

    /// <summary>Runs one unit of work; with <c>ARA_TRACE=1</c> it also reports its time and the runtime's GC and JIT activity meanwhile.</summary>
    void Run(Work w, CancellationToken ct)
    {
        if (!StartupTrace.Enabled) { RunCore(w, ct); return; }
        StartupTrace.Mark($"work begin {w.Kind}");
        var before = StartupTrace.Counters.Now();
        var clock = Stopwatch.StartNew();
        try { RunCore(w, ct); }
        finally { StartupTrace.Work(w.Kind.ToString(), clock.Elapsed.TotalMilliseconds, before); }
    }

    void RunCore(Work w, CancellationToken ct)
    {
        if (w.Kind == WorkKind.Recover)
        {
            CompileState(true);
            RecoverAll(w.Recovery, ct);
            return;
        }
        if (!_bySlot.TryGetValue(w.Slot, out var m)) return;
        if (w.Kind == WorkKind.Verify)
        {
            // Nothing to do (and nothing reported) unless the sources differ from what the module compiled.
            var drift = m.DriftedSources();
            if (drift.Length == 0) return;
            w = new Work(WorkKind.Change, w.Slot, w.Origin, w.TEvent);
            w.Paths.UnionWith(drift);
        }
        if (w.Kind == WorkKind.Heat && m.LoadedHot == (w.Hot && Enc)) return;
        Begin(m.Slot, m.Name, m.Profile);
        CompileState(true);
        switch (w.Kind)
        {
            case WorkKind.Initial:
                var unbuilt = m.References.Where(r => !_byName[r].Loaded).ToList();
                if (unbuilt.Count > 0)
                {
                    SendDiagnostics(m.Slot, m.Dir, [], [(AraCodes.DependencyFailed, "error",
                        $"module `{m.Name}` is not built because {string.Join(", ", unbuilt.Select(u => $"`{u}`"))} did not build")], 0, w.Origin, validated: false);
                    return;
                }
                Handle(Evaluated(m.Initial(this, ct), ct), w.Origin, ct);
                m.Prewarm(this, ct);
                break;
            case WorkKind.Change:
                Handle(Evaluated(m.Compile(w.Paths, w.TEvent, this, ct), ct), w.Origin, ct);
                break;
            case WorkKind.Heat:
                if (m.LoadedHot == (w.Hot && Enc)) return;
                Handle(Evaluated(m.Full(Escalation.Requested, w.Hot, this, ct), ct), w.Origin, ct);
                break;
            case WorkKind.Rebuild:
                Handle(Evaluated(m.Full(w.Escalation, null, this, ct), ct), w.Origin, ct);
                break;
            case WorkKind.PublishedRebind:
                Handle(Evaluated(m.Cascade(this, ct), ct), w.Origin, ct);
                break;
        }
    }

    void RecoveryFinished(ulong ticket, string outcome) =>
        _out.Send(MessageKind.RecoveryFinished, new Protocol.Writer().U64(ticket).Str(outcome).Strs(_modules.Select(m => m.Name).ToArray()).ToArray());

    void RecoverAll(ulong ticket, CancellationToken cancellation)
    {
        if (_modules.Count == 0) { RecoveryFinished(ticket, "unchanged"); return; }
        _cascade = [];
        var outcomes = new List<Outcome>();
        try
        {
            foreach (var module in _modules)
            {
                Begin(module.Slot, module.Name, module.Profile);
                var outcome = module.Initial(this, cancellation);
                outcomes.Add(outcome);
                if (_restartSent || outcome.Kind != OutcomeKind.Image)
                {
                    outcome.Extra.Add((AraCodes.RestartRequired, "error", "Script recovery cannot activate this module graph or changed engine bindings. Stop Play and reopen the project; no restart was performed."));
                    foreach (var pending in outcomes)
                        SendDiagnostics(pending.Module.Slot, pending.Module.Dir, pending.Problems,
                            [.. pending.Extra, (AraCodes.LoadFailed, "error", "Script recovery failed; all modules retain their last good code and fault state.")], 0, AbiConst.OriginHuman, validated: false);
                    RecoveryFinished(ticket, "failed");
                    return;
                }
                _cascade[module] = outcome;
            }
            cancellation.ThrowIfCancellationRequested();
            if (_restartSent || outcomes.Any(o => !o.Module.MatchesCurrent(o, this)))
            {
                foreach (var pending in outcomes)
                    SendDiagnostics(pending.Module.Slot, pending.Module.Dir, pending.Problems,
                        [(AraCodes.LoadFailed, "error", "Script inputs changed during recovery; retry. Nothing was activated.")], 0, AbiConst.OriginHuman, validated: false);
                RecoveryFinished(ticket, "failed");
                return;
            }
            foreach (var outcome in outcomes) outcome.Module.Commit(outcome);
            foreach (var outcome in outcomes)
                SendDiagnostics(outcome.Module.Slot, outcome.Module.Dir, outcome.Problems, outcome.Extra, 0, AbiConst.OriginHuman, validated: true);
            SendReload(outcomes, AbiConst.OriginHuman, ticket);
        }
        finally { _cascade = null; }
    }

    static Outcome Evaluated(Outcome outcome, CancellationToken cancellation)
    {
        cancellation.ThrowIfCancellationRequested();
        return outcome;
    }

    void Handle(Outcome o, uint origin, CancellationToken cancellation)
    {
        var m = o.Module;
        StartupTrace.Outcome($"{o.Kind}/{(o.ReloadKind == AbiConst.ReloadL1 ? "L1" : o.ReloadKind == AbiConst.ReloadL2 ? "L2" : o.ReloadKind == AbiConst.ReloadL3 ? "L3" : o.ReloadKind == AbiConst.ReloadInitial ? "initial" : o.ReloadKind.ToString())}");
        switch (o.Kind)
        {
            case OutcomeKind.None:
                if (!ValidateForPublication([o], o, origin, cancellation)) return;
                if (o.Next is not null) { m.Commit(o); RebindDependents(m); }
                SendDiagnostics(m.Slot, m.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
                return;
            case OutcomeKind.Errors:
                if (TryEditWave(o, origin, cancellation)) return;
                SendDiagnostics(m.Slot, m.Dir, o.Problems, o.Extra, o.Timing.ToTiming().ServerUs, origin, o.ValidatedInputs);
                return;
            case OutcomeKind.Restart:
                _out.Send(MessageKind.Restart, new Protocol.Writer().U32(m.Slot).Str("module/source domain boundary changed; restart Play from the current snapshot to load the new inventory (L4)").ToArray());
                return;
            case OutcomeKind.Delta:
                if (!ValidateForPublication([o], o, origin, cancellation)) return;
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
            if (m.Loaded) list.Add(m);
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
                var o = d.Cascade(this, cancellation, root.Intent);
                if (o.Kind == OutcomeKind.Restart)
                {
                    Handle(o, origin, cancellation);
                    return;
                }
                if (o.Kind == OutcomeKind.Errors)
                {
                    if (TryEditWave(root, origin, cancellation)) return;
                    o.Extra.Add((AraCodes.DependencyFailed, "error",
                        $"module `{d.Name}` does not compile against the change to `{root.Module.Name}`; both keep their last good version"));
                    SendDiagnostics(d.Slot, d.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
                    SendDiagnostics(root.Module.Slot, root.Module.Dir, root.Problems.Where(p => p.Severity != Microsoft.CodeAnalysis.DiagnosticSeverity.Error).ToList(),
                        [(AraCodes.DependencyFailed, "error", $"the change to `{root.Module.Name}` breaks module `{d.Name}` (see its diagnostics); nothing was applied")],
                        root.Timing.ToTiming().ServerUs, origin, root.ValidatedInputs);
                    return;
                }
                _cascade[d] = o;
                outcomes.Add(o);
            }
            cancellation.ThrowIfCancellationRequested();
            if (!ValidateForPublication(outcomes, root, origin, cancellation)) return;
            foreach (var o in outcomes) o.Module.Commit(o);
            foreach (var o in outcomes) SendDiagnostics(o.Module.Slot, o.Module.Dir, o.Problems, o.Extra, 0, origin, o.ValidatedInputs);
            SendReload(outcomes, origin);
        }
        finally
        {
            _cascade = null;
        }
    }

    // A refactor can change independent roots before their shared dependent.
    // Expand only a failed source attempt; successful L1/single-root work keeps
    // its usual path and never inspects unrelated saved modules.
    bool TryEditWave(Outcome trigger, uint origin, CancellationToken cancellation)
    {
        if (trigger.Intent != CompileIntent.CurrentSources || !trigger.Module.Loaded) return false;
        var previousCascade = _cascade;
        var previousInventories = _waveInventories;
        _waveInventories = [];
        try
        {
            var closures = new Dictionary<ModuleSession, HashSet<ModuleSession>>();
            HashSet<ModuleSession> Closure(ModuleSession module)
            {
                if (!closures.TryGetValue(module, out var closure))
                    closures[module] = closure = new HashSet<ModuleSession>(Dependents(module)) { module };
                return closure;
            }
            var affected = new HashSet<ModuleSession>(Closure(trigger.Module));
            var inspected = new HashSet<ModuleSession>();
            var expanded = false;
            while (true)
            {
                var grew = false;
                foreach (var module in _modules)
                {
                    cancellation.ThrowIfCancellationRequested();
                    if (!module.Loaded || module.ReadOnlyPublished || affected.Contains(module)) continue;
                    var closure = Closure(module);
                    // Join through an affected dependent, never merely through
                    // a shared unchanged ancestor or an unrelated saved edit.
                    if (!closure.Overlaps(affected) || !inspected.Add(module)) continue;
                    if (!module.SavedSourcesChanged(this)) continue;
                    affected.UnionWith(closure);
                    grew = expanded = true;
                }
                if (!grew) break;
            }
            if (!expanded) return false;
            var order = _modules.Where(affected.Contains).ToArray();
            foreach (var module in order)
                if (!_attempts.ContainsKey(module.Slot)) Begin(module.Slot, module.Name, module.Profile);
            _cascade = [];
            var outcomes = new List<Outcome>();
            foreach (var module in order)
            {
                cancellation.ThrowIfCancellationRequested();
                var outcome = module == trigger.Module && trigger.Kind == OutcomeKind.Image && module.MatchesCurrent(trigger, this)
                    ? trigger : module.Cascade(this, cancellation);
                if (outcome.Kind == OutcomeKind.Restart)
                {
                    Handle(outcome, origin, cancellation);
                    return true;
                }
                if (outcome.Kind == OutcomeKind.Errors)
                {
                    foreach (var member in order)
                    {
                        if (member == module)
                            SendDiagnostics(member.Slot, member.Dir, outcome.Problems, outcome.Extra, 0, origin, outcome.ValidatedInputs);
                        else
                        {
                            _cascade.TryGetValue(member, out var staged);
                            SendDiagnostics(member.Slot, member.Dir, [],
                                [(AraCodes.DependencyFailed, "error", $"source edit wave failed in `{module.Name}`; every module keeps its last good code")],
                                0, origin, staged?.ValidatedInputs ?? false);
                        }
                    }
                    return true;
                }
                _cascade[module] = outcome;
                outcomes.Add(outcome);
            }
            cancellation.ThrowIfCancellationRequested();
            if (!ValidateForPublication(outcomes, trigger, origin, cancellation)) return true;
            foreach (var outcome in outcomes) outcome.Module.Commit(outcome);
            foreach (var outcome in outcomes)
                SendDiagnostics(outcome.Module.Slot, outcome.Module.Dir, outcome.Problems, outcome.Extra, 0, origin, outcome.ValidatedInputs);
            SendReload(outcomes, origin);
            return true;
        }
        finally
        {
            _cascade = previousCascade;
            _waveInventories = previousInventories;
        }
    }

    bool ValidateForPublication(IReadOnlyList<Outcome> outcomes, Outcome trigger, uint origin, CancellationToken cancellation)
    {
        var current = outcomes.All(outcome => outcome.Module.MatchesCurrent(outcome, this));
        cancellation.ThrowIfCancellationRequested();
        if (current) return true;
        // Includes deltas and diagnostic-only restoration. Stale candidates may
        // neither publish executable bytes nor clear an older current error scope.
        foreach (var outcome in outcomes)
            SendDiagnostics(outcome.Module.Slot, outcome.Module.Dir, [], [], 0, origin, validated: false);
        if (trigger.Intent == CompileIntent.CurrentSources)
        {
            Log(0, "script inputs changed during preparation; nothing applied, recompiling current inputs");
            Enqueue(new Work(WorkKind.Change, trigger.Module.Slot, origin, Stopwatch.GetTimestamp()));
        }
        else Log(0, "script inputs changed during a committed-code rebuild; nothing applied and current source inputs remain unvalidated");
        return false;
    }

    /// <inheritdoc/>
    public ProjectInventoryResult Inventory(ModuleInputs project)
    {
        if (_waveInventories is null) return project.Scan();
        var key = (project.Project, project.AuthoringRoot);
        if (!_waveInventories.TryGetValue(key, out var inventory)) _waveInventories[key] = inventory = project.Scan();
        return inventory;
    }

    /// <inheritdoc/>
    public ModuleState? Dependency(string name)
    {
        if (!_byName.TryGetValue(name, out var m)) return null;
        return _cascade is not null && _cascade.TryGetValue(m, out var o) && o.Next is { } prepared ? prepared : m.State;
    }

    /// <inheritdoc/>
    public MetadataReference? Reference(string name) => Dependency(name)?.Reference
        ?? (_byName.TryGetValue(name, out var module) ? module.LoadedReference : null);

    /// <inheritdoc/>
    public string? Surface(string name) => Dependency(name)?.Surface;

    /// <inheritdoc/>
    public ImmutableDictionary<string, string>? CommittedTexts(string name) => Dependency(name)?.Texts
        ?? (_byName.TryGetValue(name, out var module) ? module.LoadedTexts : null);

    /// <inheritdoc/>
    public IEnumerable<string>? CommittedPaths(string name) => Dependency(name)?.Texts.Keys
        ?? (_byName.TryGetValue(name, out var module) ? module.LoadedPaths : null);

    /// <inheritdoc/>
    public bool HasOpaqueSourceOwnership => _modules.Any(module => module.ReadOnlyPublished && module.LoadedPaths is null);

    /// <inheritdoc/>
    public string? PublishedFingerprint(string name) => _cascade is not null && _byName.TryGetValue(name, out var candidate) && _cascade.ContainsKey(candidate)
        ? null : _byName.TryGetValue(name, out var module) ? module.PublicationFingerprint : null;

    /// <inheritdoc/>
    public bool HasDependents(ModuleSession module) => _modules.Any(m => m.References.Contains(module.Name));

    void SendReload(List<Outcome> outcomes, uint origin, ulong recovery = 0)
    {
        var w = new Protocol.Writer().Now().U32((uint)outcomes.Count);
        foreach (var o in outcomes)
        {
            var s = o.Next!;
            w.U32(o.Module.Slot).U32(s.Generation).Bool(s.Hot).U32(o.ReloadKind).U32((uint)o.Escalation).U32(origin)
                .Timing(o.Timing.ToTiming()).Bytes(o.Image?.Pe ?? []).Bytes(o.Image?.Pdb ?? []);
        }
        w.U64(recovery);
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
    void RecordReference(string path)
    {
        path = Path.GetFullPath(path);
        if (_references.ContainsKey(path) || !File.Exists(path)) return;
        _references[path] = Mvid(path) ?? Guid.Empty;
    }

    void WatchReference(string path)
    {
        path = Path.GetFullPath(path);
        lock (_lock)
        {
            // Another module's start may be creating (or have created) this watcher; and a path is only "watched"
            // once its watcher is live, so a failed creation can be retried.
            if (!_references.ContainsKey(path) || _referenceWatched.Contains(path) || !_referenceStarting.Add(path)) return;
        }
        try
        {
            var w = ReferenceWatcherFactory(Path.GetDirectoryName(path)!, Path.GetFileName(path));
            w.NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size;
            FileSystemEventHandler on = (_, _) => ThreadPool.QueueUserWorkItem(_ => CheckReference(path));
            w.Changed += on;
            w.Created += on;
            try { w.EnableRaisingEvents = true; }
            catch { w.Dispose(); throw; }
            lock (_referenceWatchers) _referenceWatchers.Add(w);
            lock (_lock) _referenceWatched.Add(path);
        }
        finally { lock (_lock) _referenceStarting.Remove(path); }
        // A rebuild between the baseline and the watcher going live raised no event.
        ThreadPool.QueueUserWorkItem(_ => CheckReference(path));
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

    /// <summary>Requests worker cancellation and stops watchers; the host separately bounds helper-process shutdown.</summary>
    public void Dispose()
    {
        _stop.Cancel();
        _wake.Set();
        Task[] starts;
        lock (_watchStarts) starts = [.. _watchStarts];
        try { Task.WaitAll(starts, TimeSpan.FromSeconds(5)); } catch (AggregateException) { /* each start logs its own failure */ }
        _watcher.Dispose();
        lock (_referenceWatchers) foreach (var w in _referenceWatchers) w.Dispose();
    }
}

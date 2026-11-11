using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Watches module folders for <c>*.cs</c> changes and reports one change set per module per
/// save: events are coalesced over a 1 ms settle (a truncate+write pair reads once). One pump
/// thread serves every module. On macOS, <c>FileSystemWatcher</c> (FSEvents) reports an in-place save
/// ~11 ms late, so source files also carry a kqueue watch (<see cref="VnodeEvents"/>) that reports the
/// write at once; the late directory event for the same write is dropped as a duplicate.
/// </summary>
internal sealed class ModuleWatcher : IDisposable
{
    /// <summary>Actual compiler inputs; directory notices still trigger inventory validation.</summary>
    internal static bool IsSourceInput(string path) => path.EndsWith(".cs", StringComparison.OrdinalIgnoreCase)
        || path.EndsWith(ProjectInventory.ManifestSuffix, StringComparison.OrdinalIgnoreCase);
    const double SettleMs = 1.0;
    /// <summary>A late directory <c>Changed</c> or <c>Created</c> event this soon after a vnode event is the same write.</summary>
    const double DuplicateWindowMs = 100.0;

    readonly Action<uint, IReadOnlyCollection<string>, long> _changed;
    readonly BlockingCollection<(uint Slot, string Path, long Ticks)> _events = [];
    readonly List<FileSystemWatcher> _watchers = [];
    readonly CancellationTokenSource _stop = new();
    readonly Thread _pump;
    readonly VnodeEvents? _vnode;
    /// <summary>The last fast delivery per file: when, and the file's modification time and length then.</summary>
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, (long At, long Mtime, long Length)> _vnodeWrites = new(StringComparer.Ordinal);

    /// <summary><paramref name="changed"/> receives (slot, paths, first event timestamp) on the pump thread.</summary>
    internal ModuleWatcher(Action<uint, IReadOnlyCollection<string>, long> changed)
    {
        _changed = changed;
        if (OperatingSystem.IsMacOS())
        {
            try { _vnode = new VnodeEvents(OnVnode); }
            catch (Exception) { /* no kqueue: the directory watcher alone, as on every other platform */ }
        }
        _pump = new Thread(Pump) { Name = "ara-watch", IsBackground = true };
        _pump.Start();
    }

    /// <summary>Starts watching a module folder.</summary>
    internal void Watch(uint slot, string dir)
    {
        var w = new FileSystemWatcher(dir, "*")
        {
            IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.Size,
            InternalBufferSize = 64 * 1024,
        };
        static bool Relevant(string path) => IsSourceInput(path) || !Path.HasExtension(path);
        FileSystemEventHandler on = (_, e) =>
        {
            if (!Relevant(e.FullPath) || (e.ChangeType is WatcherChangeTypes.Changed or WatcherChangeTypes.Created && IsVnodeDuplicate(e.FullPath))) return;
            _events.Add((slot, e.FullPath, Stopwatch.GetTimestamp()));
            // A new or replaced source file needs a fresh vnode watch.
            if (_vnode is not null && IsSourceInput(e.FullPath) && e.ChangeType != WatcherChangeTypes.Deleted) AddVnode(slot, e.FullPath);
        };
        w.Changed += on;
        w.Created += on;
        w.Deleted += on;
        w.Renamed += (_, e) =>
        {
            var t = Stopwatch.GetTimestamp();
            if (Relevant(e.OldFullPath)) _events.Add((slot, e.OldFullPath, t));
            if (Relevant(e.FullPath)) _events.Add((slot, e.FullPath, t));
            if (_vnode is not null && IsSourceInput(e.FullPath)) AddVnode(slot, e.FullPath);
        };
        // A failed start leaves nothing behind, so the caller can retry without watching the folder twice.
        try
        {
            w.EnableRaisingEvents = true;
            if (_vnode is not null)
                foreach (var file in SourceFiles(dir)) AddVnode(slot, file);
        }
        catch { w.Dispose(); throw; }
        lock (_watchers) _watchers.Add(w);
    }

    /// <summary>
    /// The file watch of an in-place write: queued at once, and remembered with the file's stat to drop the directory
    /// watcher's echo. Truncation alone raises no event here (<c>NOTE_ATTRIB</c> is not subscribed), so the first event is a
    /// write; a save written in chunks raises one event per write and each is reported, so the final state always arrives (the
    /// pump's 1 ms settle merges steady writes; a writer that pauses longer gets a compile of the partial file first, as it
    /// did through FSEvents past its 11 ms). A wait here would only add latency, and under load it was measured to erase the gain.
    /// </summary>
    void OnVnode(uint slot, string path)
    {
        var stat = Stat(path) ?? default;
        var now = Stopwatch.GetTimestamp();
        _vnodeWrites[path] = (now, stat.Mtime, stat.Length);
        if (_vnodeWrites.Count > 4096) PruneVnodeWrites();
        _events.Add((slot, path, now));
    }

    /// <summary>Forgets deliveries older than a minute so the table does not grow with every path ever watched.</summary>
    void PruneVnodeWrites()
    {
        foreach (var (path, delivery) in _vnodeWrites)
            if (Stopwatch.GetElapsedTime(delivery.At).TotalSeconds > 60) _vnodeWrites.TryRemove(path, out _);
    }

    static (long Mtime, long Length)? Stat(string path)
    {
        try
        {
            var file = new FileInfo(path);
            return file.Exists ? (file.LastWriteTimeUtc.Ticks, file.Length) : null;
        }
        catch (IOException) { return null; }
        catch (UnauthorizedAccessException) { return null; }
    }

    /// <summary>
    /// FSEvents reports one in-place write as <c>Changed</c> and often <c>Created</c> too, ~11 ms late. It is that write's
    /// echo when a vnode delivery for the file is under 100 ms old, its vnode watch is alive (a delete or rename drops it), and
    /// the file still has the modification time and length seen at the delivery. Anything that changed since (a save that ends
    /// with a shrinking truncate, a second write) passes, so no final state is ever dropped.
    /// </summary>
    bool IsVnodeDuplicate(string path) =>
        _vnode is not null && _vnode.Watching(path) && _vnodeWrites.TryGetValue(path, out var delivery)
        && Stopwatch.GetElapsedTime(delivery.At).TotalMilliseconds < DuplicateWindowMs
        && Stat(path) is { } now && now.Mtime == delivery.Mtime && now.Length == delivery.Length;

    void AddVnode(uint slot, string path) => _vnode!.Add(slot, path);

    /// <summary>Source inputs under a module folder, skipping build output and hidden folders.</summary>
    static IEnumerable<string> SourceFiles(string dir)
    {
        try
        {
            return Directory.EnumerateFiles(dir, "*", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.Hidden })
                .Where(path => IsSourceInput(path) && !Path.GetRelativePath(dir, path).Split(Path.DirectorySeparatorChar)
                    .Any(part => part is "bin" or "obj" || part.StartsWith('.')))
                .ToArray();
        }
        catch (IOException) { return []; }
    }

    void Pump()
    {
        var settle = (long)(SettleMs * Stopwatch.Frequency / 1000);
        try
        {
            while (!_stop.IsCancellationRequested)
            {
                var first = _events.Take(_stop.Token);
                var sets = new Dictionary<uint, (HashSet<string> Paths, long First)>
                {
                    [first.Slot] = (new(StringComparer.OrdinalIgnoreCase) { first.Path }, first.Ticks),
                };
                // Spin, don't sleep: Windows timed waits round up to the 15.6 ms timer tick (S2 finding 8).
                var until = Stopwatch.GetTimestamp() + settle;
                while (Stopwatch.GetTimestamp() < until)
                {
                    if (_events.TryTake(out var e))
                    {
                        if (sets.TryGetValue(e.Slot, out var s)) s.Paths.Add(e.Path);
                        else sets[e.Slot] = (new(StringComparer.OrdinalIgnoreCase) { e.Path }, e.Ticks);
                        until = Stopwatch.GetTimestamp() + settle;
                    }
                    else Thread.Yield();
                }
                foreach (var (slot, (paths, t)) in sets) _changed(slot, paths, t);
            }
        }
        catch (OperationCanceledException) { }
    }

    /// <summary>Folder watchers started so far.</summary>
    internal int Count { get { lock (_watchers) return _watchers.Count; } }

    /// <summary>Stops every watcher and the pump.</summary>
    public void Dispose()
    {
        _stop.Cancel();
        _vnode?.Dispose();
        lock (_watchers)
        {
            foreach (var w in _watchers) w.Dispose();
            _watchers.Clear();
        }
    }
}

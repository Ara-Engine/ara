using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Watches module folders for <c>*.cs</c> changes and reports one change set per module per
/// save: events are coalesced over a 1 ms settle (a truncate+write pair reads once). One pump
/// thread serves every module. On macOS, <c>FileSystemWatcher</c> (FSEvents) reports an in-place save
/// ~11 ms late, so source files and folders also carry kqueue watches (<see cref="VnodeEvents"/>) that report in-place
/// saves, atomic saves and new files at once; the late directory-watcher event for the same save is dropped as a duplicate.
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
    /// <summary>The last fast delivery per file and the file's stat at that moment (a late directory event is its echo while the stat still matches).</summary>
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, (long At, long Mtime, long Length)> _deliveries = new(StringComparer.Ordinal);
    /// <summary>Source files' last seen modification time and length, to find which file a directory event is about.</summary>
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, (long Mtime, long Length)> _known = new(StringComparer.Ordinal);
    /// <summary>Folders whose entries changed and have not been rescanned yet (folder, slot); the scan thread debounces them.</summary>
    readonly ConcurrentDictionary<string, uint> _dirty = new(StringComparer.Ordinal);
    readonly AutoResetEvent _scanWake = new(false);
    readonly Thread? _scan;
    int _scans;

    /// <summary><paramref name="changed"/> receives (slot, paths, first event timestamp) on the pump thread.</summary>
    internal ModuleWatcher(Action<uint, IReadOnlyCollection<string>, long> changed, int vnodeBudget = int.MaxValue)
    {
        _changed = changed;
        if (OperatingSystem.IsMacOS())
        {
            try
            {
                _vnode = new VnodeEvents(OnVnode, MarkDirty, Math.Min(vnodeBudget, 1024));
                _scan = new Thread(ScanLoop) { Name = "ara-vnode-scan", IsBackground = true };
                _scan.Start();
            }
            catch (Exception) { /* no kqueue: the directory watcher alone, as on every other platform */ }
        }
        _pump = new Thread(Pump) { Name = "ara-watch", IsBackground = true };
        _pump.Start();
    }

    /// <summary>Folder rescans run so far (tests: a burst of entry changes must not cost one scan per event).</summary>
    internal int ScanCount => Volatile.Read(ref _scans);

    /// <summary>True when the kqueue layer watches <paramref name="path"/> (a file or a folder).</summary>
    internal bool VnodeWatching(string path) => _vnode?.Watching(path) == true;

    /// <summary>Files and folders under kqueue watches right now (tests).</summary>
    internal int VnodeWatchedCount => _vnode?.WatchedCount ?? 0;

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
            if (!Relevant(e.FullPath)) return;
            // Arm first, then maybe drop the echo: an echo must never be the reason a replaced file or a new folder stays unwatched.
            if (_vnode is not null) ArmFromEvent(slot, e.FullPath, e.ChangeType);
            if (IsFastDuplicate(e.FullPath, e.ChangeType)) return;
            if (_vnode is not null && IsSourceInput(e.FullPath)) RememberOrForget(e.FullPath, e.ChangeType);
            _events.Add((slot, e.FullPath, Stopwatch.GetTimestamp()));
        };
        w.Changed += on;
        w.Created += on;
        w.Deleted += on;
        w.Renamed += (_, e) =>
        {
            var t = Stopwatch.GetTimestamp();
            if (_vnode is not null)
            {
                _vnode.RemoveUnder(e.OldFullPath); // a renamed folder's descendants are registered under paths that no longer exist
                _vnode.RemovePath(e.OldFullPath);
                ArmFromEvent(slot, e.FullPath, e.ChangeType);
            }
            if (Relevant(e.OldFullPath)) _events.Add((slot, e.OldFullPath, t));
            if (Relevant(e.FullPath) && !IsFastDuplicate(e.FullPath, e.ChangeType))
            {
                if (_vnode is not null && IsSourceInput(e.FullPath)) RememberOrForget(e.FullPath, e.ChangeType);
                _events.Add((slot, e.FullPath, t));
            }
        };
        w.EnableRaisingEvents = true;
        if (_vnode is not null) ArmFolder(slot, dir);
        lock (_watchers) _watchers.Add(w);
    }

    /// <summary>
    /// Arms what a directory-watcher event tells about: a source file (new or replaced) gets a file watch, a folder (new,
    /// or moved in by a rename) gets its folder watches, and a deleted folder drops everything under it.
    /// </summary>
    void ArmFromEvent(uint slot, string path, WatcherChangeTypes type)
    {
        if (type == WatcherChangeTypes.Deleted)
        {
            if (!IsSourceInput(path)) { _vnode!.RemoveUnder(path); _vnode.RemovePath(path); }
            return;
        }
        if (IsSourceInput(path)) AddVnode(slot, path);
        else if (type is WatcherChangeTypes.Created or WatcherChangeTypes.Renamed && Directory.Exists(path) && !IsSkippedFolder(Path.GetFileName(path)))
            ArmFolder(slot, path);
    }

    /// <summary>
    /// Folder watches first, then file watches: one folder watch covers every atomic save in its folder, a file watch only
    /// one file's in-place saves, so when the descriptor budget is small the folders must not be starved by the files.
    /// Files already in the folder are recorded as known (their change set is the folder event's business).
    /// </summary>
    void ArmFolder(uint slot, string folder)
    {
        AddVnodeDirectory(slot, folder);
        foreach (var sub in SourceFolders(folder)) AddVnodeDirectory(slot, sub);
        foreach (var file in SourceFiles(folder))
        {
            if (Stat(file) is { } stat) _known[file] = stat;
            AddVnode(slot, file);
        }
    }

    /// <summary>Keeps <c>_known</c> in step with changes the slow path reported, so the next folder scan does not redeliver them.</summary>
    void RememberOrForget(string path, WatcherChangeTypes type)
    {
        if (type == WatcherChangeTypes.Deleted) _known.TryRemove(path, out _);
        else if (Stat(path) is { } stat) _known[path] = stat;
    }

    /// <summary>The file watch of an in-place write: queued at once, and remembered to drop the directory watcher's echo.</summary>
    void OnVnode(uint slot, string path)
    {
        var stat = Stat(path);
        Deliver(slot, path, stat ?? default);
    }

    /// <summary>
    /// Entries of a watched folder changed (called on the kqueue thread, so it only records the folder): an atomic save (a
    /// temp file renamed over the source) or a new file looks like this. The scan thread rescans it after a short debounce.
    /// </summary>
    void MarkDirty(uint slot, string folder)
    {
        _dirty[folder] = slot;
        _scanWake.Set();
    }

    /// <summary>
    /// Rescans changed folders on its own thread, never the kqueue thread, so a burst of temp files cannot hold up in-place
    /// saves. A folder is rescanned once per ~1.5 ms burst however many entry events it raised.
    /// </summary>
    void ScanLoop()
    {
        while (!_stop.IsCancellationRequested)
        {
            if (!_scanWake.WaitOne(250)) continue;
            Spin(1.5); // let the burst (a save's temp file, rename and cleanup) finish, collecting more folders
            foreach (var (folder, slot) in _dirty.ToArray())
            {
                if (!_dirty.TryRemove(folder, out _)) continue;
                try { ScanFolder(slot, folder); }
                catch (Exception) { /* the directory watcher still reports the save */ }
            }
        }
    }

    static void Spin(double ms)
    {
        var until = Stopwatch.GetTimestamp() + (long)(ms * Stopwatch.Frequency / 1000);
        while (Stopwatch.GetTimestamp() < until) Thread.SpinWait(20);
    }

    /// <summary>
    /// Reports the source files of <paramref name="folder"/> whose modification time or length differs from what was last
    /// seen. A replaced file gets a fresh file watch; a file that is missing is left to the directory watcher (a save that
    /// deletes the source and then renames the new one in must not be compiled half done). A new file is reported only
    /// once its length and time hold still for ~3 ms, and not while empty: the folder event fires at creation, before
    /// the first write, and a partial source would show as errors until the rest lands. Its own writes raise file events.
    /// </summary>
    void ScanFolder(uint slot, string folder)
    {
        Interlocked.Increment(ref _scans);
        try
        {
            foreach (var info in new DirectoryInfo(folder).EnumerateFiles())
            {
                var path = info.FullName;
                if (!IsSourceInput(path)) continue;
                (long Mtime, long Length) stat;
                try { stat = (info.LastWriteTimeUtc.Ticks, info.Length); }
                catch (IOException) { continue; }
                if (_known.TryGetValue(path, out var known))
                {
                    if (known == stat) continue;
                    _vnode!.Replace(slot, path); // a replacement is a new inode: watch it, whatever the old watch is doing
                    Deliver(slot, path, stat);
                    continue;
                }
                if (!SettleNewFile(path, ref stat)) continue;
                _known[path] = stat;
                _vnode!.Replace(slot, path);
                if (stat.Length > 0) Deliver(slot, path, stat);
            }
        }
        catch (IOException) { /* the folder went away; the directory watcher reports it */ }
        catch (UnauthorizedAccessException) { }
    }

    /// <summary>Waits (up to ~12 ms) until a new file's stat holds still for ~3 ms; false when it disappeared.</summary>
    static bool SettleNewFile(string path, ref (long Mtime, long Length) stat)
    {
        for (var i = 0; i < 4; i++)
        {
            Spin(3);
            if (Stat(path) is not { } now) return false;
            if (now == stat) return true;
            stat = now;
        }
        return true;
    }

    void Deliver(uint slot, string path, (long Mtime, long Length) stat)
    {
        var now = Stopwatch.GetTimestamp();
        _known[path] = stat;
        _deliveries[path] = (now, stat.Mtime, stat.Length);
        if (_deliveries.Count > 4096) PruneDeliveries();
        _events.Add((slot, path, now));
    }

    /// <summary>Forgets deliveries older than a minute so the table does not grow with every path ever watched.</summary>
    void PruneDeliveries()
    {
        foreach (var (path, delivery) in _deliveries)
            if (Stopwatch.GetElapsedTime(delivery.At).TotalSeconds > 60) _deliveries.TryRemove(path, out _);
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
    /// Is this directory-watcher event the echo of a save the fast watch already reported? FSEvents reports one save as a few
    /// events (<c>Changed</c>, often <c>Created</c>, <c>Renamed</c> for an atomic save) ~11 ms late. It is an echo only when a
    /// fast delivery for the file is under 100 ms old and the file still has the modification time and length seen at that
    /// delivery. Anything that changed since (a save that ends with a shrinking truncate, a second write) passes, so no final
    /// state is ever dropped. A delete never is an echo.
    /// </summary>
    bool IsFastDuplicate(string path, WatcherChangeTypes type) =>
        _vnode is not null && type != WatcherChangeTypes.Deleted && _deliveries.TryGetValue(path, out var delivery)
        && Stopwatch.GetElapsedTime(delivery.At).TotalMilliseconds < DuplicateWindowMs
        && Stat(path) is { } now && now.Mtime == delivery.Mtime && now.Length == delivery.Length;

    void AddVnode(uint slot, string path) => _vnode!.Add(slot, path);

    void AddVnodeDirectory(uint slot, string path) => _vnode!.AddDirectory(slot, path);

    static bool IsSkippedFolder(string name) => name is "bin" or "obj" || name.StartsWith('.');

    /// <summary>Folders under a module folder, skipping build output and hidden folders.</summary>
    static IEnumerable<string> SourceFolders(string dir)
    {
        try
        {
            return Directory.EnumerateDirectories(dir, "*", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.Hidden })
                .Where(path => !Path.GetRelativePath(dir, path).Split(Path.DirectorySeparatorChar).Any(IsSkippedFolder))
                .ToArray();
        }
        catch (IOException) { return []; }
    }

    /// <summary>Source inputs under a module folder, skipping build output and hidden folders.</summary>
    static IEnumerable<string> SourceFiles(string dir)
    {
        try
        {
            return Directory.EnumerateFiles(dir, "*", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.Hidden })
                .Where(path => IsSourceInput(path) && !Path.GetRelativePath(dir, path).Split(Path.DirectorySeparatorChar)
                    .Any(IsSkippedFolder))
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

    /// <summary>Stops every watcher and the pump.</summary>
    public void Dispose()
    {
        _stop.Cancel();
        _scanWake.Set();
        _scan?.Join(1000);
        _vnode?.Dispose();
        lock (_watchers)
        {
            foreach (var w in _watchers) w.Dispose();
            _watchers.Clear();
        }
    }
}

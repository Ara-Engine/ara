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
    /// <summary>The last fast delivery per file and the file's stat at that moment (a late directory event is its echo while the stat still matches).</summary>
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, (long At, long Mtime, long Length)> _deliveries = new(StringComparer.Ordinal);
    /// <summary>Source files' last seen modification time and length, to find which file a directory event is about.</summary>
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, (long Mtime, long Length)> _known = new(StringComparer.Ordinal);

    /// <summary><paramref name="changed"/> receives (slot, paths, first event timestamp) on the pump thread.</summary>
    internal ModuleWatcher(Action<uint, IReadOnlyCollection<string>, long> changed)
    {
        _changed = changed;
        if (OperatingSystem.IsMacOS())
        {
            try { _vnode = new VnodeEvents(OnVnode, OnDirectory); }
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
            if (!Relevant(e.FullPath) || IsFastDuplicate(e.FullPath, e.ChangeType)) return;
            _events.Add((slot, e.FullPath, Stopwatch.GetTimestamp()));
            if (_vnode is null || e.ChangeType == WatcherChangeTypes.Deleted) return;
            // A new or replaced source file needs a fresh vnode watch, a new folder a directory watch.
            if (IsSourceInput(e.FullPath)) AddVnode(slot, e.FullPath);
            else if (e.ChangeType == WatcherChangeTypes.Created && Directory.Exists(e.FullPath) && !IsSkippedFolder(Path.GetFileName(e.FullPath))) AddVnodeDirectory(slot, e.FullPath);
        };
        w.Changed += on;
        w.Created += on;
        w.Deleted += on;
        w.Renamed += (_, e) =>
        {
            var t = Stopwatch.GetTimestamp();
            if (Relevant(e.OldFullPath)) _events.Add((slot, e.OldFullPath, t));
            if (Relevant(e.FullPath) && !IsFastDuplicate(e.FullPath, e.ChangeType)) _events.Add((slot, e.FullPath, t));
            if (_vnode is not null && IsSourceInput(e.FullPath)) AddVnode(slot, e.FullPath);
        };
        w.EnableRaisingEvents = true;
        if (_vnode is not null)
        {
            foreach (var file in SourceFiles(dir))
            {
                if (Stat(file) is { } stat) _known[file] = stat;
                AddVnode(slot, file);
            }
            AddVnodeDirectory(slot, dir);
            foreach (var folder in SourceFolders(dir)) AddVnodeDirectory(slot, folder);
        }
        lock (_watchers) _watchers.Add(w);
    }

    /// <summary>The file watch of an in-place write: queued at once, and remembered to drop the directory watcher's echo.</summary>
    void OnVnode(uint slot, string path)
    {
        var stat = Stat(path);
        Deliver(slot, path, stat ?? default);
    }

    /// <summary>
    /// Entries of a watched folder changed. An atomic save (a temp file renamed over the source) and a new file look
    /// like this: report the source files whose modification time or length differs from what was last seen. A file that
    /// is missing is left to the directory watcher, so a delete-then-rename save is not compiled half done.
    /// </summary>
    void OnDirectory(uint slot, string directory)
    {
        try
        {
            foreach (var path in Directory.EnumerateFiles(directory))
            {
                if (!IsSourceInput(path) || Stat(path) is not { } stat) continue;
                if (_known.TryGetValue(path, out var known) && known == stat) continue;
                Deliver(slot, path, stat);
                AddVnode(slot, path); // the replacement is a new file: watch it
            }
        }
        catch (IOException) { /* the folder went away; the directory watcher reports it */ }
        catch (UnauthorizedAccessException) { }
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
        _vnode?.Dispose();
        lock (_watchers)
        {
            foreach (var w in _watchers) w.Dispose();
            _watchers.Clear();
        }
    }
}

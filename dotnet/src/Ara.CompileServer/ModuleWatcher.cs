using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Watches module folders for <c>*.cs</c> changes and reports one change set per module per
/// save: events are coalesced over a 1 ms settle (a truncate+write pair reads once). One pump
/// thread serves every module.
/// </summary>
internal sealed class ModuleWatcher : IDisposable
{
    const double SettleMs = 1.0;

    readonly Action<uint, IReadOnlyCollection<string>, long> _changed;
    readonly BlockingCollection<(uint Slot, string Path, long Ticks)> _events = [];
    readonly List<FileSystemWatcher> _watchers = [];
    readonly CancellationTokenSource _stop = new();
    readonly Thread _pump;

    /// <summary><paramref name="changed"/> receives (slot, paths, first event timestamp) on the pump thread.</summary>
    internal ModuleWatcher(Action<uint, IReadOnlyCollection<string>, long> changed)
    {
        _changed = changed;
        _pump = new Thread(Pump) { Name = "ara-watch", IsBackground = true };
        _pump.Start();
    }

    /// <summary>Starts watching a module folder.</summary>
    internal void Watch(uint slot, string dir)
    {
        var w = new FileSystemWatcher(dir, "*.cs")
        {
            IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
            InternalBufferSize = 64 * 1024,
        };
        FileSystemEventHandler on = (_, e) => _events.Add((slot, e.FullPath, Stopwatch.GetTimestamp()));
        w.Changed += on;
        w.Created += on;
        w.Deleted += on;
        w.Renamed += (_, e) =>
        {
            var t = Stopwatch.GetTimestamp();
            _events.Add((slot, e.OldFullPath, t));
            _events.Add((slot, e.FullPath, t));
        };
        w.EnableRaisingEvents = true;
        lock (_watchers) _watchers.Add(w);
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
        lock (_watchers)
        {
            foreach (var w in _watchers) w.Dispose();
            _watchers.Clear();
        }
    }
}

using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer.Tests;

/// <summary>The kqueue folder and file watches under folder renames, bursts of entries, a small descriptor budget and slow writers.</summary>
public sealed class VnodeFolderTests : IDisposable
{
    readonly string _root = InstalledTools.Scratch("vnode-folders");
    readonly BlockingCollection<(string[] Paths, long At)> _sets = [];
    readonly ConcurrentQueue<string> _contents = new();
    ModuleWatcher? _watcher;

    ModuleWatcher Watch(string dir, int budget = int.MaxValue)
    {
        _watcher = new ModuleWatcher((_, paths, _) =>
        {
            foreach (var path in paths.Where(path => path.EndsWith("N.cs", StringComparison.Ordinal) && File.Exists(path)))
                try { _contents.Enqueue(File.ReadAllText(path)); } catch (IOException) { }
            _sets.Add(([.. paths], Stopwatch.GetTimestamp()));
        }, budget);
        _watcher.Watch(0, dir);
        Thread.Sleep(400); // the directory watcher starts asynchronously
        Drain();
        return _watcher;
    }

    void Drain()
    {
        Thread.Sleep(100);
        while (_sets.TryTake(out _)) { }
    }

    string Make(string relative, string text = "class C { }")
    {
        var path = Path.Combine(_root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
        return path;
    }

    /// <summary>Waits for a set naming <paramref name="file"/>; the time it arrived, or null.</summary>
    long? Next(string file, int ms = 3000)
    {
        var deadline = Stopwatch.GetTimestamp() + ms * Stopwatch.Frequency / 1000;
        while (Stopwatch.GetTimestamp() < deadline)
            if (_sets.TryTake(out var set, 20) && set.Paths.Any(path => Path.GetFileName(path) == file)) return set.At;
        return null;
    }

    static bool Eventually(Func<bool> condition, int ms = 3000)
    {
        var deadline = System.Environment.TickCount64 + ms;
        while (System.Environment.TickCount64 < deadline)
        {
            if (condition()) return true;
            Thread.Sleep(5);
        }
        return condition();
    }

    double MedianInPlaceLatency(string path, int saves = 10)
    {
        var latencies = new List<double>();
        for (var i = 0; i < saves; i++)
        {
            Drain();
            var start = Stopwatch.GetTimestamp();
            File.WriteAllText(path, $"class C {{ int v{i}; }}");
            var at = Next(Path.GetFileName(path)) ?? throw new Xunit.Sdk.XunitException("save not delivered");
            latencies.Add(Stopwatch.GetElapsedTime(start, at).TotalMilliseconds);
        }
        latencies.Sort();
        return latencies[latencies.Count / 2];
    }

    [MacOnlyFact]
    public void A_folder_renamed_while_watched_is_rearmed_and_its_old_entries_are_released()
    {
        var file = Make("Sub/S.cs");
        Make("A.cs");
        var watcher = Watch(_root);
        Assert.True(watcher.VnodeWatching(Path.Combine(_root, "Sub")));
        Assert.True(watcher.VnodeWatching(file));
        var before = watcher.VnodeWatchedCount;

        Directory.Move(Path.Combine(_root, "Sub"), Path.Combine(_root, "Sub2"));
        var moved = Path.Combine(_root, "Sub2", "S.cs");
        // The old paths are released (they would hold budget and block a folder created under the old name)...
        Assert.True(Eventually(() => !watcher.VnodeWatching(file) && !watcher.VnodeWatching(Path.Combine(_root, "Sub"))), "stale entries kept");
        // ...and the folder is armed under its new name, files and all.
        Assert.True(Eventually(() => watcher.VnodeWatching(Path.Combine(_root, "Sub2")) && watcher.VnodeWatching(moved)), "renamed folder never re-armed");
        Assert.Equal(before, watcher.VnodeWatchedCount);
        Assert.True(MedianInPlaceLatency(moved) < 8, "a save in the renamed folder still took the slow path");

        // A new folder at the old name is armed too (a stale entry would block it).
        var recreated = Make("Sub/S.cs");
        Assert.True(Eventually(() => watcher.VnodeWatching(Path.Combine(_root, "Sub")) && watcher.VnodeWatching(recreated)), "folder recreated under the old name was not armed");
    }

    [MacOnlyFact]
    public void Deleting_a_watched_folder_releases_everything_under_it()
    {
        Make("A.cs");
        Make("Gone/G.cs");
        Make("Gone/Deep/D.cs");
        var watcher = Watch(_root);
        var withFolder = watcher.VnodeWatchedCount;
        Directory.Delete(Path.Combine(_root, "Gone"), recursive: true);
        Assert.True(Eventually(() => watcher.VnodeWatchedCount == withFolder - 4), $"{watcher.VnodeWatchedCount} still watched; the folder had 4 entries (itself, G.cs, Deep, D.cs)");
    }

    [MacOnlyFact]
    public void A_burst_of_two_hundred_temp_files_does_not_hold_up_saves_or_cost_a_scan_each()
    {
        var file = Make("A.cs");
        // A big folder makes each scan cost real time (a stat per source file).
        foreach (var i in Enumerable.Range(0, 400)) File.WriteAllText(Path.Combine(_root, $"Big{i:D3}.cs"), "class C { }");
        var watcher = Watch(_root);
        var scansBefore = watcher.ScanCount;
        var burst = Task.Run(() =>
        {
            for (var i = 0; i < 200; i++) File.WriteAllText(Path.Combine(_root, $"t{i}.cs___jb_tmp___"), "temp");
        });
        var latencies = new List<double>();
        for (var i = 0; i < 6; i++)
        {
            var start = Stopwatch.GetTimestamp();
            File.WriteAllText(file, $"class C {{ int b{i}; }}");
            var at = Next("A.cs") ?? throw new Xunit.Sdk.XunitException("save not delivered during the burst");
            latencies.Add(Stopwatch.GetElapsedTime(start, at).TotalMilliseconds);
            Thread.Sleep(2);
        }
        burst.Wait();
        Thread.Sleep(300);
        latencies.Sort();
        Assert.True(latencies[latencies.Count / 2] < 12, $"median {latencies[latencies.Count / 2]:F1} ms during the burst");
        var scans = watcher.ScanCount - scansBefore;
        Assert.True(scans < 100, $"{scans} folder scans for 200 entries");
    }

    [MacOnlyFact]
    public void A_module_with_more_source_files_than_the_budget_still_gets_its_folder_watch()
    {
        foreach (var i in Enumerable.Range(0, 20)) Make($"F{i:D2}.cs");
        var watcher = Watch(_root, budget: 8);
        Assert.True(watcher.VnodeWatching(_root), "the folder watch was starved by file watches");
        Assert.True(watcher.VnodeWatchedCount <= 8);
        // An atomic save of a file that has no file watch of its own is still fast: the folder watch reports it.
        var unwatched = Enumerable.Range(0, 20).Select(i => Path.Combine(_root, $"F{i:D2}.cs")).First(path => !watcher.VnodeWatching(path));
        var latencies = new List<double>();
        for (var i = 0; i < 10; i++)
        {
            Drain();
            var start = Stopwatch.GetTimestamp();
            File.WriteAllText(unwatched + ".tmp", $"class C {{ int a{i}; }}");
            File.Move(unwatched + ".tmp", unwatched, overwrite: true);
            var at = Next(Path.GetFileName(unwatched)) ?? throw new Xunit.Sdk.XunitException("save not delivered");
            latencies.Add(Stopwatch.GetElapsedTime(start, at).TotalMilliseconds);
        }
        latencies.Sort();
        Assert.True(latencies[latencies.Count / 2] < 8, $"median {latencies[latencies.Count / 2]:F1} ms");
    }

    /// <summary>An in-place change to a file with no file watch arrives from the slow path; the next folder scan must not report it again.</summary>
    [MacOnlyFact]
    public void A_change_the_slow_path_reported_is_not_reported_again_by_the_next_folder_scan()
    {
        foreach (var i in Enumerable.Range(0, 12)) Make($"F{i:D2}.cs");
        var watcher = Watch(_root, budget: 4);
        var unwatched = Enumerable.Range(0, 12).Select(i => Path.Combine(_root, $"F{i:D2}.cs")).First(path => !watcher.VnodeWatching(path));
        File.WriteAllText(unwatched, "class C { int slow; }"); // only the directory watcher sees this
        Assert.NotNull(Next(Path.GetFileName(unwatched)));
        Thread.Sleep(200);
        Drain();
        // Any entry change makes the folder be scanned; the file is already known in its new state.
        File.WriteAllText(Path.Combine(_root, "x.tmp"), "x");
        File.Delete(Path.Combine(_root, "x.tmp"));
        Thread.Sleep(300);
        Assert.False(_sets.Any(set => set.Paths.Any(path => path == unwatched)), "the file was reported again by a folder scan");
    }

    [MacOnlyFact]
    public void After_an_atomic_save_a_following_in_place_save_is_fast_again()
    {
        var file = Make("A.cs");
        Watch(_root);
        var latencies = new List<double>();
        for (var i = 0; i < 8; i++)
        {
            Drain();
            File.WriteAllText(file + ".tmp", $"class C {{ int a{i}; }}");
            File.Move(file + ".tmp", file, overwrite: true);
            Assert.NotNull(Next("A.cs"));
            Thread.Sleep(30); // the old inode's delete event and the late directory echo have both been handled
            Drain();
            var start = Stopwatch.GetTimestamp();
            File.WriteAllText(file, $"class C {{ int p{i}; }}");
            var at = Next("A.cs") ?? throw new Xunit.Sdk.XunitException("in-place save not delivered");
            latencies.Add(Stopwatch.GetElapsedTime(start, at).TotalMilliseconds);
        }
        latencies.Sort();
        Assert.True(latencies[latencies.Count / 2] < 8, $"median {latencies[latencies.Count / 2]:F1} ms: the replacement was left unwatched");
    }

    /// <summary>The folder event fires at creation, before the first write; a partial source would show as errors until the rest lands.</summary>
    [MacOnlyFact]
    public void A_new_file_written_in_chunks_is_not_reported_partial()
    {
        Make("A.cs");
        Watch(_root);
        _contents.Clear();
        var path = Path.Combine(_root, "N.cs");
        const string First = "class N { int a; ";
        const string Whole = First + "int b; }";
        using (var stream = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.ReadWrite))
        {
            stream.Write(System.Text.Encoding.UTF8.GetBytes(First));
            stream.Flush();
            Spin(3.5);
            stream.Write(System.Text.Encoding.UTF8.GetBytes(Whole[First.Length..]));
        }
        Assert.NotNull(Next("N.cs"));
        Thread.Sleep(300);
        Assert.NotEmpty(_contents);
        Assert.All(_contents, content => Assert.Equal(Whole, content));
    }

    /// <summary>A checkout adding hundreds of source files must cost the scan thread one settle, not one per file.</summary>
    [MacOnlyFact]
    public void Hundreds_of_new_files_are_reported_after_one_settle_not_one_per_file()
    {
        Make("A.cs");
        var watcher = Watch(_root);
        var names = Enumerable.Range(0, 300).Select(i => $"New{i:D3}.cs").ToArray();
        var seen = new HashSet<string>();
        var start = Stopwatch.GetTimestamp();
        foreach (var name in names) File.WriteAllText(Path.Combine(_root, name), "class C { int x; }");
        var deadline = Stopwatch.GetTimestamp() + 5 * Stopwatch.Frequency;
        while (seen.Count < names.Length && Stopwatch.GetTimestamp() < deadline)
            if (_sets.TryTake(out var set, 20)) foreach (var path in set.Paths) seen.Add(Path.GetFileName(path));
        var elapsed = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
        Assert.True(names.All(seen.Contains), $"{names.Count(seen.Contains)} of {names.Length} reported");
        // One ~3-12 ms settle per scan; a per-file settle (3 ms x 300) would take over 900 ms on the scan thread alone.
        Assert.True(elapsed < 600, $"{elapsed:F0} ms to report 300 new files");
        Assert.True(watcher.VnodeWatching(Path.Combine(_root, names[^1])));
    }

    static void Spin(double ms)
    {
        var until = Stopwatch.GetTimestamp() + (long)(ms * Stopwatch.Frequency / 1000);
        while (Stopwatch.GetTimestamp() < until) Thread.SpinWait(20);
    }

    public void Dispose()
    {
        _watcher?.Dispose();
        Directory.Delete(_root, recursive: true);
    }
}

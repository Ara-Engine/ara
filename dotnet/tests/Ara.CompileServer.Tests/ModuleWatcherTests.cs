using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer.Tests;

/// <summary>A fact that is reported as skipped, not passed, off macOS (the kqueue paths do not exist there).</summary>
sealed class MacOnlyFactAttribute : FactAttribute
{
    public MacOnlyFactAttribute()
    {
        if (!OperatingSystem.IsMacOS()) Skip = "kqueue file watches exist only on macOS";
    }
}

/// <summary>Source saves reach the compile server once each, fast on macOS (kqueue) and correctly for every kind of save.</summary>
public sealed class ModuleWatcherTests : IDisposable
{
    readonly string _directory = InstalledTools.Scratch("module-watcher");
    readonly BlockingCollection<(string[] Paths, long At)> _sets = [];
    readonly ModuleWatcher _watcher;
    readonly ConcurrentQueue<string> _contents = new();

    public ModuleWatcherTests()
    {
        _watcher = new ModuleWatcher((_, paths, _) =>
        {
            // What a compile starting now would read.
            foreach (var path in paths.Where(path => Path.GetFileName(path) == "A.cs" && File.Exists(path)))
                try { _contents.Enqueue(File.ReadAllText(path)); } catch (IOException) { }
            _sets.Add(([.. paths], Stopwatch.GetTimestamp()));
        });
        File.WriteAllText(Path.Combine(_directory, "A.cs"), "class A { }");
        _watcher.Watch(0, _directory);
        Thread.Sleep(300); // the directory watcher starts asynchronously
        Drain();
    }

    void Drain()
    {
        Thread.Sleep(100);
        while (_sets.TryTake(out _)) { }
    }

    /// <summary>Waits for a set containing <paramref name="file"/>; null when none arrives within <paramref name="ms"/>.</summary>
    (string[] Paths, long At)? Next(string file, int ms = 3000)
    {
        var deadline = Stopwatch.GetTimestamp() + ms * Stopwatch.Frequency / 1000;
        while (Stopwatch.GetTimestamp() < deadline)
            if (_sets.TryTake(out var set, 20) && set.Paths.Any(path => Path.GetFileName(path) == file)) return set;
        return null;
    }

    [Fact]
    public void An_in_place_save_is_delivered()
    {
        var start = Stopwatch.GetTimestamp();
        File.WriteAllText(Path.Combine(_directory, "A.cs"), "class A { int x; }");
        Assert.NotNull(Next("A.cs"));
        Assert.True(start > 0);
    }

    [MacOnlyFact]
    public void One_in_place_save_is_delivered_once_on_macOS()
    {
        var start = Stopwatch.GetTimestamp();
        File.WriteAllText(Path.Combine(_directory, "A.cs"), "class A { int x; }");
        Assert.NotNull(Next("A.cs"));
        // The directory watcher reports the same write ~11 ms later; it must not queue a second compile.
        Thread.Sleep(400);
        var extras = new List<string>();
        while (_sets.TryTake(out var extra)) extras.Add(string.Join(",", extra.Paths.Select(Path.GetFileName)) + "@" + Stopwatch.GetElapsedTime(start, extra.At).TotalMilliseconds.ToString("F1"));
        Assert.True(extras.Count == 0, "extra sets: " + string.Join(" | ", extras));
    }

    [MacOnlyFact]
    public void In_place_saves_arrive_faster_than_the_directory_watcher_on_macOS()
    {
        var latencies = new List<double>();
        for (var i = 0; i < 20; i++)
        {
            Drain();
            var start = Stopwatch.GetTimestamp();
            File.WriteAllText(Path.Combine(_directory, "A.cs"), $"class A {{ int x{i}; }}");
            var set = Next("A.cs") ?? throw new Xunit.Sdk.XunitException("save not delivered");
            latencies.Add(Stopwatch.GetElapsedTime(start, set.At).TotalMilliseconds);
        }
        latencies.Sort();
        // FSEvents alone takes ~11 ms (median); a vnode event plus the 1 ms settle is a few ms.
        Assert.True(latencies[latencies.Count / 2] < 8, $"median {latencies[latencies.Count / 2]:F1} ms");
    }

    void AtomicSave(string path, string text)
    {
        var temp = path + ".tmp";
        File.WriteAllText(temp, text);
        File.Move(temp, path, overwrite: true);
    }

    [MacOnlyFact]
    public void One_atomic_save_is_delivered_once_on_macOS()
    {
        var start = Stopwatch.GetTimestamp();
        AtomicSave(Path.Combine(_directory, "A.cs"), "class A { int atomic; }");
        Assert.NotNull(Next("A.cs"));
        // FSEvents reports the rename ~11 ms later; the file is unchanged since the fast delivery, so it is dropped.
        Thread.Sleep(400);
        var extras = new List<string>();
        while (_sets.TryTake(out var extra)) extras.Add(string.Join(",", extra.Paths.Select(Path.GetFileName)) + "@" + Stopwatch.GetElapsedTime(start, extra.At).TotalMilliseconds.ToString("F1"));
        Assert.True(extras.Count == 0, "extra sets: " + string.Join(" | ", extras));
    }

    [MacOnlyFact]
    public void Atomic_saves_arrive_faster_than_the_directory_watcher_on_macOS()
    {
        var latencies = new List<double>();
        for (var i = 0; i < 20; i++)
        {
            Drain();
            var start = Stopwatch.GetTimestamp();
            AtomicSave(Path.Combine(_directory, "A.cs"), $"class A {{ int a{i}; }}");
            var set = Next("A.cs") ?? throw new Xunit.Sdk.XunitException("save not delivered");
            latencies.Add(Stopwatch.GetElapsedTime(start, set.At).TotalMilliseconds);
        }
        latencies.Sort();
        Assert.True(latencies[latencies.Count / 2] < 8, $"median {latencies[latencies.Count / 2]:F1} ms");
    }

    [MacOnlyFact]
    public void A_new_file_is_delivered_once_on_macOS()
    {
        File.WriteAllText(Path.Combine(_directory, "New.cs"), "class New { }");
        Assert.NotNull(Next("New.cs"));
        Thread.Sleep(400);
        Assert.False(_sets.TryTake(out var extra) && extra.Paths.Any(path => Path.GetFileName(path) == "New.cs"));
    }

    [MacOnlyFact]
    public void A_save_in_a_new_subfolder_is_delivered_fast_on_macOS()
    {
        var folder = Path.Combine(_directory, "Sub");
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, "S.cs");
        File.WriteAllText(path, "class S { }");
        Assert.NotNull(Next("S.cs"));
        Thread.Sleep(300); // the folder's own watch starts from its Created event
        Drain();
        var start = Stopwatch.GetTimestamp();
        AtomicSave(path, "class S { int v; }");
        var set = Next("S.cs") ?? throw new Xunit.Sdk.XunitException("save not delivered");
        Assert.True(Stopwatch.GetElapsedTime(start, set.At).TotalMilliseconds < 9, "a save in a folder created after Watch took the slow path");
    }

    /// <summary>A save written in chunks ends with its final state reported, whatever was reported before it.</summary>
    [MacOnlyFact]
    public void A_save_written_in_chunks_ends_with_its_whole_file_reported()
    {
        _contents.Clear();
        var path = Path.Combine(_directory, "A.cs");
        const string First = "class A { int a; int b; ";
        const string Whole = First + "int c; int d; }";
        using (var stream = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.ReadWrite))
        {
            stream.Write(System.Text.Encoding.UTF8.GetBytes(First));
            stream.Flush();
            Thread.Sleep(20);
            stream.Write(System.Text.Encoding.UTF8.GetBytes(Whole[First.Length..]));
        }
        var deadline = Stopwatch.GetTimestamp() + 3 * Stopwatch.Frequency;
        while (!_contents.Contains(Whole) && Stopwatch.GetTimestamp() < deadline) Thread.Sleep(10);
        Assert.Contains(Whole, _contents);
        Thread.Sleep(300);
        Assert.Equal(Whole, _contents.Last());
    }

    /// <summary>A save that ends with a shrinking truncate: the final state must be reported, not dropped as an echo.</summary>
    [MacOnlyFact]
    public void A_save_that_ends_with_a_shrinking_truncate_is_reported_in_its_final_state()
    {
        var path = Path.Combine(_directory, "A.cs");
        File.WriteAllText(path, "class A { int a_long_field_that_makes_the_old_file_longer; }");
        Assert.NotNull(Next("A.cs"));
        Drain();
        _contents.Clear();
        using (var stream = new FileStream(path, FileMode.Open, FileAccess.Write, FileShare.ReadWrite))
        {
            stream.Write("class A { int b; }"u8); // new text over the old bytes: the old tail is still there
            stream.Flush();
            Thread.Sleep(25);
            stream.SetLength(18); // the shrinking truncate
        }
        var deadline = Stopwatch.GetTimestamp() + 3 * Stopwatch.Frequency;
        while (!_contents.Contains("class A { int b; }") && Stopwatch.GetTimestamp() < deadline) Thread.Sleep(10);
        Assert.Contains("class A { int b; }", _contents);
    }

    [Fact]
    public void An_atomic_save_is_delivered_and_the_replacement_is_watched_afterwards()
    {
        var path = Path.Combine(_directory, "A.cs");
        var temp = Path.Combine(_directory, "A.cs.tmp");
        File.WriteAllText(temp, "class A { int replaced; }");
        File.Move(temp, path, overwrite: true);
        Assert.NotNull(Next("A.cs"));
        Drain();
        File.WriteAllText(path, "class A { int edited; }");
        Assert.NotNull(Next("A.cs"));
    }

    [Fact]
    public void A_new_file_is_delivered_and_its_later_saves_too()
    {
        var path = Path.Combine(_directory, "B.cs");
        File.WriteAllText(path, "class B { }");
        Assert.NotNull(Next("B.cs"));
        Drain();
        File.WriteAllText(path, "class B { int x; }");
        Assert.NotNull(Next("B.cs"));
    }

    [Fact]
    public void A_deleted_file_is_delivered()
    {
        File.Delete(Path.Combine(_directory, "A.cs"));
        Assert.NotNull(Next("A.cs"));
    }

    public void Dispose()
    {
        _watcher.Dispose();
        Directory.Delete(_directory, recursive: true);
    }
}

/// <summary>The kqueue file watches stay within their budget and give every descriptor back.</summary>
public sealed class VnodeEventsTests : IDisposable
{
    readonly string _directory = InstalledTools.Scratch("vnode-events");

    string Make(string name)
    {
        var path = Path.Combine(_directory, name);
        File.WriteAllText(path, "x");
        return path;
    }

    [MacOnlyFact]
    public void The_watch_budget_caps_the_descriptors_it_holds_and_releases_them_on_dispose()
    {
        var events = new VnodeEvents((_, _) => { }, maxFiles: 5);
        foreach (var i in Enumerable.Range(0, 20)) events.Add(0, Make($"F{i}.cs"));
        Assert.Equal(5, events.WatchedCount);
        Assert.Equal(5, events.DescriptorCount);
        events.Dispose();
        Assert.Equal(0, events.WatchedCount);
        Assert.Equal(0, events.DescriptorCount);
    }

    [MacOnlyFact]
    public void Deleting_and_replacing_files_does_not_leak_descriptors()
    {
        var seen = new System.Collections.Concurrent.ConcurrentBag<string>();
        using var events = new VnodeEvents((_, path) => seen.Add(path));
        for (var i = 0; i < 40; i++)
        {
            var path = Make("Churn.cs");
            events.Add(0, path);
            if (i % 2 == 0) File.Delete(path);
            else
            {
                File.WriteAllText(path + ".tmp", "y");
                File.Move(path + ".tmp", path, overwrite: true); // an atomic save replaces the watched inode
                File.Delete(path);
            }
            // The watch drops itself when the delete or rename event arrives.
            var deadline = System.Environment.TickCount64 + 2000;
            while (events.Watching(path) && System.Environment.TickCount64 < deadline) Thread.Sleep(2);
            Assert.False(events.Watching(path), $"watch of replaced file kept (iteration {i})");
        }
        Assert.Equal(0, events.WatchedCount);
        Assert.Equal(0, events.DescriptorCount);
    }

    [MacOnlyFact]
    public void A_throwing_callback_does_not_end_the_watcher()
    {
        var calls = 0;
        using var events = new VnodeEvents((_, _) => { Interlocked.Increment(ref calls); throw new InvalidOperationException("boom"); });
        var path = Make("Throw.cs");
        events.Add(0, path);
        File.WriteAllText(path, "y");
        var deadline = System.Environment.TickCount64 + 2000;
        while (Volatile.Read(ref calls) < 1 && System.Environment.TickCount64 < deadline) Thread.Sleep(5);
        File.WriteAllText(path, "z");
        deadline = System.Environment.TickCount64 + 2000;
        while (Volatile.Read(ref calls) < 2 && System.Environment.TickCount64 < deadline) Thread.Sleep(5);
        Assert.True(calls >= 2, "the watcher stopped after the first callback threw");
    }

    public void Dispose() => Directory.Delete(_directory, recursive: true);
}

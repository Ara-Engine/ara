using System.Collections.Concurrent;
using System.Diagnostics;

namespace Ara.CompileServer.Tests;

/// <summary>Source saves reach the compile server once each, fast on macOS (kqueue) and correctly for every kind of save.</summary>
public sealed class ModuleWatcherTests : IDisposable
{
    readonly string _directory = InstalledTools.Scratch("module-watcher");
    readonly BlockingCollection<(string[] Paths, long At)> _sets = [];
    readonly ModuleWatcher _watcher;

    public ModuleWatcherTests()
    {
        _watcher = new ModuleWatcher((_, paths, _) => _sets.Add(([.. paths], Stopwatch.GetTimestamp())));
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

    [Fact]
    public void One_in_place_save_is_delivered_once_on_macOS()
    {
        if (!OperatingSystem.IsMacOS()) return;
        var start = Stopwatch.GetTimestamp();
        File.WriteAllText(Path.Combine(_directory, "A.cs"), "class A { int x; }");
        Assert.NotNull(Next("A.cs"));
        // The directory watcher reports the same write ~11 ms later; it must not queue a second compile.
        Thread.Sleep(400);
        var extras = new List<string>();
        while (_sets.TryTake(out var extra)) extras.Add(string.Join(",", extra.Paths.Select(Path.GetFileName)) + "@" + Stopwatch.GetElapsedTime(start, extra.At).TotalMilliseconds.ToString("F1"));
        Assert.True(extras.Count == 0, "extra sets: " + string.Join(" | ", extras));
    }

    [Fact]
    public void In_place_saves_arrive_faster_than_the_directory_watcher_on_macOS()
    {
        if (!OperatingSystem.IsMacOS()) return;
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

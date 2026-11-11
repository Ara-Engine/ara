using System.Diagnostics;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>The first compile no longer waits for the file watchers; whatever happens to the sources meanwhile must still be compiled.</summary>
public sealed class WatchStartupTests : IDisposable
{
    const string Game = """
        namespace Game;

        /// <summary>A counter.</summary>
        [Component] public partial struct Count { /// <summary>Ticks.</summary>
            public int Value; }

        public static partial class Counting
        {
            [Query]
            static void Run(ref Count c)
            {
                c.Value += 1;
            }
        }
        """;

    const string Extra = "namespace Game; public static class Extra { public static int Two() => 2; }";

    readonly Modules _modules = new();
    readonly string _dir;
    readonly string _path;
    readonly string[] _assemblies = [typeof(Behaviour).Assembly.Location];

    /// <summary>A hot module of two files, compiled once with no watcher (the state before the watchers start).</summary>
    public WatchStartupTests()
    {
        _dir = Path.Combine(_modules.Root, "Game");
        Directory.CreateDirectory(_dir);
        _path = Path.Combine(_dir, "Code.cs");
        File.WriteAllText(_path, Game);
        File.WriteAllText(Path.Combine(_dir, "Extra.cs"), Extra);
        _modules.Server.Watch(0, "Game", _dir, true, _assemblies, [], "gameplay", watch: false);
        _modules.Idle();
        Assert.Equal(MessageKind.Reload, _modules.Sink.Next().Kind);
    }

    bool Compiled(Func<IEnumerable<string>, bool> condition)
    {
        var deadline = Stopwatch.GetTimestamp() + 30 * Stopwatch.Frequency;
        while (Stopwatch.GetTimestamp() < deadline)
        {
            if (_modules.Server.Dependency("Game") is { } state && condition(state.Texts.Values)) return true;
            Thread.Sleep(50);
        }
        return false;
    }

    [Fact]
    public void A_save_made_before_the_watcher_goes_live_is_compiled_once_it_does()
    {
        File.WriteAllText(_path, Game.Replace("c.Value += 1;", "c.Value += 7;", StringComparison.Ordinal));
        _modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.True(Compiled(texts => texts.Any(text => text.Contains("c.Value += 7;", StringComparison.Ordinal))), "the save was never compiled");
    }

    /// <summary>NTFS and some Linux file systems stamp writes from a coarse clock, so a save can carry a time before the request.</summary>
    [Fact]
    public void A_save_whose_file_time_predates_the_request_is_still_found()
    {
        File.WriteAllText(_path, Game.Replace("c.Value += 1;", "c.Value += 8;", StringComparison.Ordinal));
        File.SetLastWriteTimeUtc(_path, DateTime.UtcNow.AddHours(-1));
        _modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.True(Compiled(texts => texts.Any(text => text.Contains("c.Value += 8;", StringComparison.Ordinal))), "the save was never compiled");
    }

    [Fact]
    public void A_file_deleted_before_the_watcher_goes_live_is_found()
    {
        File.Delete(Path.Combine(_dir, "Extra.cs"));
        _modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.True(Compiled(texts => !texts.Any(text => text.Contains("class Extra", StringComparison.Ordinal))), "the deleted file stayed compiled");
    }

    /// <summary>A rename or a copy that keeps the time (mv, git mv, cp -p) leaves a file with an old modification time.</summary>
    [Fact]
    public void A_file_moved_in_with_an_old_time_is_found()
    {
        var outside = Path.Combine(_modules.Root, "Moved.cs");
        File.WriteAllText(outside, "namespace Game; public static class Moved { public static int Three() => 3; }");
        File.SetLastWriteTimeUtc(outside, DateTime.UtcNow.AddDays(-3));
        File.Move(outside, Path.Combine(_dir, "Moved.cs"));
        _modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.True(Compiled(texts => texts.Any(text => text.Contains("class Moved", StringComparison.Ordinal))), "the moved-in file was never compiled");
    }

    [Fact]
    public void When_nothing_changed_the_verification_compiles_and_reports_nothing()
    {
        _modules.Server.StartWatching(0, _assemblies, watch: true);
        _modules.Idle();
        Thread.Sleep(300); // the start runs on its own thread; its verification is queued once the watcher is live
        _modules.Idle();
        Assert.True(_modules.Sink.Empty, "an unchanged module sent messages");
    }

    [Fact]
    public void A_start_that_fails_is_contained_not_thrown()
    {
        Directory.Delete(_dir, recursive: true); // the module folder vanished between the request and the start
        _modules.Server.StartWatching(0, _assemblies, watch: true);
    }

    /// <summary>Throws on the first <paramref name="failures"/> calls, then creates the real watcher.</summary>
    static Func<string, string, FileSystemWatcher> FailingFactory(int failures, Action? onCall = null)
    {
        var calls = 0;
        return (directory, file) =>
        {
            onCall?.Invoke();
            if (Interlocked.Increment(ref calls) <= failures) throw new IOException("no inotify instances left (injected)");
            return new FileSystemWatcher(directory, file);
        };
    }

    /// <summary>A fresh coordinator whose failures are injected before <c>Watch</c>, so the real background start meets them.</summary>
    Modules Fresh(Action<Coordinator> inject)
    {
        var modules = new Modules();
        inject(modules.Server);
        var dir = Path.Combine(modules.Root, "Game");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "Code.cs"), Game);
        modules.Server.Watch(0, "Game", dir, true, _assemblies, [], "gameplay", watch: true);
        modules.Server.WaitForWatchStarts();
        return modules;
    }

    [Fact]
    public void A_reference_watcher_that_fails_once_is_retried_and_the_start_ends_with_one_of_each()
    {
        using var modules = Fresh(server => server.ReferenceWatcherFactory = FailingFactory(1));
        Assert.Equal(1, modules.Server.ModuleWatcherCount);   // not watched twice by the retry
        Assert.Equal(1, modules.Server.ReferenceWatcherCount); // the retry created it (the failure did not record it as watched)
        Assert.Single(modules.Sink.Logs, log => log.Contains("retrying", StringComparison.Ordinal));
        // A later start for the same module changes nothing.
        modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.Equal(1, modules.Server.ModuleWatcherCount);
        Assert.Equal(1, modules.Server.ReferenceWatcherCount);
    }

    [Fact]
    public void A_reference_watcher_that_fails_twice_is_logged_and_can_still_be_started_later()
    {
        using var modules = Fresh(server => server.ReferenceWatcherFactory = FailingFactory(2));
        Assert.Equal(1, modules.Server.ModuleWatcherCount);
        Assert.Equal(0, modules.Server.ReferenceWatcherCount);
        Assert.Contains(modules.Sink.Logs, log => log.Contains("will not reload until the project reopens", StringComparison.Ordinal));
        // The failures did not poison the set: the next start creates the watcher, and still only one folder watcher exists.
        modules.Server.StartWatching(0, _assemblies, watch: true);
        Assert.Equal(1, modules.Server.ModuleWatcherCount);
        Assert.Equal(1, modules.Server.ReferenceWatcherCount);
    }

    [Fact]
    public void A_folder_watcher_that_fails_once_is_retried_without_watching_the_folder_twice()
    {
        var calls = 0;
        using var modules = Fresh(server => server.ModuleWatchStart = (watcher, slot, directory) =>
        {
            if (Interlocked.Increment(ref calls) == 1) throw new IOException("injected");
            watcher.Watch(slot, directory);
        });
        Assert.Equal(2, calls);
        Assert.Equal(1, modules.Server.ModuleWatcherCount);
        Assert.Equal(1, modules.Server.ReferenceWatcherCount);
    }

    /// <summary>A first compile that failed (syntax error) committed no state; the sources it read are what it compiled.</summary>
    [Fact]
    public void A_fix_saved_while_the_watcher_starts_is_found_when_the_first_compile_had_failed()
    {
        using var modules = new Modules();
        var dir = Path.Combine(modules.Root, "Broken");
        Directory.CreateDirectory(dir);
        var path = Path.Combine(dir, "Code.cs");
        File.WriteAllText(path, Game.Replace("c.Value += 1;", "c.Value += ;", StringComparison.Ordinal));
        modules.Server.Watch(0, "Broken", dir, true, _assemblies, [], "gameplay", watch: false);
        modules.Idle();
        Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
        Assert.Null(modules.Server.Dependency("Broken")); // nothing committed
        File.WriteAllText(path, Game.Replace("c.Value += 1;", "c.Value += 5;", StringComparison.Ordinal));
        modules.Server.StartWatching(0, _assemblies, watch: true);
        var deadline = Stopwatch.GetTimestamp() + 30 * Stopwatch.Frequency;
        while (modules.Server.Dependency("Broken") is not { } state || !state.Texts.Values.Any(text => text.Contains("c.Value += 5;", StringComparison.Ordinal)))
        {
            Assert.True(Stopwatch.GetTimestamp() < deadline, "the fix was never compiled");
            Thread.Sleep(50);
        }
    }

    [Fact]
    public void Disposing_while_the_watchers_start_does_not_hang_or_throw()
    {
        var modules = new Modules();
        var dir = Path.Combine(modules.Root, "Game");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "Code.cs"), Game);
        modules.Server.Watch(0, "Game", dir, true, _assemblies, [], "gameplay", watch: true);
        modules.Dispose();
    }

    public void Dispose() => _modules.Dispose();
}

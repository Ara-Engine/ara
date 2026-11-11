using System.Diagnostics;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>The first compile no longer waits for the file watchers; a save made meanwhile must still be compiled.</summary>
public sealed class WatchStartupTests
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

    [Fact]
    public void A_save_made_before_the_watcher_goes_live_is_compiled_once_it_does()
    {
        using var modules = new Modules();
        var dir = Path.Combine(modules.Root, "Game");
        Directory.CreateDirectory(dir);
        var path = Path.Combine(dir, "Code.cs");
        File.WriteAllText(path, Game);
        var assemblies = new[] { typeof(Behaviour).Assembly.Location };
        // No watcher yet, and the first compile has read the original text.
        var requested = DateTime.UtcNow.AddSeconds(-1);
        modules.Server.Watch(0, "Game", dir, true, assemblies, [], "gameplay", watch: false);
        modules.Idle();
        File.WriteAllText(path, Game.Replace("c.Value += 1;", "c.Value += 7;", StringComparison.Ordinal));
        // The watcher starts: the save predates it, so it must be compiled as a change.
        modules.Server.StartWatching(0, assemblies, watch: true, requested);
        var deadline = Stopwatch.GetTimestamp() + 30 * Stopwatch.Frequency;
        while (!(modules.Server.Dependency("Game") is { } state && state.Texts.Values.Any(text => text.Contains("c.Value += 7;", StringComparison.Ordinal))))
        {
            Assert.True(Stopwatch.GetTimestamp() < deadline, "the save was never compiled");
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
        modules.Server.Watch(0, "Game", dir, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: true);
        modules.Dispose();
    }
}

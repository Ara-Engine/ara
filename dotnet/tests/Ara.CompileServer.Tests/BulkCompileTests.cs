using System.Diagnostics;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class BulkCompileTests
{
    [Fact]
    public void Same_module_source_additions_and_removals_reload_without_a_player_restart()
    {
        var root = InstalledTools.Scratch("same-module-membership");
        var authored = Path.Combine(root, "content");
        var state = Path.Combine(root, ".ara", "scripts", "game");
        Directory.CreateDirectory(authored);
        Directory.CreateDirectory(state);
        var original = Path.Combine(authored, "Original.cs");
        File.WriteAllText(original, "public static class Original { public static int Value => 1; }");
        File.WriteAllText(Path.Combine(state, "module.inputs.json"), System.Text.Json.JsonSerializer.Serialize(new { project = root, authoringRoot = "content", name = "game" }));
        var capture = new Capture();
        using var coordinator = new Coordinator(capture, new AnalyzerSet());
        coordinator.Hello(false, "");
        coordinator.Watch(0, "game", state, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        Assert.Equal(AbiConst.ReloadInitial, capture.Next().Reload().Single().Kind);
        var added = Path.Combine(authored, "Added.cs");
        File.WriteAllText(added, "public static class Added { public static int Value => Original.Value; }");
        coordinator.Change(0, [added], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.ReloadL3, capture.Next().Reload().Single().Kind);
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        File.Delete(added);
        coordinator.Change(0, [added], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.ReloadL3, capture.Next().Reload().Single().Kind);
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.True(capture.Empty);
    }

    [Fact]
    public void A_cross_module_rename_compiles_the_dependents_current_sources_in_one_atomic_cascade()
    {
        using var modules = new Modules();
        const string core = "namespace Core; public static class Rules { public static int Heal(int value) => value + 1; }";
        const string game = "namespace Game; public static class Rules { public static int Run(int value) => Core.Rules.Heal(value); }";
        modules.Add(0, "Core", core);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Add(1, "Game", game, references: ["Core"]);
        modules.Idle();
        modules.Sink.Next().Reload();
        // Both files have been saved before the root module's notification. The
        // dependent must use its new source, even if its watcher notice arrives later.
        File.WriteAllText(modules.File("Core", "Code.cs"), core.Replace("Heal", "Restore", StringComparison.Ordinal));
        File.WriteAllText(modules.File("Game", "Code.cs"), game.Replace("Heal", "Restore", StringComparison.Ordinal));
        modules.Server.Change(0, [modules.File("Core", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        var batch = modules.Sink.Next().Reload();
        Assert.Equal(new uint[] { 0, 1 }, batch.Select(e => e.Slot));
        Assert.All(batch, entry => Assert.NotEmpty(entry.Pe));
        Assert.Equal(Escalation.Dependency, batch[1].Escalation);
        modules.Server.Change(1, [modules.File("Game", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        Assert.True(modules.Sink.Empty, "the delayed watcher echo must not create another generation");
    }

    [Fact]
    public void Broken_current_dependent_sources_keep_the_entire_cascade_at_last_good()
    {
        using var modules = new Modules();
        const string core = "namespace Core; public static class Rules { public static int Number() => 1; }";
        const string game = "namespace Game; public static class Rules { public static int Number() => Core.Rules.Number(); }";
        modules.Add(0, "Core", core);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Add(1, "Game", game, references: ["Core"]);
        modules.Idle();
        modules.Sink.Next().Reload();
        File.WriteAllText(modules.File("Core", "Code.cs"), core.Replace("int Number()", "long Number()", StringComparison.Ordinal));
        File.WriteAllText(modules.File("Game", "Code.cs"), "this is broken C#");
        modules.Server.Change(0, [modules.File("Core", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
        Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
        Assert.True(modules.Sink.Empty);
        File.WriteAllText(modules.File("Game", "Code.cs"), game.Replace("int Number()", "long Number()", StringComparison.Ordinal));
        modules.Server.Change(0, [modules.File("Core", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        var fixedBatch = modules.Sink.Next().Reload();
        Assert.All(fixedBatch, entry => Assert.Equal(1u, entry.Generation));
    }
}

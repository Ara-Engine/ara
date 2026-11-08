using System.Diagnostics;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class BulkCompileTests
{
    const string Alpha = "namespace Alpha; public static class Rules { public static int Old() => 1; }";
    const string Beta = "namespace Beta; public static class Rules { public static int Old() => 2; }";
    const string Combined = "namespace Combined; public static class Rules { public static int Run() => Alpha.Rules.Old() + Beta.Rules.Old(); }";

    static void StartIndependentRoots(Modules modules, string[]? sharedReferences = null)
    {
        modules.Add(0, "Alpha", Alpha, references: sharedReferences);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Add(1, "Beta", Beta, references: sharedReferences);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Add(2, "Combined", Combined, references: ["Alpha", "Beta"]);
        modules.Idle();
        modules.Sink.Next().Reload();
    }

    static void SaveIndependentRefactor(Modules modules, bool broken = false)
    {
        File.WriteAllText(modules.File("Alpha", "Code.cs"), Alpha.Replace("Old", "New", StringComparison.Ordinal));
        File.WriteAllText(modules.File("Beta", "Code.cs"), Beta.Replace("Old", "New", StringComparison.Ordinal));
        File.WriteAllText(modules.File("Combined", "Code.cs"), broken ? "this is broken C#" : Combined.Replace("Old", "New", StringComparison.Ordinal));
    }

    [Theory]
    [InlineData(0u)]
    [InlineData(1u)]
    [InlineData(2u)]
    public void Independent_dirty_roots_and_their_shared_dependent_apply_as_one_edit_wave(uint first)
    {
        using var modules = new Modules();
        StartIndependentRoots(modules);
        SaveIndependentRefactor(modules);
        var names = new[] { "Alpha", "Beta", "Combined" };
        modules.Server.Change(first, [modules.File(names[first], "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        var batch = modules.Sink.Next().Reload();
        Assert.Equal(new uint[] { 0, 1, 2 }, batch.Select(entry => entry.Slot));
        Assert.All(batch, entry => { Assert.Equal(1u, entry.Generation); Assert.NotEmpty(entry.Pe); });
        for (uint slot = 0; slot < names.Length; slot++)
            modules.Server.Change(slot, [modules.File(names[slot], "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        Assert.True(modules.Sink.Empty, "late notifications for the same bytes must not activate another generation");
    }

    [Theory]
    [InlineData(0u)]
    [InlineData(2u)]
    public void Invalid_edit_waves_preserve_all_last_good_generations_and_repair_atomically(uint first)
    {
        using var modules = new Modules();
        StartIndependentRoots(modules);
        SaveIndependentRefactor(modules, broken: true);
        var names = new[] { "Alpha", "Beta", "Combined" };
        modules.Server.Change(first, [modules.File(names[first], "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        foreach (var name in names)
        {
            Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
            Assert.Equal(0u, modules.Server.Dependency(name)!.Generation);
        }
        Assert.True(modules.Sink.Empty);
        File.WriteAllText(modules.File("Combined", "Code.cs"), Combined.Replace("Old", "New", StringComparison.Ordinal));
        modules.Server.Change(2, [modules.File("Combined", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        modules.Idle();
        var batch = modules.Sink.Next().Reload();
        Assert.Equal(new uint[] { 0, 1, 2 }, batch.Select(entry => entry.Slot));
        Assert.All(batch, entry => Assert.Equal(1u, entry.Generation));
        Assert.True(modules.Sink.Empty);
    }

    [Theory]
    [InlineData(false, false)]
    [InlineData(false, true)]
    [InlineData(true, false)]
    [InlineData(true, true)]
    public void Edit_waves_do_not_activate_or_get_blocked_by_disconnected_agent_edits(bool broken, bool sharedAncestor)
    {
        using var modules = new Modules();
        if (sharedAncestor)
        {
            modules.Add(4, "Shared", "namespace Shared; public static class Rules { public static int Value() => 1; }");
            modules.Idle();
            modules.Sink.Next().Reload();
        }
        StartIndependentRoots(modules, sharedAncestor ? ["Shared"] : null);
        const string unrelated = "namespace Gamma; public static class Rules { public static int Value() => 4; }";
        modules.Add(3, "Gamma", unrelated, references: sharedAncestor ? ["Shared"] : null);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Server.AgentQuiet = TimeSpan.FromDays(1);
        File.WriteAllText(modules.File("Gamma", "Code.cs"), broken ? "this is broken C#" : unrelated.Replace("=> 4", "=> 9", StringComparison.Ordinal));
        modules.Server.Change(3, [modules.File("Gamma", "Code.cs")], AbiConst.OriginAgent, Stopwatch.GetTimestamp());
        SaveIndependentRefactor(modules);
        modules.Server.Change(0, [modules.File("Alpha", "Code.cs")], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        // Gamma intentionally stays queued: the related human refactor must finish
        // without compiling or activating those quiet-period agent bytes.
        var batch = modules.Sink.Next().Reload();
        Assert.Equal(new uint[] { 0, 1, 2 }, batch.Select(entry => entry.Slot));
        Assert.All(batch, entry => Assert.Equal(1u, entry.Generation));
        var gamma = modules.Server.Dependency("Gamma")!;
        Assert.Equal(0u, gamma.Generation);
        Assert.Equal(unrelated, gamma.Texts[modules.File("Gamma", "Code.cs")]);
        Assert.True(modules.Sink.Empty);
    }

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

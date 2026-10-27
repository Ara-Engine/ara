using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class SessionTests : IDisposable
{
    readonly Modules _m = new();

    public void Dispose() => _m.Dispose();

    [Fact]
    public void Source_reads_allow_an_editor_to_keep_its_write_handle_open()
    {
        var path = Path.Combine(_m.Root, "Shared.cs");
        File.WriteAllText(path, "// shared source\n");
        using var editor = new FileStream(path, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite | FileShare.Delete);
        Assert.Equal("// shared source\n", ModuleSession.ReadWithRetry(path));
    }

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
            // @insert
        }
        """;

    [Fact]
    public void Cold_module_goes_hot_then_takes_l1_and_l2()
    {
        _m.Add(0, "Game", Game, hot: false);
        _m.Idle();
        var first = _m.Sink.Next().Reload().Single();
        Assert.Equal((0u, 0u, false, AbiConst.ReloadInitial), (first.Slot, first.Generation, first.Hot, first.Kind));

        _m.Edit(0, "Game", "c.Value += 1;", "c.Value += 2;");
        var l3 = _m.Sink.Next().Reload().Single();
        Assert.Equal((1u, true, AbiConst.ReloadL3, Escalation.Cold), (l3.Generation, l3.Hot, l3.Kind, l3.Escalation));

        _m.Edit(0, "Game", "c.Value += 2;", "c.Value += 3;");
        Assert.Equal((0u, 1u, AbiConst.ReloadL1, 1u, AbiConst.OriginHuman), _m.Sink.Next().Delta());

        // A new helper and the body that calls it: one L2 delta.
        _m.Edit(0, "Game", "c.Value += 3;", "c.Value += Bonus();");
        Assert.Equal(MessageKind.Diagnostics, _m.Sink.Next().Kind); // Bonus does not exist yet
        _m.Edit(0, "Game", "// @insert", "static int Bonus() => 4;");
        Assert.Equal((0u, 1u, AbiConst.ReloadL2, 2u, AbiConst.OriginHuman), _m.Sink.Next().Delta());
        Assert.True(_m.Sink.Empty);
    }

    [Fact]
    public void Errors_keep_the_last_good_state()
    {
        _m.Add(0, "Game", Game);
        _m.Idle();
        Assert.Equal(MessageKind.Reload, _m.Sink.Next().Kind);

        _m.Edit(0, "Game", "c.Value += 1;", "c.Value += ;");
        var (_, errors, json) = _m.Sink.Next().Diagnostics();
        Assert.True(errors > 0);
        Assert.Contains("\"code\":\"ARA0900\"", json);
        Assert.Contains("Code.cs", json);

        // The fix is diffed against the last good code: a body-only edit, so L1.
        _m.Edit(0, "Game", "c.Value += ;", "c.Value += 5;");
        Assert.Equal(AbiConst.ReloadL1, _m.Sink.Next().Delta().Kind);

        // A new component field is structural: L3.
        _m.Edit(0, "Game", "public int Value; }", "public int Value; /// <summary>More.</summary>\n public int More; }");
        Assert.Equal(Escalation.Structural, _m.Sink.Next().Reload().Single().Escalation);

        // A doc-comment edit changes the generated schema docs: L3.
        _m.Edit(0, "Game", "/// <summary>Ticks.</summary>", "/// <summary>Ticks seen.</summary>");
        Assert.Equal(Escalation.Generated, _m.Sink.Next().Reload().Single().Escalation);

        // A new system is additive but changes the generator's output (registration): L3.
        _m.Edit(0, "Game", "// @insert", "[Query] static void Twice(ref Count c) { c.Value += 1; }\n    // @insert");
        Assert.Equal(Escalation.Generated, _m.Sink.Next().Reload().Single().Escalation);

        // Analyzer errors block a reload like compile errors.
        _m.Edit(0, "Game", "c.Value += 5;", "c.Value += 5; Hits++; } static int Hits; static void X() {");
        var (_, analyzerErrors, analyzerJson) = _m.Sink.Next().Diagnostics();
        Assert.True(analyzerErrors > 0);
        Assert.Contains("ARA0100", analyzerJson);
    }

    [Fact]
    public void Unknown_module_references_are_refused()
    {
        _m.Add(0, "Game", Game, references: ["Missing"]);
        var (slot, errors, json) = _m.Sink.Next().Diagnostics();
        Assert.Equal((0u, 1u), (slot, errors));
        Assert.Contains("ARA0924", json);
    }

    [Fact]
    public void Engine_binding_changes_require_a_restart()
    {
        var runtime = Path.Combine(_m.Root, "Ara.Runtime.dll");
        File.Copy(typeof(Behaviour).Assembly.Location, runtime);
        _m.Add(0, "Game", Game, runtime: runtime);
        _m.Idle();
        Assert.Equal(MessageKind.Reload, _m.Sink.Next().Kind);
        File.Copy(typeof(Ara.Generators.ModuleGenerator).Assembly.Location, runtime, overwrite: true); // a different assembly
        var restart = _m.Sink.Next();
        Assert.Equal(MessageKind.Restart, restart.Kind);
        Assert.Equal(uint.MaxValue, new Protocol.Reader(restart.Payload).U32());
    }
}

/// <summary>Module B (Game) references module A (Core): edits to A cascade to B.</summary>
public sealed class CascadeTests : IDisposable
{
    readonly Modules _m = new();

    public void Dispose() => _m.Dispose();

    const string Core = """
        namespace Core;

        /// <summary>Health shared by every module.</summary>
        [Component] public partial struct Health { /// <summary>Points.</summary>
            public int Value; }

        public static class Rules
        {
            public static int Heal(int hp) { return hp + 10; }
            public static int Unused(int x) => x;
            // @insert
        }
        """;

    const string Game = """
        namespace Game;

        public static partial class Regen
        {
            [Query]
            static void Run(ref Core.Health h)
            {
                h.Value = Core.Rules.Heal(h.Value);
            }
        }
        """;

    void Start()
    {
        _m.Add(0, "Core", Core);
        _m.Add(1, "Game", Game, references: ["Core"]);
        _m.Idle();
        Assert.Equal(0u, _m.Sink.Next().Reload().Single().Slot);
        Assert.Equal(1u, _m.Sink.Next().Reload().Single().Slot);
    }

    [Fact]
    public void Body_edits_do_not_cascade()
    {
        Start();
        _m.Edit(0, "Core", "hp + 10", "hp + 20");
        var d = _m.Sink.Next().Delta();
        Assert.Equal((0u, AbiConst.ReloadL1), (d.Slot, d.Kind));
        Assert.True(_m.Sink.Empty, "Game is neither relinked nor rebuilt");
    }

    [Fact]
    public void Surface_preserving_l3_relinks_and_surface_changes_rebuild()
    {
        Start();
        // A closure edit is L3 but keeps Core's public surface: Game relinks (no image, no compile).
        _m.Edit(0, "Core", "return hp + 10;", "System.Func<int> f = () => hp + 10; return f();");
        var relink = _m.Sink.Next().Reload();
        Assert.Equal(2, relink.Length);
        Assert.Equal((0u, Escalation.Closure), (relink[0].Slot, relink[0].Escalation));
        Assert.Equal((1u, 1u, Escalation.Relink, 0), (relink[1].Slot, relink[1].Generation, relink[1].Escalation, relink[1].Pe.Length));

        // A signature change moves the surface: Game is rebuilt against it.
        _m.Edit(0, "Core", "public static int Unused(int x)", "public static long Unused(long x)");
        var rebuild = _m.Sink.Next().Reload();
        Assert.Equal((0u, Escalation.Structural), (rebuild[0].Slot, rebuild[0].Escalation));
        Assert.Equal((1u, 2u, Escalation.Dependency), (rebuild[1].Slot, rebuild[1].Generation, rebuild[1].Escalation));
        Assert.NotEmpty(rebuild[1].Pe);
    }

    [Fact]
    public void A_change_that_breaks_a_dependent_applies_nowhere()
    {
        Start();
        _m.Edit(0, "Core", "public static int Heal(int hp)", "public static int Restore(int hp)");
        var game = _m.Sink.Next().Diagnostics();
        var core = _m.Sink.Next().Diagnostics();
        Assert.Equal((1u, 0u), (game.Slot, core.Slot));
        Assert.True(game.Errors > 0 && core.Errors > 0);
        Assert.Contains("ARA0925", game.Json);
        Assert.Contains("ARA0925", core.Json);
        Assert.True(_m.Sink.Empty, "nothing applies");

        // Reverting is diffed against the last good Core: nothing changed, so nothing is sent.
        _m.Edit(0, "Core", "public static int Restore(int hp)", "public static int Heal(int hp)");
        Assert.True(_m.Sink.Empty);
    }

    [Fact]
    public void Dependents_see_members_added_by_l2()
    {
        Start();
        _m.Edit(0, "Core", "// @insert", "public static int Bonus() => 7;");
        var added = _m.Sink.Next().Delta();
        Assert.Equal((0u, AbiConst.ReloadL2), (added.Slot, added.Kind));
        Assert.True(_m.Sink.Empty, "an additive edit does not cascade");
        _m.Edit(1, "Game", "Core.Rules.Heal(h.Value)", "Core.Rules.Heal(h.Value) + Core.Rules.Bonus()");
        var d = _m.Sink.Next().Delta();
        Assert.Equal((1u, AbiConst.ReloadL1), (d.Slot, d.Kind));
    }
}

public sealed class SchedulingTests : IDisposable
{
    readonly Modules _m = new();

    public void Dispose() => _m.Dispose();

    const string Three = """
        namespace X;

        public static class Work
        {
            public static int A1() { return 1; }
            public static int A2() { return 2; }
            public static int A3() { return 3; }
        }
        """;

    [Fact]
    public void Agent_edits_are_coalesced_and_run_behind_human_ones()
    {
        _m.Server.AgentQuiet = TimeSpan.FromMilliseconds(400);
        _m.Add(0, "Agent", Three);
        _m.Add(1, "Human", Three);
        _m.Idle();
        _m.Sink.Next();
        _m.Sink.Next();

        _m.Edit(0, "Agent", "return 1;", "return 10;", AbiConst.OriginAgent, wait: false);
        _m.Edit(0, "Agent", "return 2;", "return 20;", AbiConst.OriginAgent, wait: false);
        _m.Edit(0, "Agent", "return 3;", "return 30;", AbiConst.OriginAgent, wait: false);
        _m.Edit(1, "Human", "return 1;", "return 11;", AbiConst.OriginHuman, wait: false);
        _m.Idle();

        var human = _m.Sink.Next().Delta();
        Assert.Equal((1u, AbiConst.OriginHuman), (human.Slot, human.Origin));
        var agent = _m.Sink.Next().Delta();
        Assert.Equal((0u, 3u, AbiConst.OriginAgent), (agent.Slot, agent.Changed, agent.Origin)); // one compile for three edits
        Assert.True(_m.Sink.Empty);
    }

    [Fact]
    public void A_human_save_absorbs_pending_agent_edits_of_the_module()
    {
        _m.Server.AgentQuiet = TimeSpan.FromSeconds(5);
        _m.Add(0, "Shared", Three);
        _m.Idle();
        _m.Sink.Next();
        _m.Edit(0, "Shared", "return 1;", "return 10;", AbiConst.OriginAgent, wait: false);
        _m.Edit(0, "Shared", "return 2;", "return 20;", AbiConst.OriginHuman);
        var d = _m.Sink.Next().Delta();
        Assert.Equal((2u, AbiConst.OriginHuman), (d.Changed, d.Origin));
        Assert.True(_m.Sink.Empty);
    }
}

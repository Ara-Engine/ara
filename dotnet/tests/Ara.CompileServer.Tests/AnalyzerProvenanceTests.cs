using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

public sealed class AnalyzerProvenanceTests
{
    [Fact]
    public void An_accepted_warning_revokes_trust_and_remains_visible_after_a_sibling_edit()
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        var warning = AnalysisModule.First.Replace("p.Value += 1;", "p.Value += new int[2].Length;", StringComparison.Ordinal);
        fixture.Write(0, warning);
        var first = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Delta, first.Kind);
        Assert.Equal("ARA0300", Assert.Single(first.Problems).Id);
        Assert.False(first.Next!.AnalyzerClean);
        fixture.Module.Commit(first);
        fixture.Write(0, warning.Replace("p.Value += 3;", "p.Value += 4;", StringComparison.Ordinal));
        var sibling = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Delta, sibling.Kind);
        Assert.Equal(AnalyzerBodyTests.Exact(first.Problems), AnalyzerBodyTests.Exact(sibling.Problems));
        Assert.False(sibling.Next!.AnalyzerClean);
        fixture.Module.Commit(sibling);
        fixture.Write(0, AnalysisModule.First);
        var repair = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Delta, repair.Kind);
        Assert.Empty(repair.Problems);
        Assert.False(repair.Next!.AnalyzerClean);
        fixture.Module.Commit(repair);
        var rebuild = fixture.Module.Full(Escalation.Requested, null, fixture);
        Assert.True(rebuild.Next!.AnalyzerClean);
        fixture.Module.Commit(rebuild);
        Assert.True(fixture.Module.State!.AnalyzerClean);
    }

    [Fact]
    public void An_empty_changed_file_does_not_prove_an_untouched_partial_file_clean()
    {
        using var fixture = new AnalysisModule();
        fixture.Write(1, AnalysisModule.Second.Replace("p.Value += 2;", "p.Value += new int[2].Length;", StringComparison.Ordinal));
        fixture.Start();
        Assert.False(fixture.Module.State!.AnalyzerClean);
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += 4;", StringComparison.Ordinal));
        var other = fixture.Compile(0);
        Assert.Empty(other.Problems);
        Assert.False(other.Next!.AnalyzerClean);
        fixture.Module.Commit(other);
        fixture.Write(1, AnalysisModule.Second);
        var repair = fixture.Compile(1);
        Assert.False(repair.Next!.AnalyzerClean);
        fixture.Module.Commit(repair);
        var noop = fixture.Compile(0);
        Assert.Equal(OutcomeKind.None, noop.Kind);
        Assert.False(noop.Next!.AnalyzerClean);
        fixture.Module.Commit(noop);
        // A real module-wide trivia check can re-establish the proof.
        fixture.Write(1, AnalysisModule.Second + "\n// saved comment\n");
        var trivia = fixture.Compile(1);
        Assert.Equal(OutcomeKind.None, trivia.Kind);
        Assert.True(trivia.Next!.AnalyzerClean);
    }

    [Fact]
    public void Failed_multifile_edits_keep_last_good_proof_and_repair_rechecks_both_pending_bodies()
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        var good = fixture.Module.State;
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += new int[2].Length;", StringComparison.Ordinal));
        fixture.Write(1, AnalysisModule.Second.Replace("p.Value += 2;", "p.Value += DateTime.UtcNow.Second;", StringComparison.Ordinal));
        var rejected = fixture.Compile(0, 1);
        Assert.Equal(OutcomeKind.Errors, rejected.Kind);
        Assert.Contains(rejected.Problems, diagnostic => diagnostic.Id == "ARA0200");
        Assert.Contains(rejected.Problems, diagnostic => diagnostic.Id == "ARA0300");
        Assert.Same(good, fixture.Module.State);
        Assert.True(good!.AnalyzerClean);
        fixture.Write(1, AnalysisModule.Second);
        var repair = fixture.Compile(1);
        Assert.Equal(OutcomeKind.Delta, repair.Kind);
        Assert.Equal("ARA0300", Assert.Single(repair.Problems).Id);
        Assert.False(repair.Next!.AnalyzerClean);
        fixture.Module.Commit(repair);
        var noop = fixture.Compile(0, 1);
        Assert.Equal(OutcomeKind.None, noop.Kind);
        Assert.False(noop.Next!.AnalyzerClean);
    }

    [Fact]
    public void Discarded_or_unvalidated_candidates_cannot_grant_committed_trust()
    {
        using var fixture = new AnalysisModule();
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += new int[2].Length;", StringComparison.Ordinal));
        fixture.Start();
        var good = fixture.Module.State;
        fixture.Write(0, AnalysisModule.First);
        var candidate = fixture.Module.Full(Escalation.Requested, null, fixture);
        Assert.False(candidate.Next!.AnalyzerClean); // Rebuild uses committed warning source.
        Assert.False(candidate.ValidatedInputs);
        Assert.Same(good, fixture.Module.State);
        // Discard the candidate and perform an actual clean source rebuild.
        var clean = fixture.Module.Initial(fixture);
        Assert.True(clean.Next!.AnalyzerClean);
        Assert.False(fixture.Module.State!.AnalyzerClean);
        clean.ValidatedInputs = false;
        fixture.Module.Commit(clean);
        Assert.False(fixture.Module.State!.AnalyzerClean);
    }

    [Fact]
    public void A_dependency_rebind_revokes_the_old_compilations_proof_until_full_validation()
    {
        using var modules = new Modules();
        modules.Add(0, "Core", "public static class CoreRules { public static int Value() => 1; }");
        modules.Add(1, "Game", "public static class GameRules { public static int Value() => CoreRules.Value() + 1; }", references: ["Core"]);
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        Assert.True(modules.Server.Dependency("Game")!.AnalyzerClean);
        modules.Edit(0, "Core", "=> 1", "=> 2");
        Assert.Equal(MessageKind.Delta, modules.Sink.Next().Kind);
        Assert.False(modules.Server.Dependency("Game")!.AnalyzerClean);
        modules.Edit(1, "Game", "+ 1", "+ 2");
        Assert.Equal(MessageKind.Delta, modules.Sink.Next().Kind);
        Assert.True(modules.Server.Dependency("Game")!.AnalyzerClean);
    }

    [Fact]
    public void A_custom_provider_keeps_full_tree_analysis_and_never_earns_builtin_trust()
    {
        var analysis = new ForwardingAnalysis();
        using var fixture = new AnalysisModule(analysis);
        fixture.Start();
        Assert.False(fixture.Module.State!.AnalyzerClean);
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += 4;", StringComparison.Ordinal));
        var edit = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Delta, edit.Kind);
        Assert.False(edit.Next!.AnalyzerClean);
        Assert.Equal(2, analysis.Calls);
    }

    sealed class ForwardingAnalysis : IAnalysis
    {
        readonly AnalyzerSet _analyzers = new();
        internal int Calls;
        public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile)
        {
            Calls++;
            return _analyzers.Analyze(compilation, trees, profile);
        }
    }
}

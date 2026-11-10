using System.Collections.Immutable;
using System.Reflection;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.CompileServer.Tests;

public sealed class AnalysisCancellationTests
{
    [Fact]
    public void Token_adapters_preserve_each_legacy_override_and_check_both_boundaries()
    {
        var (compilation, tree, method) = Input();
        var legacy = new LegacyAnalysis();
        IAnalysis analysis = legacy;
        using var cancel = new CancellationTokenSource();
        analysis.Analyze(compilation, [tree], "gameplay", cancel.Token);
        analysis.AnalyzeBodies(compilation, [method], "gameplay", cancel.Token);
        analysis.AnalyzeModule(compilation, [tree], "gameplay", cancel.Token);
        Assert.Equal(new[] { "tree", "bodies", "module" }, legacy.Calls);
        legacy.During = cancel.Cancel;
        var stopped = Assert.ThrowsAny<OperationCanceledException>(() => analysis.AnalyzeModule(compilation, [tree], "gameplay", cancel.Token));
        Assert.Equal(cancel.Token, stopped.CancellationToken);
        var count = legacy.Calls.Count;
        Assert.ThrowsAny<OperationCanceledException>(() => analysis.Analyze(compilation, [tree], "gameplay", cancel.Token));
        Assert.ThrowsAny<OperationCanceledException>(() => analysis.AnalyzeBodies(compilation, [method], "gameplay", cancel.Token));
        Assert.ThrowsAny<OperationCanceledException>(() => analysis.AnalyzeModule(compilation, [tree], "gameplay", cancel.Token));
        Assert.Equal(count, legacy.Calls.Count);
    }

    [Theory]
    [InlineData("tree")]
    [InlineData("bodies")]
    [InlineData("module")]
    public async Task Builtin_requests_cancel_inflight_and_preserve_real_analyzer_failures(string coverage)
    {
        var (compilation, tree, method) = Input();
        using var cancel = new CancellationTokenSource();
        using var entered = new ManualResetEventSlim();
        using var exited = new ManualResetEventSlim();
        using var release = new ManualResetEventSlim();
        var analyzer = Inject(new InvocationAnalyzer(context =>
        {
            entered.Set();
            try
            {
                Assert.NotEqual(WaitHandle.WaitTimeout,
                    WaitHandle.WaitAny([context.CancellationToken.WaitHandle, release.WaitHandle], TimeSpan.FromSeconds(30)));
                context.CancellationToken.ThrowIfCancellationRequested();
            }
            finally { exited.Set(); }
        }));
        var running = Task.Run(() => Request(analyzer, coverage, compilation, tree, method, cancel.Token));
        try
        {
            Assert.True(entered.Wait(TimeSpan.FromSeconds(30)));
            cancel.Cancel();
            var stopped = await Assert.ThrowsAnyAsync<OperationCanceledException>(async () => await running.WaitAsync(TimeSpan.FromSeconds(30)));
            Assert.Equal(cancel.Token, stopped.CancellationToken);
            // Public request completion alone is not a private Roslyn worker join.
            Assert.True(exited.Wait(TimeSpan.FromSeconds(30)));
        }
        finally { cancel.Cancel(); release.Set(); await running.ContinueWith(task => { _ = task.Exception; }); }
        Assert.ThrowsAny<OperationCanceledException>(() => Request(analyzer, coverage, compilation, tree, method, cancel.Token));
        foreach (var error in new Exception[] { new InvalidOperationException("actual analyzer failure"), new OperationCanceledException(new CancellationToken(true)) })
        {
            var failing = Inject(new InvocationAnalyzer(_ => throw error));
            var raw = Request(failing, coverage, compilation, tree, method, CancellationToken.None);
            Assert.Contains(raw, diagnostic => diagnostic.Id == "AD0001" && diagnostic.Severity == DiagnosticSeverity.Error);
        }
    }

    [Fact]
    public async Task Unconsumed_analysis_drains_and_retains_secondary_faults_and_raw_failure_diagnostics()
    {
        using var entered = new ManualResetEventSlim();
        using var cancelled = new ManualResetEventSlim();
        using var release = new ManualResetEventSlim();
        using var exited = new ManualResetEventSlim();
        var primary = new InvalidOperationException("primary emit failure");
        using var analysis = new PendingAnalysis(token =>
        {
            entered.Set();
            try
            {
                token.WaitHandle.WaitOne();
                cancelled.Set();
                Assert.True(release.Wait(TimeSpan.FromSeconds(30)));
                throw new InvalidOperationException("secondary analyzer failure");
            }
            finally { exited.Set(); }
        }, CancellationToken.None);
        Assert.True(entered.Wait(TimeSpan.FromSeconds(30)));
        var finishing = Task.Run(() =>
        {
            try { throw primary; }
            finally { analysis.Dispose(); Assert.True(exited.IsSet); }
        });
        try { Assert.True(cancelled.Wait(TimeSpan.FromSeconds(30))); Assert.False(finishing.IsCompleted); }
        finally { release.Set(); }
        Assert.Same(primary, await Assert.ThrowsAsync<InvalidOperationException>(async () => await finishing.WaitAsync(TimeSpan.FromSeconds(30))));
        Assert.Equal("secondary analyzer failure", Assert.IsType<InvalidOperationException>(analysis.Failure).Message);
        using var parent = new CancellationTokenSource();
        parent.Cancel();
        using var preCancelled = new PendingAnalysis(_ => throw new InvalidOperationException("must not run"), parent.Token);
        Assert.Equal(parent.Token, Assert.ThrowsAny<OperationCanceledException>(() => preCancelled.Complete()).CancellationToken);
        var failure = Diagnostic.Create(new DiagnosticDescriptor("AD0001", "Failure", "raw failure", "Compiler", DiagnosticSeverity.Error, true), Location.None);
        using var produced = new ManualResetEventSlim();
        using var completed = new PendingAnalysis(_ => { produced.Set(); return [failure]; }, CancellationToken.None);
        Assert.True(produced.Wait(TimeSpan.FromSeconds(30)));
        completed.Dispose();
        Assert.Same(failure, Assert.Single(completed.Failures));
    }

    [Theory]
    [InlineData("change", false)]
    [InlineData("recover", true)]
    [InlineData("watch", true)]
    public void Human_work_preempts_running_agent_analysis_before_the_next_provider_runs(string human, bool image)
    {
        using var fixture = new CancellationFixture();
        var good = fixture.Server.Dependency("Game");
        fixture.Gate.Armed = 1;
        var source = CancellationFixture.Source.Replace("=> 1", "=> 2", StringComparison.Ordinal);
        if (image) source = source.Replace("int Value", "long Value", StringComparison.Ordinal);
        File.WriteAllText(fixture.Path, source);
        fixture.Server.Change(0, [fixture.Path], AbiConst.OriginAgent, 0);
        try
        {
            Assert.True(fixture.Gate.Entered.Wait(TimeSpan.FromSeconds(30)));
            Assert.Same(good, fixture.Server.Dependency("Game"));
            if (human == "watch") fixture.WatchOther();
            else
            {
                File.WriteAllText(fixture.Path, CancellationFixture.Source.Replace("=> 1", "=> 3", StringComparison.Ordinal));
                if (human == "recover") fixture.Server.Recover(42);
                else fixture.Server.Change(0, [fixture.Path], AbiConst.OriginHuman, 0);
            }
            Assert.True(fixture.Gate.Cancelled.Wait(TimeSpan.FromSeconds(30)));
            fixture.Idle();
            Assert.True(fixture.Gate.Exited.IsSet);
            Assert.Equal(0, fixture.Gate.Active);
            Assert.Contains(fixture.Sink.Completions, row => (string?)row["result"] == "cancelled");
            var state = fixture.Server.Dependency("Game")!;
            Assert.Contains(human == "watch" ? "=> 2" : "=> 3", state.Texts[fixture.Path]);
            Assert.False(state.AnalyzerClean); // Custom providers never earn built-in proof.
            var messages = fixture.Sink.Messages.ToArray();
            if (human == "watch")
            {
                var watched = Array.FindIndex(messages, message => message.Kind == MessageKind.Reload && message.Reload().Any(entry => entry.Slot == 1));
                var resumed = Array.FindIndex(messages, message => message.Kind == MessageKind.Reload && message.Reload().Any(entry => entry.Slot == 0 && entry.Origin == AbiConst.OriginAgent));
                Assert.True(watched >= 0 && resumed > watched);
            }
            else
                Assert.DoesNotContain(messages, message => message.Kind == MessageKind.Delta && message.Delta().Origin == AbiConst.OriginAgent
                    || message.Kind == MessageKind.Reload && message.Reload().Any(entry => entry.Origin == AbiConst.OriginAgent));
        }
        finally { fixture.Gate.Release.Set(); }
    }

    [Theory]
    [InlineData("human")]
    [InlineData("agent")]
    [InlineData("stop")]
    public void Only_requested_cancellation_yields_and_uncancelled_provider_failures_leave_the_worker_usable(string action)
    {
        using var fixture = new CancellationFixture();
        var good = fixture.Server.Dependency("Game");
        fixture.Gate.Unrequested = action != "stop";
        fixture.Gate.Armed = 1;
        File.WriteAllText(fixture.Path, CancellationFixture.Source.Replace("=> 1", "=> 2", StringComparison.Ordinal));
        fixture.Server.Change(0, [fixture.Path], action == "agent" ? AbiConst.OriginAgent : AbiConst.OriginHuman, 0);
        if (action == "stop")
        {
            try
            {
                Assert.True(fixture.Gate.Entered.Wait(TimeSpan.FromSeconds(30)));
                fixture.Server.Dispose(); // Requests stop; no unbounded worker join.
                Assert.True(fixture.Gate.Cancelled.Wait(TimeSpan.FromSeconds(30)));
                Assert.True(fixture.Gate.Exited.Wait(TimeSpan.FromSeconds(30)));
                fixture.Idle();
                Assert.Same(good, fixture.Server.Dependency("Game"));
            }
            finally { fixture.Gate.Release.Set(); }
            return;
        }
        fixture.Idle();
        Assert.Same(good, fixture.Server.Dependency("Game"));
        Assert.Contains(fixture.Sink.Messages, message => message.Kind == MessageKind.Diagnostics && message.Diagnostics().Errors > 0
            && message.Diagnostics().Json.Contains("unrequested cancellation", StringComparison.Ordinal));
        Assert.DoesNotContain(fixture.Sink.Logs, log => log.Contains("yielded", StringComparison.Ordinal));
        fixture.Gate.Unrequested = false;
        File.WriteAllText(fixture.Path, CancellationFixture.Source.Replace("=> 1", "=> 3", StringComparison.Ordinal));
        fixture.Server.Change(0, [fixture.Path], AbiConst.OriginHuman, 0);
        fixture.Idle();
        Assert.Contains("=> 3", fixture.Server.Dependency("Game")!.Texts[fixture.Path]);
    }

    static (Compilation Compilation, SyntaxTree Tree, MethodDeclarationSyntax Method) Input()
    {
        var tree = ModuleCompiler.Parse("Cancellation.cs", "static class C { static int M() => System.Math.Abs(-2); }");
        var compilation = new ModuleCompiler("Cancellation", [typeof(Behaviour).Assembly.Location]).Create([tree], true, []);
        return (compilation, tree, Assert.Single(tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>()));
    }

    static ImmutableArray<Diagnostic> Request(AnalyzerSet analyzer, string coverage, Compilation compilation, SyntaxTree tree, MethodDeclarationSyntax method, CancellationToken ct) => coverage switch
    {
        "tree" => analyzer.Analyze(compilation, [tree], "gameplay", ct),
        "bodies" => analyzer.AnalyzeBodies(compilation, [method], "gameplay", ct),
        _ => analyzer.AnalyzeModule(compilation, [tree], "gameplay", ct),
    };

    static AnalyzerSet Inject(DiagnosticAnalyzer analyzer)
    {
        var set = new AnalyzerSet();
        typeof(AnalyzerSet).GetField("_analyzers", BindingFlags.Instance | BindingFlags.NonPublic)!
            .SetValue(set, ImmutableArray.Create(analyzer));
        return set;
    }

#pragma warning disable RS1001 // Only instantiated by the test, never discovered as an extension.
    sealed class InvocationAnalyzer(Action<OperationAnalysisContext> action) : DiagnosticAnalyzer
    {
        public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => new Ara.Analyzers.AllocationAnalyzer().SupportedDiagnostics;
        public override void Initialize(AnalysisContext context)
        {
            context.EnableConcurrentExecution();
            context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
            context.RegisterOperationAction(action, OperationKind.Invocation);
        }
    }
#pragma warning restore RS1001

    sealed class LegacyAnalysis : IAnalysis
    {
        internal readonly List<string> Calls = [];
        internal Action? During;
        public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile) => Check("tree");
        public ImmutableArray<Diagnostic> AnalyzeBodies(Compilation compilation, IReadOnlyCollection<MethodDeclarationSyntax> methods, string profile) => Check("bodies");
        public ImmutableArray<Diagnostic> AnalyzeModule(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile) => Check("module");
        ImmutableArray<Diagnostic> Check(string coverage) { Calls.Add(coverage); During?.Invoke(); return []; }
    }
}

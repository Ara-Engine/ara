using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text.Json.Nodes;
using Ara.Hosting;
using System.Collections.Immutable;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

public sealed class CompileLifecycleTests
{
    sealed class Gate : IAnalysis, IDisposable
    {
        internal int Armed;
        internal readonly ManualResetEventSlim Entered = new(false);
        internal readonly ManualResetEventSlim Release = new(false);
        public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile)
        {
            if (Interlocked.Exchange(ref Armed, 0) == 1)
            {
                Entered.Set();
                Assert.True(Release.Wait(TimeSpan.FromSeconds(20)));
            }
            return [];
        }
        public void Dispose() { Entered.Dispose(); Release.Dispose(); }
    }
    const string Source = """
        /// <summary>A counter in ECS.</summary>
        [Component] public partial struct Count { /// <summary>Ticks.</summary>
            public int Value; }
        public static partial class Counting {
            [Query] static void Run(ref Count c) { c.Value += 1; }
        }
        """;

    sealed class Sink : IMessageSink
    {
        internal readonly ConcurrentQueue<Sent> Messages = new();
        public void Send(MessageKind kind, byte[] payload) => Messages.Enqueue(new(kind, payload));
        internal JsonObject[] Rows => Messages.Where(m => m.Kind == MessageKind.Diagnostics)
            .SelectMany(m => JsonNode.Parse(m.Diagnostics().Json)!.AsArray().Select(d => d!.AsObject())).ToArray();
        internal JsonObject Last(string module) => Rows.Last(d => (string?)d["data"]?["module"] == module
            && (string?)d["data"]?["compile"]?["phase"] == "completed");
    }

    static string Watch(Coordinator server, string root, uint slot, string module, string source = Source, string[]? references = null)
    {
        var dir = Path.Combine(root, module);
        Directory.CreateDirectory(dir);
        var path = Path.Combine(dir, "Counter.cs");
        File.WriteAllText(path, source);
        server.Watch(slot, module, dir, true, [typeof(Behaviour).Assembly.Location], references ?? [], "gameplay", watch:false);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        return path;
    }

    static void Edit(Coordinator server, uint slot, string path, string text)
    {
        File.WriteAllText(path, text);
        server.Change(slot, [path], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
    }

    [Fact]
    public void Repair_has_a_new_terminal_attempt_without_an_alc_generation_change_or_clearing_another_module()
    {
        var root = InstalledTools.Scratch("compile-lifecycle");
        var sink = new Sink();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline AddMethodToExistingType AddStaticFieldToExistingType AddInstanceFieldToExistingType NewTypeDefinition");
        var combat = Watch(server, root, 0, "Combat");
        var inventory = Watch(server, root, 1, "Inventory");
        Edit(server, 0, combat, Source.Replace("+= 1", "+= Missing"));
        Edit(server, 1, inventory, Source.Replace("+= 1", "+= Missing"));
        var failed = sink.Last("Combat");
        Assert.Equal("errors", (string?)failed["data"]?["compile"]?["result"]);
        Assert.True((int)failed["data"]!["compile"]!["errorCount"]! > 0);
        Edit(server, 0, combat, Source.Replace("+= 1", "+= 5"));
        var complete = sink.Last("Combat");
        var current = complete["data"]!["compile"]!;
        Assert.Equal("success", (string?)current["result"]);
        Assert.Equal("current-module-inputs", (string?)current["validation"]);
        Assert.Equal(0, (int)current["errorCount"]!);
        Assert.True((ulong)current["attempt"]! > (ulong)failed["data"]!["compile"]!["attempt"]!);
        Assert.Equal((string?)failed["data"]?["compile"]?["instance"], (string?)current["instance"]);
        Assert.Equal("errors", (string?)sink.Last("Inventory")["data"]?["compile"]?["result"]);
        var delta = sink.Messages.Last(m => m.Kind == MessageKind.Delta).Delta();
        Assert.Equal(0u, delta.Generation);
        Assert.Equal(AbiConst.ReloadL1, delta.Kind);
        var error = sink.Rows.First(d => (string?)d["data"]?["module"] == "Combat" && (string?)d["severity"] == "error");
        Assert.Equal(combat.Replace('\\', '/'), (string?)error["location"]?["file"]);
        Assert.Contains(error["subjects"]!.AsArray(), s => (string?)s == "module:Combat");
        Assert.Contains(error["subjects"]!.AsArray(), s => ((string?)s)?.StartsWith("system:csharp.compile/", StringComparison.Ordinal) == true);
    }

    [Fact]
    public void Exact_last_good_restoration_completes_without_reload_but_rebuilding_committed_code_does_not_clear_broken_disk_input()
    {
        var sink = new Sink();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        var path = Watch(server, InstalledTools.Scratch("restored-input"), 0, "Combat");
        Edit(server, 0, path, Source.Replace("+= 1", "+= Missing"));
        var failed = sink.Last("Combat");
        server.Rebuild(0, Escalation.Requested);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var rebuilt = sink.Last("Combat")["data"]!["compile"]!;
        Assert.NotEqual("success", (string?)rebuilt["result"]);
        Assert.NotEqual("current-module-inputs", (string?)rebuilt["validation"]);
        var messages = sink.Messages.Count;
        Edit(server, 0, path, Source);
        var restored = sink.Last("Combat")["data"]!["compile"]!;
        Assert.Equal("success", (string?)restored["result"]);
        Assert.Equal("current-module-inputs", (string?)restored["validation"]);
        Assert.True((ulong)restored["attempt"]! > (ulong)failed["data"]!["compile"]!["attempt"]!);
        Assert.DoesNotContain(sink.Messages.Skip(messages), message => message.Kind is MessageKind.Reload or MessageKind.Delta);
    }

    [Fact]
    public void Truncated_errors_advertise_the_complete_count_and_partial_warning_coverage()
    {
        var sink = new Sink();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        var path = Watch(server, InstalledTools.Scratch("truncated-input"), 0, "Combat");
        Edit(server, 0, path, string.Join('\n', Enumerable.Range(0, 70).Select(i => "#error broken " + i)));
        var completed = sink.Last("Combat")["data"]!["compile"]!;
        Assert.Equal("errors", (string?)completed["result"]);
        Assert.True((int)completed["errorCount"]! >= 70);
        Assert.Equal(50, (int)completed["emitted"]!);
        Assert.True((bool)completed["truncated"]!);
        Assert.Equal("partial", (string?)completed["coverage"]?["warnings"]);
        Assert.Equal("module", (string?)completed["coverage"]?["errors"]);
    }

    [Fact]
    public void Preempted_agent_attempt_is_cancelled_and_later_evaluations_have_new_identities()
    {
        using var gate = new Gate();
        var sink = new Sink();
        using var server = new Coordinator(sink, gate) { AgentQuiet=TimeSpan.Zero };
        server.Hello(true, "Baseline");
        var path = Watch(server, InstalledTools.Scratch("overlapping-inputs"), 0, "Combat");
        gate.Armed = 1;
        File.WriteAllText(path, Source.Replace("+= 1", "+= 2"));
        server.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp());
        try
        {
            Assert.True(gate.Entered.Wait(TimeSpan.FromSeconds(20)));
            File.WriteAllText(path, Source.Replace("+= 1", "+= 3"));
            server.Change(0, [path], AbiConst.OriginHuman, Stopwatch.GetTimestamp());
        }
        finally { gate.Release.Set(); }
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var completions = sink.Rows.Where(row => (string?)row["data"]?["compile"]?["phase"] == "completed").ToArray();
        Assert.Contains(completions, row => (string?)row["data"]?["compile"]?["result"] == "cancelled");
        var attempts = completions.Select(row => (ulong)row["data"]!["compile"]!["attempt"]!).ToArray();
        Assert.Equal(attempts.Order(), attempts);
        Assert.Equal(attempts.Length, attempts.Distinct().Count());
        Assert.Equal("success", (string?)sink.Last("Combat")["data"]?["compile"]?["result"]);
    }

    [Fact]
    public void Recovery_rejects_inputs_changed_during_preparation_and_cannot_publish_verified_success()
    {
        using var gate = new Gate();
        var sink = new Sink();
        using var server = new Coordinator(sink, gate);
        server.Hello(true, "Baseline");
        var path = Watch(server, InstalledTools.Scratch("recovery-stale-input"), 0, "Combat");
        var before = sink.Messages.Count;
        gate.Armed = 1;
        server.Recover(82);
        try {
            Assert.True(gate.Entered.Wait(TimeSpan.FromSeconds(20)));
            File.WriteAllText(path, Source.Replace("+= 1", "+= Missing"));
        }
        finally { gate.Release.Set(); }
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.DoesNotContain(sink.Messages.Skip(before), message => message.Kind is MessageKind.Reload or MessageKind.Delta or MessageKind.Restart);
        Assert.NotEqual("success", (string?)sink.Last("Combat")["data"]?["compile"]?["result"]);
        Assert.NotEqual("current-module-inputs", (string?)sink.Last("Combat")["data"]?["compile"]?["validation"]);
        var finished = sink.Messages.Last(message => message.Kind == MessageKind.RecoveryFinished);
        var reader = new Protocol.Reader(finished.Payload);
        Assert.Equal(82ul, reader.U64());
        Assert.Equal("failed", reader.Str());
    }

    [Fact]
    public void Trivia_restoration_and_each_cascade_module_have_validated_independent_results()
    {
        var sink = new Sink();
        using var server = new Coordinator(sink, new AnalyzerSet());
        server.Hello(true, "Baseline");
        var root = InstalledTools.Scratch("cascade-lifecycle");
        var source = Watch(server, root, 0, "Combat");
        Edit(server, 0, source, Source.Replace("+= 1", "+= Missing"));
        Edit(server, 0, source, Source + "\n// restored with trivia\n");
        Assert.Equal("success", (string?)sink.Last("Combat")["data"]?["compile"]?["result"]);
        Watch(server, root, 1, "Inventory", "public class UsesCombat { public int Value(Count c) => c.Value; }", ["Combat"]);
        server.Rebuild(0, Escalation.Requested);
        Assert.True(server.WaitIdle(TimeSpan.FromSeconds(60)));
        var combat = sink.Last("Combat")["data"]!["compile"]!;
        var inventory = sink.Last("Inventory")["data"]!["compile"]!;
        Assert.Equal("success", (string?)combat["result"]);
        Assert.Equal("success", (string?)inventory["result"]);
        Assert.Equal("current-module-inputs", (string?)inventory["validation"]);
        Assert.NotEqual((ulong)combat["attempt"]!, (ulong)inventory["attempt"]!);
        Assert.Equal((string?)combat["instance"], (string?)inventory["instance"]);
    }
}

using System.Diagnostics;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>Attribution follows materialized source edits, independently of worker latency.</summary>
public sealed class SourceClaimTests
{
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Delayed_source_replace_materialization_preserves_agent_origin(bool directoryEcho)
    {
        var root = InstalledTools.Scratch("delayed-source-claim");
        var path = Path.Combine(root, "Code.cs");
        const string before = "public static class Work { public static int Run() => 1; }";
        const string after = "public static class Work { public static int Run() => 2; }";
        File.WriteAllText(path, before);
        long claimTime = 0;
        var capture = new Capture();
        using var coordinator = new Coordinator(capture, null, () => Volatile.Read(ref claimTime)) { AgentQuiet = TimeSpan.Zero };
        coordinator.Hello(true, "Baseline");
        coordinator.Watch(0, "game", root, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        Assert.Equal(AbiConst.ReloadInitial, capture.Next().Reload().Single().Kind);

        // script.write claims this exact pending revision before source.replace's
        // materialization. The clock then advances without any wall-clock sleep.
        var claim = new SourceClaim("source-job-1", Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(after))), "preparing");
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), claim);
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        File.WriteAllText(path, "public static class Work { public static int Run() => 3; }");
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginHuman, capture.Next().Delta().Origin);
        Interlocked.Exchange(ref claimTime, 3 * Stopwatch.Frequency);
        File.WriteAllText(path, after);
        // Deliver the real watcher's settled change synchronously: no timer or sleep
        // determines whether attribution is inspected before or after expiry.
        coordinator.OnWatcher(0, directoryEcho ? [root, path] : [path], Stopwatch.GetTimestamp());
        var delta = capture.Next().Delta();
        Assert.Equal(AbiConst.OriginAgent, delta.Origin);
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), claim with { Phase = "materialized" });
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        Assert.True(capture.Empty, "duplicate materialization/watch echoes do not reload twice");

        // A later human save is not captured by the old path claim, even if it
        // arrives immediately and rewrites a revision the agent used previously.
        File.WriteAllText(path, before);
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginHuman, capture.Next().Delta().Origin);
        File.WriteAllText(path, after);
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginHuman, capture.Next().Delta().Origin);
    }

    [Fact]
    public void Cancelling_one_revision_does_not_claim_future_human_bytes_or_remove_another_edit()
    {
        var root = InstalledTools.Scratch("cancelled-source-claim");
        var path = Path.Combine(root, "Code.cs");
        const string before = "public static class Work { public static int Run() => 1; }";
        const string first = "public static class Work { public static int Run() => 2; }";
        const string second = "public static class Work { public static int Run() => 3; }";
        File.WriteAllText(path, before);
        var capture = new Capture();
        using var coordinator = new Coordinator(capture, null) { AgentQuiet = TimeSpan.Zero };
        coordinator.Hello(true, "Baseline");
        coordinator.Watch(0, "game", root, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        capture.Next();
        static SourceClaim Claim(string edit, string text) => new(edit,
            Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(text))), "preparing");
        var a = Claim("first", first);
        var b = Claim("second", second);
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), a);
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), b);
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), a with { Phase = "cancelled" });
        File.WriteAllText(path, second);
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginAgent, capture.Next().Delta().Origin);
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), b with { Phase = "materialized" });
        Assert.True(coordinator.WaitIdle(TimeSpan.FromSeconds(60)));
        File.WriteAllText(path, first);
        coordinator.OnWatcher(0, [path], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginHuman, capture.Next().Delta().Origin);
    }

    [Fact]
    public void A_mixed_batch_of_claimed_and_unclaimed_source_bytes_remains_human_work()
    {
        var root = InstalledTools.Scratch("mixed-source-claim");
        var path = Path.Combine(root, "Code.cs");
        var human = Path.Combine(root, "Other.cs");
        File.WriteAllText(path, "public static class Work { public static int Run() => 1; }");
        File.WriteAllText(human, "public static class Other { public static int Run() => 1; }");
        const string after = "public static class Work { public static int Run() => 2; }";
        var capture = new Capture();
        using var coordinator = new Coordinator(capture, null) { AgentQuiet = TimeSpan.Zero };
        coordinator.Hello(true, "Baseline");
        coordinator.Watch(0, "game", root, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        capture.Next();
        var claim = new SourceClaim("agent-job", Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(after))), "preparing");
        coordinator.Change(0, [path], AbiConst.OriginAgent, Stopwatch.GetTimestamp(), claim);
        File.WriteAllText(path, after);
        File.WriteAllText(human, "public static class Other { public static int Run() => 3; }");
        coordinator.OnWatcher(0, [root, path, human], Stopwatch.GetTimestamp());
        Assert.Equal(AbiConst.OriginHuman, capture.Next().Delta().Origin);
    }
}

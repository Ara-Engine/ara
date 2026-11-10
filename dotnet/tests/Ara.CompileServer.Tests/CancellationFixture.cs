using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Text.Json.Nodes;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

/// <summary>Token-aware provider gates and captured coordinator boundaries, without timing assumptions.</summary>
sealed class CancellationFixture : IDisposable
{
    internal const string Source = "public static class Rules { public static int Value() => 1; }";
    readonly string _directory = InstalledTools.Scratch("analysis-cancellation");
    internal readonly CancellationGate Gate = new();
    internal readonly CancellationSink Sink = new();
    internal readonly Coordinator Server;
    internal readonly string Path;

    internal CancellationFixture()
    {
        Path = System.IO.Path.Combine(_directory, "Game", "Game.cs");
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
        File.WriteAllText(Path, Source);
        Server = new Coordinator(Sink, Gate) { AgentQuiet = TimeSpan.Zero };
        Server.Hello(true, "Baseline");
        Server.Watch(0, "Game", System.IO.Path.GetDirectoryName(Path)!, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        Idle();
        Assert.NotNull(Server.Dependency("Game"));
        Sink.Messages.Clear();
    }

    internal void WatchOther()
    {
        var dir = System.IO.Path.Combine(_directory, "Other");
        Directory.CreateDirectory(dir);
        File.WriteAllText(System.IO.Path.Combine(dir, "Other.cs"), "public static class Other { public static int Value() => 9; }");
        Server.Watch(1, "Other", dir, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
    }

    internal void Idle() => Assert.True(Server.WaitIdle(TimeSpan.FromSeconds(60)));
    public void Dispose()
    {
        Gate.Release.Set();
        Server.Dispose();
        if (Gate.Entered.IsSet) Gate.Exited.Wait(TimeSpan.FromSeconds(30));
        try { Directory.Delete(_directory, recursive: true); } catch (IOException) { }
        // Coordinator shutdown remains nonjoining; its callback events stay alive
        // until the provider exits even during failed-test cleanup.
    }
}

sealed class CancellationGate : IAnalysis
{
    internal int Armed;
    internal int Active;
    internal bool Unrequested;
    internal readonly ManualResetEventSlim Entered = new();
    internal readonly ManualResetEventSlim Cancelled = new();
    internal readonly ManualResetEventSlim Exited = new();
    internal readonly ManualResetEventSlim Release = new();
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile) => [];
    public ImmutableArray<Diagnostic> AnalyzeModule(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile, CancellationToken ct) =>
        Analyze(compilation, trees, profile, ct);
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile, CancellationToken ct)
    {
        Assert.Equal(0, Volatile.Read(ref Active));
        if (compilation.AssemblyName != "Game" || Interlocked.Exchange(ref Armed, 0) != 1) return [];
        Interlocked.Increment(ref Active);
        Entered.Set();
        try
        {
            if (Unrequested) throw new OperationCanceledException("unrequested cancellation", ct);
            var signaled = WaitHandle.WaitAny([ct.WaitHandle, Release.WaitHandle], TimeSpan.FromSeconds(30));
            Assert.NotEqual(WaitHandle.WaitTimeout, signaled);
            if (ct.IsCancellationRequested) Cancelled.Set();
            ct.ThrowIfCancellationRequested();
            return [];
        }
        finally { Interlocked.Decrement(ref Active); Exited.Set(); }
    }
}

sealed class CancellationSink : IMessageSink
{
    internal readonly ConcurrentQueue<Sent> Messages = new();
    internal readonly ConcurrentQueue<string> Logs = new();
    internal JsonObject[] Completions => [.. Messages.Where(message => message.Kind == MessageKind.Diagnostics)
        .SelectMany(message => JsonNode.Parse(message.Diagnostics().Json)!.AsArray().Select(row => row!["data"]?["compile"] as JsonObject))
        .OfType<JsonObject>().Where(row => (string?)row["phase"] == "completed")];
    public void Send(MessageKind kind, byte[] payload)
    {
        if (kind == MessageKind.Log)
        {
            var reader = new Protocol.Reader(payload);
            reader.U32();
            Logs.Enqueue(reader.Str());
        }
        Messages.Enqueue(new Sent(kind, payload));
    }
}

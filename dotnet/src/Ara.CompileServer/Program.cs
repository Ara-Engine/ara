using System.Diagnostics;
using System.IO.Pipes;
using Ara.CompileServer;
using Ara.Hosting;

// Ara.CompileServer --pipe <name> --parent <pid>
//
// Started by the game or editor process (Ara.Host's CompileClient). Connects to its pipe and
// exits with its parent. Idle and agent compiles run below normal priority; the human's run at
// normal priority while the host raises its tick threads above normal (A13, D20).
try { Process.GetCurrentProcess().PriorityClass = ProcessPriorityClass.BelowNormal; }
catch (Exception) { /* not allowed on this platform: compiles still run out of process */ }

if (args.Contains("--inventory", StringComparer.Ordinal))
{
    var request = System.Text.Json.JsonSerializer.Deserialize<InventoryRequest>(await Console.In.ReadToEndAsync(), CodeFixRunner.Json)
        ?? throw new ArgumentException("missing inventory request");
    Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(ProjectInventory.Scan(request), CodeFixRunner.Json));
    return 0;
}

if (args.Contains("--compile-project", StringComparer.Ordinal))
{
    var request = System.Text.Json.JsonSerializer.Deserialize<ProjectBuildRequest>(await Console.In.ReadToEndAsync(), CodeFixRunner.Json)
        ?? throw new ArgumentException("missing project compilation request");
    Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(ProjectCompiler.Compile(request), CodeFixRunner.Json));
    return 0;
}

if (args.Contains("--fix", StringComparer.Ordinal))
{
    try
    {
        var request = System.Text.Json.JsonSerializer.Deserialize<FixRequest>(await Console.In.ReadToEndAsync(), CodeFixRunner.Json)
            ?? throw new ArgumentException("missing code-fix request");
        Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(await CodeFixRunner.Run(request), CodeFixRunner.Json));
        return 0;
    }
    catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
}
var pipeName = Arg("--pipe") ?? throw new ArgumentException("missing --pipe <name>");
var parent = int.TryParse(Arg("--parent"), out var pid) ? pid : 0;

using var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
pipe.Connect(TimeSpan.FromSeconds(30));
if (parent != 0) _ = Task.Run(() => WatchParent(parent));

using var coordinator = new Coordinator(new PipeSink(pipe), new AnalyzerSet());
while (Protocol.Receive(pipe) is { } msg)
{
    var r = new Protocol.Reader(msg.Payload);
    switch (msg.Kind)
    {
        case MessageKind.Hello:
            var version = r.U32();
            if (version != Protocol.Version) throw new InvalidOperationException($"protocol {version}, server {Protocol.Version}");
            r.U32();
            var enc = r.Bool();
            coordinator.Hello(enc, r.Str());
            break;
        case MessageKind.Watch:
            var (slot, name, dir, hot, refs, modules, profile, watch) = (r.U32(), r.Str(), r.Str(), r.Bool(), r.Strs(), r.Strs(), r.Str(), r.Bool());
            coordinator.Watch(slot, name, dir, hot, refs, modules, profile, watch);
            break;
        case MessageKind.Change:
            var (changeSlot, paths, origin) = (r.U32(), r.Strs(), r.U32());
            coordinator.Change(changeSlot, paths, origin, Stopwatch.GetTimestamp());
            break;
        case MessageKind.SetHeat:
            var (heatSlot, makeHot) = (r.U32(), r.Bool());
            coordinator.SetHeat(heatSlot, makeHot);
            break;
        case MessageKind.Rebuild:
            var (rebuildSlot, escalation) = (r.U32(), r.U32());
            coordinator.Rebuild(rebuildSlot, (Escalation)escalation);
            break;
        case MessageKind.Shutdown:
            return 0;
    }
}
return 0;

string? Arg(string name)
{
    var i = Array.IndexOf(args, name);
    return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
}

static async Task WatchParent(int pid)
{
    try { await Process.GetProcessById(pid).WaitForExitAsync(); }
    catch (ArgumentException) { /* already gone */ }
    Environment.Exit(0);
}

/// <summary>Sends coordinator messages to the host over the pipe.</summary>
sealed class PipeSink(Stream pipe) : IMessageSink
{
    /// <inheritdoc/>
    public void Send(MessageKind kind, byte[] payload)
    {
        try { Protocol.Send(pipe, kind, payload); }
        catch (IOException) { Environment.Exit(0); } // the host is gone
    }
}

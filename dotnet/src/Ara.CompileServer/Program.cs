using System.Diagnostics;
using System.IO.Pipes;
using Ara.CompileServer;
using Ara.Hosting;

// Ara.CompileServer --pipe <name> --parent <pid>
//
// Started by the game or editor process (Ara.Host's CompileClient). Connects to its pipe,
// runs below normal priority (so compiles never starve ticks, A13), exits with its parent.
var pipeName = Arg("--pipe") ?? throw new ArgumentException("missing --pipe <name>");
var parent = int.TryParse(Arg("--parent"), out var pid) ? pid : 0;

try { Process.GetCurrentProcess().PriorityClass = ProcessPriorityClass.BelowNormal; }
catch (Exception) { /* not allowed on this platform: compiles still run out of process */ }

using var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
pipe.Connect(TimeSpan.FromSeconds(30));
if (parent != 0) _ = Task.Run(() => WatchParent(parent));

var sink = new PipeSink(pipe);
var sessions = new Dictionary<uint, ModuleSession>();
var enc = false;
var analysis = new AnalyzerSet();
while (Protocol.Receive(pipe) is { } msg)
{
    var r = new Protocol.Reader(msg.Payload);
    switch (msg.Kind)
    {
        case MessageKind.Hello:
            var version = r.U32();
            if (version != Protocol.Version) throw new InvalidOperationException($"protocol {version}, server {Protocol.Version}");
            r.U32();
            enc = r.Bool();
            break;
        case MessageKind.Watch:
            var (slot, name, dir, hot, refs) = (r.U32(), r.Str(), r.Str(), r.Bool(), r.Strs());
            var session = new ModuleSession(sink, slot, name, dir, hot, enc, refs, analysis);
            sessions[slot] = session;
            session.Start(watch: true);
            break;
        case MessageKind.SetHeat:
            var (heatSlot, makeHot) = (r.U32(), r.Bool());
            if (sessions.TryGetValue(heatSlot, out var s1)) s1.SetHeat(makeHot);
            break;
        case MessageKind.Rebuild:
            var (rebuildSlot, escalation) = (r.U32(), r.U32());
            if (sessions.TryGetValue(rebuildSlot, out var s2)) s2.Rebuild(escalation);
            break;
        case MessageKind.Shutdown:
            foreach (var s in sessions.Values) s.Dispose();
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

/// <summary>Sends session messages to the host over the pipe.</summary>
sealed class PipeSink(Stream pipe) : IMessageSink
{
    /// <inheritdoc/>
    public void Send(MessageKind kind, byte[] payload)
    {
        try { Protocol.Send(pipe, kind, payload); }
        catch (IOException) { Environment.Exit(0); } // the host is gone
    }
}

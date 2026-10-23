using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Agent;
using Ara.RunWorker;

// The ara_run worker. Protocol: one JSON object per line.
//   supervisor → worker: {"type":"run","id":N,"code":"..."} | {"type":"reply","seq":S,"ok":true,"output":...} | {"type":"exit"}
//   worker → supervisor: {"type":"ready"} | {"type":"call","seq":S,"command":"...","params":{...}}
//                        | {"type":"done","id":N,"ok":bool,"result":...,"why":...,"log":[...],"calls":n,"diagnostics":[...]}
// Limits (wall clock, memory) are the supervisor's job: it kills this process on violation.
var output = Console.Out;
var writeLock = new object();
var runs = new BlockingCollection<(long Id, string Code)>();
var sink = new StdioSink(Send);

void Send(JsonObject message)
{
    var line = message.ToJsonString();
    lock (writeLock)
    {
        output.Write(line);
        output.Write('\n');
        output.Flush();
    }
}

var reader = new Thread(() =>
{
    while (Console.In.ReadLine() is { } line)
    {
        if (line.Length == 0) continue;
        var msg = JsonNode.Parse(line)!.AsObject();
        switch ((string?)msg["type"])
        {
            case "run": runs.Add(((long)msg["id"]!, (string)msg["code"]!)); break;
            case "reply": sink.Complete(msg); break;
            case "exit": runs.CompleteAdding(); return;
        }
    }
    runs.CompleteAdding();
}) { IsBackground = true, Name = "ara-run-stdin" };
reader.Start();

var runner = new ScriptRunner();
// Warm Roslyn and the load path once, so the first real script is fast (~1.3 s cold, R10).
await runner.Run("return 1;", new NullSink(), CancellationToken.None);
Send(new JsonObject { ["type"] = "ready" });

foreach (var (id, code) in runs.GetConsumingEnumerable())
{
    var r = await runner.Run(code, sink, CancellationToken.None);
    Send(new JsonObject
    {
        ["type"] = "done",
        ["id"] = id,
        ["ok"] = r.Ok,
        ["result"] = r.Result,
        ["why"] = r.Why,
        ["log"] = new JsonArray([.. r.Log.Select(l => (JsonNode)l)]),
        ["calls"] = r.Calls,
        ["diagnostics"] = r.Diagnostics,
    });
}
return 0;

/// <summary>Forwards a script's engine calls to the supervisor and waits for its reply.</summary>
sealed class StdioSink(Action<JsonObject> send) : ICommandSink
{
    readonly ConcurrentDictionary<long, TaskCompletionSource<CommandReply>> _pending = new();
    long _seq;

    /// <inheritdoc/>
    public Task<CommandReply> CallAsync(string command, JsonNode? parameters, CancellationToken cancel)
    {
        var seq = Interlocked.Increment(ref _seq);
        var tcs = new TaskCompletionSource<CommandReply>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[seq] = tcs;
        send(new JsonObject { ["type"] = "call", ["seq"] = seq, ["command"] = command, ["params"] = parameters?.DeepClone() });
        return tcs.Task;
    }

    /// <summary>Completes the call a reply answers.</summary>
    public void Complete(JsonObject reply)
    {
        if (!_pending.TryRemove((long)reply["seq"]!, out var tcs)) return;
        tcs.SetResult(new CommandReply((bool)reply["ok"]!, reply["output"]?.DeepClone(), (string?)reply["code"], (string?)reply["message"]));
    }
}

/// <summary>A sink for the warm-up run.</summary>
sealed class NullSink : ICommandSink
{
    /// <inheritdoc/>
    public Task<CommandReply> CallAsync(string command, JsonNode? parameters, CancellationToken cancel) =>
        Task.FromResult(new CommandReply(false, null, "ARA0963", "no supervisor"));
}

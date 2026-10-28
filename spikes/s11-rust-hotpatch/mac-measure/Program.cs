// Manual opt-in only: edits one body in an owned checkout and restores it.
// Uses the existing .NET 10 prerequisite; no NuGet dependencies.
using System.Diagnostics;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

if (args.Length is < 3 or > 4 || args[0] is not ("sim" or "editor"))
    throw new ArgumentException("sim|editor <root> <output> [project]");
var kind = args[0];
var root = Path.GetFullPath(args[1]);
var output = Path.GetFullPath(args[2]);
var project = args.Length == 4 ? Path.GetFullPath(args[3]) : "";
Directory.CreateDirectory(output);
var source = Path.Combine(root, kind == "sim"
    ? "crates/ara-hotpatch/sim-probe/main.rs" : "crates/ara-editor/src/shell/panels.rs");
var original = File.ReadAllText(source);
var old = kind == "sim" ? "transform.position[0] += 1.;" : "\"console\" => self.console_panel(editor, out),";
var replacement = kind == "sim" ? "transform.position[0] += 2.;"
    : "\"console\" => self.empty(out, \"rust-live\", \"terminal\", \"RUST PATCH LIVE\", \"The editor process stayed alive.\", &[]),";
if (original.Split(old).Length != 2) throw new InvalidOperationException("body anchor is not unique");
using var socket = new ClientWebSocket();
JsonNode? session = null;
var rpcId = 0;
var rows = new List<object>();
var daemonPath = Path.Combine(project, ".ara/daemon.json");
int hostPid = 0;
ulong entity = 0;
bool changed = false;
try
{
    var startup = Stopwatch.StartNew();
    if (kind == "editor")
    {
        while (!File.Exists(daemonPath))
        {
            if (startup.Elapsed.TotalMinutes > 40) throw new TimeoutException("editor startup");
            await Task.Delay(100);
        }
        var daemon = JsonNode.Parse(await File.ReadAllTextAsync(daemonPath))!;
        hostPid = daemon["pid"]!.GetValue<int>();
        socket.Options.SetRequestHeader("Authorization", "Bearer " + daemon["token"]!.GetValue<string>());
        await socket.ConnectAsync(new Uri($"ws://127.0.0.1:{daemon["port"]}/"), CancellationToken.None);
        session = (await Rpc("session.begin", new { client = "hotpatch-latency", author = "agent:hotpatch-latency" }))["session"];
        await Capture("editor", ".ara/hotpatch-proof/latency-baseline.png");
    }
    else
    {
        JsonNode? state;
        while ((state = ReadState()) is null || state["frame"]!.GetValue<ulong>() < 3)
        {
            if (startup.Elapsed.TotalMinutes > 30) throw new TimeoutException("sim startup");
            await Task.Delay(100);
        }
        hostPid = state["pid"]!.GetValue<int>();
        entity = state["entity"]!.GetValue<ulong>();
    }
    await Task.Delay(1000);
    for (int sample = 0; sample < 4; sample++)
    {
        changed = !changed;
        var generation = kind == "sim" ? ReadState()!["rust"]!["generation"]!.GetValue<ulong>() : 0;
        long saveUs = UnixUs();
        var timer = Stopwatch.StartNew();
        await File.WriteAllTextAsync(source, changed ? original.Replace(old, replacement) : original, new UTF8Encoding(false));
        JsonNode? state = null;
        while (true)
        {
            if (timer.Elapsed.TotalSeconds > 180) throw new TimeoutException($"patch {sample}");
            bool matches;
            if (kind == "editor")
            {
                var capture = await Capture("layout", $".ara/hotpatch-proof/latency-{sample}.json");
                matches = File.ReadAllText(capture["path"]!.GetValue<string>()).Contains("RUST PATCH LIVE") == changed;
            }
            else
            {
                state = ReadState();
                matches = state is not null && state["rust"]!["generation"]!.GetValue<ulong>() > generation
                    && state["delta"]!.GetValue<double>() == (changed ? 2 : 1);
                if (state is not null && state["pid"]!.GetValue<int>() != hostPid) throw new InvalidOperationException("host restarted");
                if (matches && (state!["entity"]!.GetValue<ulong>() != entity || !state["guards_verified"]!.GetValue<bool>()))
                    throw new InvalidOperationException("ECS state or determinism guards lost");
            }
            if (matches) break;
            await Task.Delay(40);
        }
        if (kind == "editor")
        {
            await Capture("editor", $".ara/hotpatch-proof/latency-{sample}.png");
            var daemon = JsonNode.Parse(await File.ReadAllTextAsync(daemonPath))!;
            if (daemon["pid"]!.GetValue<int>() != hostPid) throw new InvalidOperationException("editor restarted");
        }
        var row = new { kind, mode = "baseline", sample, configuration_first = sample == 0,
            save_us = saveUs, observed_us = UnixUs(), observed_ms = timer.Elapsed.TotalMilliseconds,
            pid = hostPid, changed, generation = state?["rust"]?["generation"]?.GetValue<ulong>() };
        rows.Add(row);
        Console.WriteLine(JsonSerializer.Serialize(row));
        await File.WriteAllTextAsync(Path.Combine(output, "latency.json"), JsonSerializer.Serialize(rows));
    }
}
finally
{
    if (File.ReadAllText(source) != original) File.WriteAllText(source, original, new UTF8Encoding(false));
    if (kind == "sim") File.WriteAllText(Path.Combine(output, "stop"), "stop");
    else if (session is not null && socket.State == WebSocketState.Open)
        await Rpc("editor.command", new { session, id = "file.exit" });
}

JsonNode? ReadState()
{
    try { return JsonNode.Parse(File.ReadAllText(Path.Combine(output, "status.json"))); }
    catch (IOException) { return null; }
    catch (JsonException) { return null; }
}
static long UnixUs() => (DateTime.UtcNow.Ticks - DateTime.UnixEpoch.Ticks) / 10;
async Task<JsonNode> Rpc(string method, object parameters)
{
    var id = ++rpcId;
    using var cancel = new CancellationTokenSource(TimeSpan.FromSeconds(30));
    var request = JsonSerializer.SerializeToUtf8Bytes(new { jsonrpc = "2.0", id, method, @params = parameters });
    await socket.SendAsync(new ArraySegment<byte>(request), WebSocketMessageType.Text, true, cancel.Token);
    while (true)
    {
        using var message = new MemoryStream();
        WebSocketReceiveResult received;
        do
        {
            var buffer = new byte[65536];
            received = await socket.ReceiveAsync(new ArraySegment<byte>(buffer), cancel.Token);
            if (received.MessageType == WebSocketMessageType.Close) throw new IOException("editor closed");
            message.Write(buffer, 0, received.Count);
        } while (!received.EndOfMessage);
        var response = JsonNode.Parse(message.ToArray())!;
        if (response["id"]?.GetValue<int>() != id) continue;
        if (response["error"] is not null) throw new IOException(response["error"]!.ToJsonString());
        return response["result"]!;
    }
}
async Task<JsonNode> Capture(string target, string path)
{
    var job = (await Rpc("capture", new { session, target, path }))["job"]!.DeepClone();
    var timer = Stopwatch.StartNew();
    while (true)
    {
        var status = await Rpc("jobs.status", new { session, job });
        var value = status["status"]!.GetValue<string>();
        if (value == "done") return status["result"]!;
        if (value != "running") throw new IOException(status.ToJsonString());
        if (timer.Elapsed.TotalSeconds > 60) throw new TimeoutException("capture");
        await Task.Delay(20);
    }
}

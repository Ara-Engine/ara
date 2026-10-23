using System.Text.Json.Nodes;
using Ara.Agent;

namespace Ara.RunWorker.Tests;

sealed class EchoSink : ICommandSink
{
    public readonly List<string> Calls = [];

    public Task<CommandReply> CallAsync(string command, JsonNode? parameters, CancellationToken cancel)
    {
        Calls.Add(command);
        return Task.FromResult(command == "fail"
            ? new CommandReply(false, null, "ARA1601", "unknown command")
            : new CommandReply(true, new JsonObject { ["echo"] = parameters?.DeepClone() }));
    }
}

public class WorkerTests
{
    static readonly ScriptRunner Runner = new();

    static RunResult Run(string code, EchoSink? sink = null) =>
        Runner.Run(code, sink ?? new EchoSink(), CancellationToken.None).GetAwaiter().GetResult();

    static string Codes(RunResult r) => string.Join(" ", r.Diagnostics.Select(d => (string?)d!["code"] + ": " + (string?)d["message"]));

    [Fact]
    public void Runs_scripts_against_the_facade()
    {
        var sink = new EchoSink();
        var r = Run("using System.Text;\nvar sb = new StringBuilder();\nvar got = await ara.Call(\"entity.create\", new { name = \"Crate\" });\nara.Why = \"test\";\nreturn new { names = new[] { \"a\", \"b\" }.Select(x => x.ToUpper()).ToList(), echoed = got![\"echo\"]![\"name\"]!.ToString() };", sink);
        Assert.True(r.Ok, Codes(r));
        Assert.Equal("""{"names":["A","B"],"echoed":"Crate"}""", r.Result!.ToJsonString());
        Assert.Equal(["entity.create"], sink.Calls);
        Assert.Equal("test", r.Why);
    }

    [Theory]
    [InlineData("return System.IO.File.Exists(\"x\");", "System.IO.File")]
    [InlineData("return System.Diagnostics.Process.GetCurrentProcess().Id;", "System.Diagnostics.Process")]
    [InlineData("return System.Environment.GetEnvironmentVariable(\"PATH\");", "System.Environment")]
    [InlineData("new System.Threading.Thread(() => { }).Start(); return 0;", "System.Threading.Thread")]
    [InlineData("return typeof(string).GetMethods().Length;", "System.Type.GetMethods")]
    [InlineData("return System.AppContext.BaseDirectory;", "System.AppContext")]
    [InlineData("return new System.Net.Http.HttpClient();", "System.Net.Http.HttpClient")]
    public void The_whitelist_refuses_ambient_authority(string code, string what)
    {
        var r = Run(code);
        Assert.False(r.Ok);
        Assert.Contains(r.Diagnostics, d => (string?)d!["code"] == "ARA0961" && ((string?)d["message"])!.Contains(what));
    }

    [Fact]
    public void Diagnostics_point_at_script_lines()
    {
        var r = Run("var a = 1;\nvar b = 2;\nreturn c;");
        Assert.False(r.Ok);
        Assert.Equal(3, (int?)r.Diagnostics[0]!["location"]!["line"]);
        var thrown = Run("var a = 1;\nthrow new InvalidOperationException(\"no\");");
        Assert.Contains("InvalidOperationException: no", Codes(thrown));
        var failed = Run("await ara.Call(\"fail\");");
        Assert.Contains("ARA1601", Codes(failed));
    }
}

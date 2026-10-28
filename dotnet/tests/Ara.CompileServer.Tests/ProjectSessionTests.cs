using System.Diagnostics;
using System.Text.Json;

namespace Ara.CompileServer.Tests;

public sealed class ProjectSessionTests
{
    [Fact]
    public async Task Warm_project_transport_rechecks_changes_and_serves_independent_absolute_roots()
    {
        var first = InstalledTools.Scratch("project-session-first");
        var second = InstalledTools.Scratch("project-session-second");
        Directory.CreateDirectory(Path.Combine(first, "content"));
        Directory.CreateDirectory(Path.Combine(second, "content"));
        var source = Path.Combine(first, "content", "Counter.cs");
        const string before = "using Ara; /// <summary>Counter in ECS.</summary>\n[Component] public partial struct Counter { /// <summary>Count.</summary>\n public int Value; }";
        File.WriteAllText(source, before);
        File.WriteAllText(Path.Combine(second, "content", "Other.cs"), "class Other { }");
        var tool = typeof(ProjectInventory).Assembly.Location;
        var start = new ProcessStartInfo("dotnet") {
            WorkingDirectory = Path.GetDirectoryName(tool)!, UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true,
        };
        foreach (var argument in new[] { "exec", tool, "--project-session", "--parent", System.Environment.ProcessId.ToString() }) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new InvalidOperationException("start project transport");
        var errors = process.StandardError.ReadToEndAsync();
        async Task<JsonDocument> Call(string root, string operation)
        {
            await process.StandardInput.WriteLineAsync(JsonSerializer.Serialize(new { project=root, authoringRoot="content", domain="gameplay", operation }));
            await process.StandardInput.FlushAsync();
            var line = await process.StandardOutput.ReadLineAsync().WaitAsync(TimeSpan.FromSeconds(60));
            Assert.False(line is null, process.HasExited ? await errors : "missing project response");
            return JsonDocument.Parse(line!);
        }
        try
        {
            using var initial = await Call(first, "--compile-project");
            Assert.Empty(initial.RootElement.GetProperty("diagnostics").EnumerateArray());
            File.WriteAllText(source, before.Replace("public int Value;", "public int Value; /// <summary>Ratio.</summary>\n public float Ratio;", StringComparison.Ordinal));
            using var changed = await Call(first, "--compile-project");
            var game = changed.RootElement.GetProperty("modules").EnumerateArray().Single(m => m.GetProperty("name").GetString() == "game");
            Assert.Equal("compiled", game.GetProperty("result").GetString());
            Assert.True(game.GetProperty("schema").GetProperty("types").GetProperty("Counter").GetProperty("fields").TryGetProperty("ratio", out _));
            using var other = await Call(second, "--inventory");
            Assert.Equal("Other.cs", other.RootElement.GetProperty("sources")[0].GetProperty("path").GetString());
        }
        finally
        {
            process.StandardInput.Close();
            try { await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(30)); }
            catch { if (!process.HasExited) process.Kill(); throw; }
        }
        Assert.Equal(0, process.ExitCode);
    }
}

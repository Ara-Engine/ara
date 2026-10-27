using System.Diagnostics;
using System.Text.Json;

namespace Ara.CompileServer.Tests;

public sealed class InstalledFixTests
{
    [Fact]
    public async Task Installed_layout_fixes_Unity_state_and_API_diagnostics_without_a_checkout()
    {
        var source = Path.GetDirectoryName(typeof(AnalyzerSet).Assembly.Location)!;
        var repo = new DirectoryInfo(source);
        while (repo is not null && !File.Exists(Path.Combine(repo.FullName, "Cargo.toml"))) repo = repo.Parent;
        Assert.NotNull(repo);
        var stage = Path.Combine(repo.FullName, "target", "dotnet-tests", "installed-fixes-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(stage);
        foreach (var file in Directory.GetFiles(source)) File.Copy(file, Path.Combine(stage, Path.GetFileName(file)));
        var start = new ProcessStartInfo("dotnet")
        {
            WorkingDirectory = stage, RedirectStandardInput = true, RedirectStandardOutput = true,
            RedirectStandardError = true, UseShellExecute = false, CreateNoWindow = true,
        };
        start.ArgumentList.Add("exec");
        start.ArgumentList.Add(Path.Combine(stage, "Ara.CompileServer.dll"));
        start.ArgumentList.Add("--fix");
        using var process = Process.Start(start)!;
        if (OperatingSystem.IsWindows()) process.PriorityClass = ProcessPriorityClass.BelowNormal;
        var request = new
        {
            files = new[] { new { path = "Game.cs", text = "using Ara; using System.Numerics; public partial class Door : Behaviour { int cache; protected override void OnTick(in Tick tick) { var v = Vector3.Zero; var x = v.x; System.Console.WriteLine(x); } }" } },
            codes = new[] { "ARA0001", "ARA0101", "ARA0506" },
        };
        await process.StandardInput.WriteAsync(JsonSerializer.Serialize(request));
        process.StandardInput.Close();
        var stdout = process.StandardOutput.ReadToEndAsync();
        var stderr = process.StandardError.ReadToEndAsync();
        try { await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(60)); }
        catch { process.Kill(); throw; }
        Assert.True(process.ExitCode == 0, await stderr);
        using var result = JsonDocument.Parse(await stdout);
        var change = Assert.Single(result.RootElement.GetProperty("changes").EnumerateArray());
        var after = change.GetProperty("after").GetString()!;
        Assert.Contains("v.X", after);
        Assert.Contains("[Transient]", after);
        Assert.Contains("Log.Info", after);
        Assert.Equal(new[] { "ARA0001", "ARA0101", "ARA0506" },
            change.GetProperty("fixes").EnumerateArray().Select(x => x.GetProperty("code").GetString()).Distinct().Order().ToArray());
        Assert.Equal(request.files[0].text, change.GetProperty("before").GetString());
        Assert.True(File.Exists(Path.Combine(stage, "Ara.Analyzers.CodeFixes.dll")));
        Assert.False(File.Exists(Path.Combine(stage, "Game.cs")), "the worker proposes changes; authoring applies them through commands");
    }
}

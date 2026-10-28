using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>Runs the installed tool layout with no symbols or checkout probing.</summary>
internal static class InstalledTools
{
    internal static void AssertNoProblems(IEnumerable<JsonElement> rows) =>
        Assert.DoesNotContain(rows, row => row.GetProperty("code").GetString() != AraCodes.CompileResult);

    internal static void AssertNoProblems(IEnumerable<JsonObject> rows) =>
        Assert.DoesNotContain(rows, row => (string?)row["code"] != AraCodes.CompileResult);
    internal static string Repo
    {
        get
        {
            var dir = new DirectoryInfo(AppContext.BaseDirectory);
            while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "Cargo.toml"))) dir = dir.Parent;
            return dir?.FullName ?? throw new InvalidOperationException("test checkout not found");
        }
    }

    internal static string Scratch(string name)
    {
        var target = System.Environment.GetEnvironmentVariable("ARA_TEST_ROOT") ?? Path.Combine(Repo, "target", "dotnet-tests");
        var path = Path.Combine(target, name + "-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    internal static async Task<JsonDocument> Run(string mode, object request)
    {
        var source = Path.Combine(Repo, "dotnet", "artifacts", "bin", "Ara.CompileServer", "release");
        var stage = Path.Combine(Scratch("installed-tool"), "engine", "dotnet", "artifacts", "bin", "Ara.CompileServer", "release");
        Directory.CreateDirectory(stage);
        foreach (var file in Directory.EnumerateFiles(source, "*", SearchOption.AllDirectories).Where(p => !p.EndsWith(".pdb", StringComparison.Ordinal)))
        {
            var destination = Path.Combine(stage, Path.GetRelativePath(source, file));
            Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
            File.Copy(file, destination);
        }
        var start = new ProcessStartInfo("dotnet") { WorkingDirectory = stage, RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false, CreateNoWindow = true };
        start.ArgumentList.Add("exec");
        start.ArgumentList.Add(Path.Combine(stage, "Ara.CompileServer.dll"));
        start.ArgumentList.Add(mode);
        using var process = Process.Start(start)!;
        if (OperatingSystem.IsWindows()) process.PriorityClass = ProcessPriorityClass.BelowNormal;
        await process.StandardInput.WriteAsync(JsonSerializer.Serialize(request));
        process.StandardInput.Close();
        var stdout = process.StandardOutput.ReadToEndAsync();
        var stderr = process.StandardError.ReadToEndAsync();
        try { await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(60)); }
        catch { process.Kill(); throw; }
        Assert.True(process.ExitCode == 0, await stderr);
        return JsonDocument.Parse(await stdout);
    }
}

// One-command launch-to-first-tick and save-to-run timing for the C# scripting loop, on any OS.
//
//   dotnet run benchmarks/iteration/reload_timing.cs
//   dotnet run benchmarks/iteration/reload_timing.cs -- --runs 20 --edits 45 --lines 5000
//   dotnet run benchmarks/iteration/reload_timing.cs -- --skip-build --variant before=C:\sdk-before --variant after=C:\sdk-after
//
// It builds dotnet/Ara.slnx (Release) and the `reload_bench` example unless --skip-build, then runs
// `reload_bench` in fresh processes (variants in mirrored order, A B B A, so drift hits all of them alike) and
// prints medians and 95th percentiles of launch-to-first-tick ("start + initial compile") and of save -> visible
// for the three edit levels: L1 (a body), L2 (a new member), L3 (a signature change), plus the phases behind
// them (emit, analyzers, the Windows AMSI image scan, generators). Everything lands in --output (default
// target/reload-timing/<utc stamp>): one raw JSON per run, summary.json and summary.txt.
//
// Needs the .NET 10 SDK and Rust, nothing else. A variant is the absolute path of a built `dotnet/artifacts/bin`
// (the default, "current", is this checkout's). Run it on a quiet machine: other builds in the background move
// every number; the summary records the CPU busy fraction the bench measured (Windows) so a noisy run shows.
using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

var options = Options.Parse(args);
var root = Repo.Root();
var exe = Path.Combine(root, "target", "release", "examples", OperatingSystem.IsWindows() ? "reload_bench.exe" : "reload_bench");

if (!options.SkipBuild)
{
    Repo.Run("dotnet", ["build", Path.Combine("dotnet", "Ara.slnx"), "-c", "Release", "-m:2", "--disable-build-servers", "--nologo", "-v", "q"], root);
    Repo.Run("cargo", ["build", "-p", "ara-script", "--release", "--example", "reload_bench"], root, ("CARGO_BUILD_JOBS", options.CargoJobs));
}
if (!File.Exists(exe)) throw new FileNotFoundException($"{exe} is missing; run without --skip-build");

var variants = options.Variants.Count > 0 ? options.Variants
    : [("current", Path.Combine(root, "dotnet", "artifacts", "bin"))];
foreach (var (name, sdk) in variants)
    if (!File.Exists(Path.Combine(sdk, "Ara.CompileServer", "release", "Ara.CompileServer.dll")))
        throw new DirectoryNotFoundException($"variant {name}: no Ara.CompileServer under {sdk}");

Directory.CreateDirectory(options.Output);
var order = variants.Concat(Enumerable.Reverse(variants)).ToArray();
var runs = new List<(string Variant, JsonNode Result)>();
for (var round = 0; round < options.Runs; round += 2)
{
    var index = 0;
    foreach (var (name, sdk) in order)
    {
        if (runs.Count(r => r.Variant == name) >= options.Runs) { index++; continue; }
        var file = Path.Combine(options.Output, $"run-{round:D3}-{index++}-{name}.json");
        Console.Write($"run {runs.Count + 1}/{options.Runs * variants.Count} ({name}) ... ");
        var code = Repo.Run(exe, ["--dotnet-bin", sdk, "--edits", options.Edits.ToString(CultureInfo.InvariantCulture),
            "--lines", options.Lines.ToString(CultureInfo.InvariantCulture), "--output", file], root, check: false, quiet: true);
        var result = File.Exists(file) ? JsonNode.Parse(File.ReadAllText(file)) : null;
        if (code != 0 || result is null || result["success"]?.GetValue<bool>() != true)
        {
            Console.WriteLine("FAILED");
            Console.Error.WriteLine($"  exit {code}; failures: {result?["failures"]?.ToJsonString() ?? "no output"}");
            continue;
        }
        Console.WriteLine($"{result["startup_ms"]!.GetValue<double>():F0} ms to first tick");
        runs.Add((name, result));
    }
}
if (runs.Count == 0) throw new InvalidOperationException("no run succeeded");

var text = new StringBuilder();
var summary = new JsonObject
{
    ["environment"] = Repo.Environment(root),
    ["settings"] = new JsonObject { ["runs_per_variant"] = options.Runs, ["edits_per_run"] = options.Edits, ["filler_lines"] = options.Lines },
};
text.AppendLine($"{summary["environment"]!["os"]}  |  {summary["environment"]!["cpu"]}  |  dotnet {summary["environment"]!["dotnet"]}  |  {summary["environment"]!["commit"]}");
text.AppendLine($"{options.Runs} fresh processes per variant, {options.Edits} edits each; median / p95 in ms");
var cells = new JsonObject();
foreach (var (name, _) in variants)
{
    var mine = runs.Where(r => r.Variant == name).Select(r => r.Result).ToArray();
    if (mine.Length == 0) continue;
    var startup = mine.Select(r => r["startup_ms"]!.GetValue<double>()).ToArray();
    var entry = new JsonObject { ["runs"] = mine.Length, ["launch_to_first_tick_ms"] = Stats.Node(startup),
        ["host_start_ms"] = Stats.Node(mine.Select(r => r["host_start_ms"]?.GetValue<double>() ?? double.NaN).ToArray()) };
    var busy = mine.Select(r => r["cpu_busy_fraction"]?.GetValue<double>() ?? double.NaN).Where(v => !double.IsNaN(v)).ToArray();
    if (busy.Length > 0) entry["system_cpu_busy_fraction_median"] = Stats.Median(busy);
    text.AppendLine().AppendLine($"[{name}]  launch to first tick: {Stats.Format(startup)}   (host start {Stats.Format(mine.Select(r => r["host_start_ms"]?.GetValue<double>() ?? double.NaN).ToArray())})");
    if (busy.Length > 0) text.AppendLine($"  system CPU busy during runs (median): {Stats.Median(busy) * 100:F0}%");
    text.AppendLine($"  {"level",-6}{"n",5}  {"save->visible",-16}{"emit",-14}{"analyzers",-14}{"generators",-14}{"AMSI load",-14}{"register",-14}");
    foreach (var (level, label) in new[] { (1, "L1"), (2, "L2"), (3, "L3") })
    {
        var edits = mine.SelectMany(r => r["edits"]!.AsArray()).Where(e => e!["warmup"]!.GetValue<bool>() == false && e["observed_kind"]?.GetValue<int>() == level
            && e["save_to_visible_ms"] is not null && e["report"] is not null).ToArray();
        if (edits.Length == 0) continue;
        double[] Phase(string key) => edits.Select(e => e!["report"]![key]!.GetValue<double>() / 1000).ToArray();
        var total = edits.Select(e => e!["save_to_visible_ms"]!.GetValue<double>()).ToArray();
        entry[label] = new JsonObject { ["n"] = edits.Length, ["save_to_visible_ms"] = Stats.Node(total), ["emit_ms"] = Stats.Node(Phase("emit_us")),
            ["analyzers_ms"] = Stats.Node(Phase("analyze_us")), ["generators_ms"] = Stats.Node(Phase("generate_us")),
            ["amsi_load_ms"] = Stats.Node(Phase("load_us")), ["register_ms"] = Stats.Node(Phase("register_us")) };
        text.AppendLine($"  {label,-6}{edits.Length,5}  {Stats.Format(total),-16}{Stats.Format(Phase("emit_us")),-14}{Stats.Format(Phase("analyze_us")),-14}{Stats.Format(Phase("generate_us")),-14}{Stats.Format(Phase("load_us")),-14}{Stats.Format(Phase("register_us")),-14}");
    }
    cells[name] = entry;
}
summary["variants"] = cells;
File.WriteAllText(Path.Combine(options.Output, "summary.json"), summary.ToJsonString(new JsonSerializerOptions { WriteIndented = true }));
File.WriteAllText(Path.Combine(options.Output, "summary.txt"), text.ToString());
Console.WriteLine();
Console.Write(text);
Console.WriteLine($"\nraw runs and summary: {options.Output}");

/// <summary>Command line.</summary>
sealed record Options(int Runs, int Edits, int Lines, bool SkipBuild, string CargoJobs, string Output, List<(string Name, string Sdk)> Variants)
{
    internal static Options Parse(string[] args)
    {
        int runs = 10, edits = 45, lines = 5000;
        var skip = false;
        var jobs = "2";
        var output = "";
        var variants = new List<(string, string)>();
        for (var i = 0; i < args.Length; i++)
        {
            string Next() => i + 1 < args.Length ? args[++i] : throw new ArgumentException($"{args[i]} needs a value");
            switch (args[i])
            {
                case "--runs": runs = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "--edits": edits = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "--lines": lines = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "--cargo-jobs": jobs = Next(); break;
                case "--skip-build": skip = true; break;
                case "--output": output = Path.GetFullPath(Next()); break;
                case "--variant":
                    var spec = Next();
                    var at = spec.IndexOf('=');
                    if (at <= 0) throw new ArgumentException("--variant takes name=/absolute/path/to/artifacts/bin");
                    variants.Add((spec[..at], Path.GetFullPath(spec[(at + 1)..])));
                    break;
                default: throw new ArgumentException($"unknown option {args[i]} (see the header of this file)");
            }
        }
        if (edits < 6) throw new ArgumentException("--edits must be at least 6 so every level is measured");
        output = output.Length > 0 ? output : Path.Combine(Repo.Root(), "target", "reload-timing", DateTime.UtcNow.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture));
        return new(Math.Max(runs, 1), edits, lines, skip, jobs, output, variants);
    }
}

/// <summary>Repository and process helpers.</summary>
static class Repo
{
    internal static string Root()
    {
        for (var dir = new DirectoryInfo(Directory.GetCurrentDirectory()); dir is not null; dir = dir.Parent)
            if (File.Exists(Path.Combine(dir.FullName, "dotnet", "Ara.slnx"))) return dir.FullName;
        throw new InvalidOperationException("run this from inside the Ara checkout");
    }

    /// <summary>Runs a program and returns its exit code; throws on failure when <paramref name="check"/>.</summary>
    internal static int Run(string program, string[] args, string cwd, (string Key, string Value)? env = null, bool check = true, bool quiet = false)
    {
        var psi = new ProcessStartInfo(program) { WorkingDirectory = cwd, UseShellExecute = false, RedirectStandardOutput = quiet, RedirectStandardError = quiet };
        foreach (var arg in args) psi.ArgumentList.Add(arg);
        if (env is { } pair) psi.Environment[pair.Key] = pair.Value;
        using var process = Process.Start(psi) ?? throw new InvalidOperationException($"cannot start {program}");
        if (quiet) { _ = process.StandardOutput.ReadToEndAsync(); _ = process.StandardError.ReadToEndAsync(); }
        process.WaitForExit();
        if (check && process.ExitCode != 0) throw new InvalidOperationException($"{program} {string.Join(' ', args)} exited with {process.ExitCode}");
        return process.ExitCode;
    }

    static string Capture(string program, params string[] args)
    {
        try
        {
            var psi = new ProcessStartInfo(program) { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
            foreach (var arg in args) psi.ArgumentList.Add(arg);
            using var process = Process.Start(psi)!;
            var output = process.StandardOutput.ReadToEnd().Trim();
            process.WaitForExit();
            return output;
        }
        catch (Exception) { return "unknown"; }
    }

    internal static JsonObject Environment(string root) => new()
    {
        ["os"] = System.Runtime.InteropServices.RuntimeInformation.OSDescription,
        ["cpu"] = $"{System.Environment.ProcessorCount} logical cores, {System.Runtime.InteropServices.RuntimeInformation.ProcessArchitecture}",
        ["dotnet"] = Capture("dotnet", "--version"),
        ["commit"] = Capture("git", "-C", root, "rev-parse", "--short", "HEAD"),
        ["dirty"] = Capture("git", "-C", root, "status", "--porcelain").Length > 0,
        ["utc"] = DateTime.UtcNow.ToString("O", CultureInfo.InvariantCulture),
    };
}

/// <summary>Median and 95th percentile (nearest rank on the sorted samples).</summary>
static class Stats
{
    internal static double Percentile(double[] values, double p)
    {
        var sorted = values.Where(v => !double.IsNaN(v)).Order().ToArray();
        return sorted.Length == 0 ? double.NaN : sorted[(int)Math.Round((sorted.Length - 1) * p)];
    }

    internal static double Median(double[] values) => Percentile(values, 0.5);
    internal static string Format(double[] values) => $"{Median(values):F1} / {Percentile(values, 0.95):F1}";
    static JsonNode? Number(double value) => double.IsNaN(value) ? null : value;
    internal static JsonObject Node(double[] values) => new() { ["median"] = Number(Median(values)), ["p95"] = Number(Percentile(values, 0.95)), ["n"] = values.Length };
}

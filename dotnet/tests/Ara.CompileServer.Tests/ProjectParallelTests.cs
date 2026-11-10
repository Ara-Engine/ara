using System.Diagnostics;
using System.Text.Json;

namespace Ara.CompileServer.Tests;

public sealed class ProjectParallelTests
{
    static readonly CompileRequestContext Context = new("project", "parallel-test", 1, 1);

    [Fact]
    public void Dependency_frontiers_keep_identical_images_schemas_and_diagnostic_order_after_edits()
    {
        var root = InstalledTools.Scratch("parallel-project");
        Write(root, "Base", Code("Base", "1"));
        Write(root, "Left", Code("Left", "ApiBase.Value + 1"), "Base");
        Write(root, "Right", Code("Right", "ApiBase.Value + 2"), "Base");
        Write(root, "Game", Code("Game", "ApiLeft.Value + ApiRight.Value"), "Left", "Right");
        Write(root, "Independent", Code("Independent", "7"));
        var sequential = new ProjectCompilations();
        var parallel = new ProjectCompilations();
        for (var revision = 0; revision < 3; revision++)
        {
            Write(root, "Base", Code("Base", (revision + 1).ToString(System.Globalization.CultureInfo.InvariantCulture)));
            var expected = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context), compilations: sequential, maxParallelism: 1);
            InstalledTools.AssertNoProblems(expected.Diagnostics);
            var images = expected.Modules.Select(m => (Pe: File.ReadAllBytes(Path.Combine(root, m.AssemblyPath)), Pdb: File.ReadAllBytes(Path.Combine(root, Path.ChangeExtension(m.AssemblyPath, ".pdb"))))).ToArray();
            var actual = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context), compilations: parallel, maxParallelism: 2);
            Assert.Equal(expected.Modules.Select(m => (m.Name, m.Result, m.Fingerprint, Schema: m.Schema?.ToJsonString())),
                actual.Modules.Select(m => (m.Name, m.Result, m.Fingerprint, Schema: m.Schema?.ToJsonString())));
            Assert.Equal(expected.Diagnostics.Select(d => d.ToJsonString()), actual.Diagnostics.Select(d => d.ToJsonString()));
            for (var i = 0; i < actual.Modules.Length; i++)
            {
                Assert.Equal(images[i].Pe, File.ReadAllBytes(Path.Combine(root, actual.Modules[i].AssemblyPath)));
                Assert.Equal(images[i].Pdb, File.ReadAllBytes(Path.Combine(root, Path.ChangeExtension(actual.Modules[i].AssemblyPath, ".pdb"))));
            }
        }
        Probe();
    }

    [Fact]
    public void Broken_dependency_keeps_its_dependents_last_good_while_independent_work_completes()
    {
        var root = InstalledTools.Scratch("parallel-dependency-failure");
        Write(root, "Base", Code("Base", "1"));
        Write(root, "Game", Code("Game", "ApiBase.Value"), "Base");
        Write(root, "Independent", Code("Independent", "1"));
        var cache = new ProjectCompilations();
        var initial = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context), compilations: cache);
        InstalledTools.AssertNoProblems(initial.Diagnostics);
        var images = initial.Modules.ToDictionary(m => m.Name, m => File.ReadAllBytes(Path.Combine(root, m.AssemblyPath)));
        Write(root, "Base", Code("Base", "Missing"));
        Write(root, "Independent", Code("Independent", "2"));
        var sequential = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context), maxParallelism: 1);
        var failed = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context), compilations: cache, maxParallelism: 2);
        Assert.Equal(sequential.Diagnostics.Select(d => d.ToJsonString()), failed.Diagnostics.Select(d => d.ToJsonString()));
        foreach (var module in failed.Modules)
        {
            Assert.Equal(module.Name == "Independent" ? "compiled" : "failed", module.Result);
            if (module.Name != "Independent") Assert.Equal(images[module.Name], File.ReadAllBytes(Path.Combine(root, module.AssemblyPath)));
        }
        var blocked = failed.Diagnostics.Last(d => (string?)d["data"]?["module"] == "Game");
        Assert.Equal("not-evaluated", (string?)blocked["data"]?["compile"]?["validation"]);
        Write(root, "Base", Code("Base", "3"));
        InstalledTools.AssertNoProblems(ProjectCompiler.Compile(new(root, "content"), compilations: cache).Diagnostics);
    }

    [Fact]
    public void Gameplay_preparation_does_not_schedule_editor_domains()
    {
        var root = InstalledTools.Scratch("parallel-domains");
        Write(root, "Core", Code("Core", "1"));
        Write(root, "Game", Code("Game", "ApiCore.Value"), "Core");
        var editor = Path.Combine(root, "content", "game", "editor");
        Directory.CreateDirectory(editor);
        File.WriteAllText(Path.Combine(editor, "Tools.cs"), "public static class Tools { public static int Value => ApiGame.Value; }");
        var gameplay = ProjectCompiler.Compile(new(root, "content", DiagnosticContext: Context));
        InstalledTools.AssertNoProblems(gameplay.Diagnostics);
        var tools = Assert.Single(gameplay.Modules, m => m.Profile == "editor");
        Assert.Equal("unbuilt", tools.Result);
        Assert.False(File.Exists(Path.Combine(root, tools.AssemblyPath)));
        var all = ProjectCompiler.Compile(new(root, "content", "editor", Context));
        InstalledTools.AssertNoProblems(all.Diagnostics);
        Assert.All(all.Modules, m => Assert.Equal("compiled", m.Result));
    }

    static string Code(string name, string expression) => $$"""
        using Ara;
        /// <summary>State emitted by {{name}}.</summary>
        [Component] public partial struct State{{name}} {
            /// <summary>Current value.</summary>
            public float Value;
        }
        public static class Api{{name}} { public static int Value => {{expression}}; }
        """;

    static void Write(string root, string name, string source, params string[] references)
    {
        var directory = Path.Combine(root, "content", name.ToLowerInvariant());
        Directory.CreateDirectory(directory);
        File.WriteAllText(Path.Combine(directory, name.ToLowerInvariant() + ".module.json"), JsonSerializer.Serialize(new { name, references }));
        File.WriteAllText(Path.Combine(directory, "Code.cs"), source);
    }

    static string ProbeCode(int module, int file, int revision) => $$"""
        using Ara;
        namespace Module{{module}};
        /// <summary>State for module {{module}}, file {{file}}.</summary>
        [Component] public partial struct State{{file}} {
            /// <summary>Current amount.</summary>
            public float Amount;
            /// <summary>Current velocity.</summary>
            public System.Numerics.Vector3 Velocity;
        }
        /// <summary>Updates one generated component column.</summary>
        public static partial class Update{{file}} {
            [Query] static void Run(ref State{{file}} state, in Tick tick) {
                state.Amount += tick.Dt * {{revision + 1}}.0f;
            }
        }
        """;

    // Explicit opt-in keeps scaling work outside ordinary unit tests. All artifacts
    // and authored probe files belong to the caller's target scratch directory.
    static void Probe()
    {
        var output = System.Environment.GetEnvironmentVariable("ARA_PROJECT_PARALLEL_BENCH_OUTPUT");
        if (output is null) return;
        var root = InstalledTools.Scratch("project-parallel-probe");
        var cases = new List<object>();
        foreach (var workers in new[] { 1, 2, 4 })
        {
            var cache = new ProjectCompilations();
            for (var module = 0; module < 32; module++)
            {
                var name = "Module" + module;
                Write(root, name, ProbeCode(module, 0, 0));
                for (var file = 1; file < 32; file++)
                    File.WriteAllText(Path.Combine(root, "content", name.ToLowerInvariant(), $"State{file}.cs"), ProbeCode(module, file, 0));
            }
            var samples = new List<double>();
            for (var revision = 0; revision < 9; revision++)
            {
                for (var module = 0; module < 32; module++) Write(root, "Module" + module, ProbeCode(module, 0, revision));
                var start = Stopwatch.GetTimestamp();
                var result = ProjectCompiler.Compile(new(root, "content"), compilations: cache, maxParallelism: workers);
                var elapsed = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
                InstalledTools.AssertNoProblems(result.Diagnostics);
                Assert.All(result.Modules, m => Assert.Equal("compiled", m.Result));
                if (revision >= 3) samples.Add(elapsed);
            }
            var ordered = samples.Order().ToArray();
            cases.Add(new { workers, p50_ms = (ordered[2] + ordered[3]) / 2, p95_ms = ordered[5], samples_ms = samples });
        }
        File.WriteAllText(output, JsonSerializer.Serialize(new { runtime = System.Environment.Version.ToString(), modules = 32, files_per_module = 32, cases }, new JsonSerializerOptions { WriteIndented = true }));
    }
}

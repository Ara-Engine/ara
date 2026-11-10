using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class ProjectCompilationsTests
{
    [Theory]
    [InlineData("gameplay")]
    [InlineData("editor")]
    public void Batched_project_analysis_matches_the_existing_per_file_diagnostics(string profile)
    {
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        var result = new ProjectCompilations().Generate("game.dll", "config", compiler, false,
            [("A.cs", "public static class A { public static int Value; }"),
             ("B.cs", "public static class B { public static long Read() => System.DateTime.Now.Ticks; }")], []);
        var analysis = new AnalyzerSet();
        static string[] Keys(IEnumerable<Microsoft.CodeAnalysis.Diagnostic> diagnostics) => diagnostics
            .Select(d => $"{d.Id}:{d.Location.SourceTree?.FilePath}:{d.Location.SourceSpan}:{d.GetMessage()}")
            .Order(StringComparer.Ordinal).ToArray();
        var expected = analysis.Analyze(result.Final, result.Trees, profile);
        if (profile == "gameplay") Assert.NotEmpty(expected);
        Assert.Equal(Keys(expected), Keys(analysis.AnalyzeProject(result.Final, profile)));
    }

    [Fact]
    public void Unchanged_sources_keep_their_syntax_trees_across_configuration_changes()
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        var first = cache.Generate("game.dll", "config-1", compiler, false,
            [("A.cs", "class A { }"), ("B.cs", "class B { }")], []);
        var changed = cache.Generate("game.dll", "config-1", compiler, false,
            [("A.cs", "class A { public int Value => 1; }"), ("B.cs", "class B { }")], []);
        Assert.NotSame(first.Trees[0], changed.Trees[0]);
        Assert.Same(first.Trees[1], changed.Trees[1]);
        var configured = cache.Generate("game.dll", "config-2", compiler, true,
            [("A.cs", "class A { public int Value => 1; }"), ("B.cs", "class B { }")], []);
        Assert.Same(changed.Trees[0], configured.Trees[0]);
        Assert.Same(changed.Trees[1], configured.Trees[1]);
        Assert.Equal(Microsoft.CodeAnalysis.OptimizationLevel.Debug, configured.Final.Options.OptimizationLevel);
    }

    [Fact]
    public void Unchanged_consumer_text_rebinds_changed_references_and_emits_fresh_build_bytes()
    {
        var cache = new ProjectCompilations();
        var coreCompiler = new ModuleCompiler("Core", [typeof(Behaviour).Assembly.Location]);
        var consumerCompiler = new ModuleCompiler("Consumer", [typeof(Behaviour).Assembly.Location]);
        (string Path, string Text)[] sources = [("Code.cs", "public static class Consumer { public static int Read() => Core.Value; }")];
        Microsoft.CodeAnalysis.SyntaxTree? priorTree = null;
        foreach (var (type, value, hot) in new[] { ("int", "1", false), ("string", "\"bad\"", false), ("int", "3", true) })
        {
            var core = coreCompiler.Create([ModuleCompiler.Parse("Core.cs", $"public static class Core {{ public const {type} Value = {value}; }}")], false, []);
            var coreImage = ModuleCompiler.Emit(core).Image;
            Assert.NotNull(coreImage);
            Microsoft.CodeAnalysis.MetadataReference[] references = [Microsoft.CodeAnalysis.MetadataReference.CreateFromImage(coreImage.Pe)];
            var configuration = type + value + hot;
            var reused = cache.Generate("consumer.dll", configuration, consumerCompiler, hot, sources, references);
            var fresh = new ProjectCompilations().Generate("consumer.dll", configuration, consumerCompiler, hot, sources, references);
            if (priorTree is not null) Assert.Same(priorTree, reused.Trees[0]);
            priorTree = reused.Trees[0];
            var actual = ModuleCompiler.Emit(reused.Final);
            var expected = ModuleCompiler.Emit(fresh.Final);
            Assert.Equal(expected.Errors.Select(error => error.ToString()), actual.Errors.Select(error => error.ToString()));
            if (type == "string")
            {
                Assert.Null(actual.Image);
                Assert.Contains(actual.Errors, error => error.Id == "CS0029");
            }
            else
            {
                Assert.NotNull(actual.Image);
                Assert.NotNull(expected.Image);
                Assert.Equal(expected.Image.Pe, actual.Image.Pe);
                Assert.Equal(expected.Image.Pdb, actual.Image.Pdb);
            }
            Assert.Equal(fresh.Driver.GetRunResult().Results.SelectMany(result => result.GeneratedSources).Select(source => source.SourceText.ToString()),
                reused.Driver.GetRunResult().Results.SelectMany(result => result.GeneratedSources).Select(source => source.SourceText.ToString()));
        }
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Replacing_all_sources_preserves_fresh_outputs_and_recovers_from_errors(bool hot)
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        for (var revision = 0; revision < 5; revision++)
        {
            var expression = revision == 3 ? "MissingOther" : revision.ToString(System.Globalization.CultureInfo.InvariantCulture);
            var value = revision == 3 ? "Missing" : revision.ToString(System.Globalization.CultureInfo.InvariantCulture);
            (string Path, string Text)[] sources = [
                ("Core.cs", $"public static class Core {{ public const float Value = {value}; }}"),
                ("State.cs", $$"""
                    /// <summary>State revision {{revision}}.</summary>
                    [Component] public partial struct State {
                        /// <summary>Accumulated amount.</summary>
                        public float Amount;
                    }
                    """),
                ("Update.cs", $$"""
                    public static partial class Update {
                        [Query] static void Run(ref State state) { state.Amount += Core.Value + {{expression}}; }
                    }
                    """)];
            var actual = cache.Generate("game.dll", "config", compiler, hot, sources, []);
            var fresh = new ProjectCompilations().Generate("game.dll", "config", compiler, hot, sources, []);
            Assert.Equal(fresh.Driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources).Select(s => s.SourceText.ToString()),
                actual.Driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources).Select(s => s.SourceText.ToString()));
            static string[] Keys(IEnumerable<Microsoft.CodeAnalysis.Diagnostic> rows) => rows
                .Select(d => d.ToString()).Order(StringComparer.Ordinal).ToArray();
            Assert.Equal(Keys(fresh.Diagnostics), Keys(actual.Diagnostics));
            var analysis = new AnalyzerSet();
            Assert.Equal(Keys(analysis.AnalyzeProject(fresh.Final, "gameplay")), Keys(analysis.AnalyzeProject(actual.Final, "gameplay")));
            var expected = ModuleCompiler.EmitWithDiagnostics(fresh.Final);
            var emitted = ModuleCompiler.EmitWithDiagnostics(actual.Final);
            Assert.Equal(Keys(expected.Diagnostics), Keys(emitted.Diagnostics));
            if (revision == 3)
            {
                Assert.Null(emitted.Image);
                Assert.Contains(emitted.Diagnostics, d => d.Id == "CS0103" && d.Severity == Microsoft.CodeAnalysis.DiagnosticSeverity.Error);
            }
            else
            {
                Assert.NotNull(emitted.Image);
                Assert.NotNull(expected.Image);
                Assert.Equal(expected.Image.Pe, emitted.Image.Pe);
                Assert.Equal(expected.Image.Pdb, emitted.Image.Pdb);
            }
        }
    }

    [Fact]
    public void Reused_projects_recheck_dependencies_and_recover_after_errors_and_file_removal()
    {
        var root = InstalledTools.Scratch("project-compilation-cache");
        var core = Path.Combine(root, "content", "core");
        Directory.CreateDirectory(core);
        File.WriteAllText(Path.Combine(core, "core.module.json"), "{\"name\":\"core\"}");
        File.WriteAllText(Path.Combine(root, "content", "game.module.json"), "{\"name\":\"game\",\"references\":[\"core\"]}");
        var dependency = Path.Combine(core, "Core.cs");
        var game = Path.Combine(root, "content", "Game.cs");
        File.WriteAllText(dependency, "public static class Core { public static int Value => 1; }");
        File.WriteAllText(game, "public static class Game { public static int Value => Core.Value; }");
        var cache = new ProjectCompilations();
        ProjectInventoryResult Compile() => ProjectCompiler.Compile(new(root, "content"), writeOutputs: false, compilations: cache);
        InstalledTools.AssertNoProblems(Compile().Diagnostics);
        File.WriteAllText(dependency, "public static class Core { public static int Other => 2; }");
        Assert.Equal("failed", Compile().Modules.Single(m => m.Name == "game").Result);
        File.WriteAllText(game, "public static class Game { public static int Value => Core.Other; }");
        InstalledTools.AssertNoProblems(Compile().Diagnostics);
        File.Delete(game);
        File.WriteAllText(Path.Combine(root, "content", "Replacement.cs"), "public static class Game { public static int Value => 3; }");
        InstalledTools.AssertNoProblems(Compile().Diagnostics);
        File.WriteAllText(Path.Combine(root, "content", "Replacement.cs"), "this is not valid C#");
        Assert.Equal("failed", Compile().Modules.Single(m => m.Name == "game").Result);
        File.WriteAllText(Path.Combine(root, "content", "Replacement.cs"), "public static class Game { public static int Value => 4; }");
        InstalledTools.AssertNoProblems(Compile().Diagnostics);
    }

    [Fact]
    public void Active_projects_keep_more_than_eight_module_compilations_and_drop_inactive_outputs()
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        var outputs = Enumerable.Range(0, 12).Select(i => $"project/module-{i}.dll").ToArray();
        cache.Retain(outputs);
        var first = cache.Generate(outputs[0], "config", compiler, false, [("Code.cs", "class Code0 { }")], []);
        for (var i = 0; i < 12; i++)
            cache.Generate(outputs[i], "config", compiler, false, [("Code.cs", $"class Code{i} {{ }}")], []);
        var reused = cache.Generate(outputs[0], "config", compiler, false, [("Code.cs", "class Code0 { }")], []);
        Assert.Same(first.Trees[0], reused.Trees[0]);
        cache.Retain([outputs[1]]);
        var evicted = cache.Generate(outputs[0], "config", compiler, false, [("Code.cs", "class Code0 { }")], []);
        Assert.NotSame(reused.Trees[0], evicted.Trees[0]);
        var result = cache.Generate(outputs[0], "config", compiler, false, [("Code.cs", "class Updated { }")], []);
        Assert.NotNull(result.Final.GetTypeByMetadataName("Updated"));
        Assert.Null(result.Final.GetTypeByMetadataName("Code0"));
        Assert.Single(result.Driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources));
    }

    [Fact]
    public void Source_membership_changes_emit_the_same_bytes_as_a_fresh_build()
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        cache.Generate("game.dll", "config", compiler, false, [("B.cs", "class B { }")], []);
        (string Path, string Text)[] sources = [("A.cs", "class A { }"), ("B.cs", "class B { }")];
        var reused = cache.Generate("game.dll", "config", compiler, false, sources, []);
        var fresh = new ProjectCompilations().Generate("game.dll", "config", compiler, false, sources, []);
        var actual = ModuleCompiler.Emit(reused.Final).Image;
        var expected = ModuleCompiler.Emit(fresh.Final).Image;
        Assert.NotNull(actual);
        Assert.NotNull(expected);
        Assert.Equal(expected.Pe, actual.Pe);
        Assert.Equal(expected.Pdb, actual.Pdb);
    }

    [Fact]
    public void Incremental_large_file_body_and_schema_edits_match_fresh_generator_and_image_outputs()
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        var source = "//" + new string('x', 2500) + "\n" + """
            /// <summary>State used by the generated query.</summary>
            [Component] public partial struct Counter {
                /// <summary>Latest result.</summary>
                public int Value;
            }
            public static partial class UpdateCounter {
                [Query] static void Run(ref Counter counter) { counter.Value = 1; }
            }
            """;
        foreach (var text in new[] { source, source.Replace("= 1;", "= 2;", StringComparison.Ordinal),
            source.Replace("public int Value;", "public int Value;\n/// <summary>Added state.</summary>\npublic int Extra;", StringComparison.Ordinal) })
        {
            var actual = cache.Generate("game.dll", "config", compiler, true, [("Code.cs", text)], []);
            var fresh = new ProjectCompilations().Generate("game.dll", "config", compiler, true, [("Code.cs", text)], []);
            Assert.Equal(fresh.Driver.GetRunResult().Results.SelectMany(result => result.GeneratedSources).Select(result => result.SourceText.ToString()),
                actual.Driver.GetRunResult().Results.SelectMany(result => result.GeneratedSources).Select(result => result.SourceText.ToString()));
            var image = ModuleCompiler.Emit(actual.Final);
            var expected = ModuleCompiler.Emit(fresh.Final);
            Assert.Empty(image.Errors);
            Assert.Empty(expected.Errors);
            Assert.NotNull(image.Image);
            Assert.NotNull(expected.Image);
            Assert.Equal(expected.Image.Pe, image.Image.Pe);
            Assert.Equal(expected.Image.Pdb, image.Image.Pdb);
        }
    }
}

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
    public void Unchanged_sources_keep_their_syntax_trees_and_configuration_changes_invalidate_them()
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
        Assert.NotSame(changed.Trees[1], configured.Trees[1]);
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
    public void Module_cache_eviction_preserves_current_source_and_generator_output()
    {
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        for (var i = 0; i < 12; i++)
            cache.Generate($"project-{i}/game.dll", "config", compiler, false, [("Code.cs", $"class Code{i} {{ }}")], []);
        var result = cache.Generate("project-0/game.dll", "config", compiler, false, [("Code.cs", "class Updated { }")], []);
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
}

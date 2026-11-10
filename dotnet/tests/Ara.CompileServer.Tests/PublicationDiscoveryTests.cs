using Ara.Hosting;
using System.Text.Json.Nodes;

namespace Ara.CompileServer.Tests;

public sealed class PublicationDiscoveryTests
{
    [Fact]
    public void Public_inventory_and_excluded_editor_publications_stay_hydrated_while_selected_caches_are_validated()
    {
        var root = InstalledTools.Scratch("publication-discovery");
        var content = Path.Combine(root, "content");
        Directory.CreateDirectory(Path.Combine(content, "editor"));
        var source = Path.Combine(content, "Game.cs");
        File.WriteAllText(source, "public static class Rules { public static int Value => 1; }");
        File.WriteAllText(Path.Combine(content, "editor", "Tool.cs"), "public static class Tool { public static int Value => 7; }");
        var initial = ProjectCompiler.Compile(new(root, "content", "editor"));
        InstalledTools.AssertNoProblems(initial.Diagnostics);
        Assert.Equal(2, initial.Modules.Length);
        foreach (var module in initial.Modules)
        {
            // Test fixture for outputs normally published by Rust's canonical writer.
            var schema = Path.Combine(root, module.SchemaPath);
            File.WriteAllText(schema, module.Schema!.ToJsonString());
            File.WriteAllText(Path.Combine(root, module.StatePath, "published.sha256"),
                $"{module.Fingerprint}\n{PublishedSnapshot.Hash(File.ReadAllBytes(schema))}\n{PublishedSnapshot.Hash(File.ReadAllBytes(Path.Combine(root, module.AssemblyPath)))}\n");
        }
        var published = ProjectInventory.Scan(new(root, "content"));
        Assert.All(published.Modules, module => Assert.Equal("published", module.Result));
        (string Path, string Hash)[] Outputs() => Directory.GetFiles(Path.Combine(root, ".ara"), "*", SearchOption.AllDirectories)
            .Order(StringComparer.Ordinal).Select(path => (path, PublishedSnapshot.Hash(File.ReadAllBytes(path)))).ToArray();
        var beforeValidation = Outputs();
        var validation = ProjectCompiler.Compile(new(root, "content", "editor"), writeOutputs: false);
        InstalledTools.AssertNoProblems(validation.Diagnostics);
        Assert.All(validation.Modules, module => Assert.Equal("cached", module.Result));
        Assert.Equal(beforeValidation, Outputs());
        var excluded = ProjectInventory.Scan(new(root, "content"), publicationFilter: module => module.Profile == "editor");
        Assert.Equal("unbuilt", excluded.Modules.Single(module => module.Name == "game").Result);
        Assert.Equal("published", excluded.Modules.Single(module => module.Name == "game.editor").Result);
        var gameplay = ProjectCompiler.Compile(new(root, "content"));
        InstalledTools.AssertNoProblems(gameplay.Diagnostics);
        Assert.Equal("cached", gameplay.Modules.Single(module => module.Name == "game").Result);
        var editor = gameplay.Modules.Single(module => module.Name == "game.editor");
        Assert.Equal("published", editor.Result);
        Assert.True(JsonNode.DeepEquals(published.Modules.Single(module => module.Name == "game.editor").Schema, editor.Schema));
        var assembly = Path.Combine(root, gameplay.Modules.Single(module => module.Name == "game").AssemblyPath);
        var expectedPe = File.ReadAllBytes(assembly);
        File.WriteAllBytes(assembly, [1, 2, 3]);
        var repaired = ProjectCompiler.Compile(new(root, "content"));
        InstalledTools.AssertNoProblems(repaired.Diagnostics);
        Assert.Equal("compiled", repaired.Modules.Single(module => module.Name == "game").Result);
        Assert.Equal(expectedPe, File.ReadAllBytes(assembly));
        File.WriteAllText(source, "public static class Rules { public static int Value => Missing; }");
        var rejected = ProjectCompiler.Compile(new(root, "content"));
        var failed = rejected.Modules.Single(module => module.Name == "game");
        Assert.Equal("failed", failed.Result);
        Assert.Null(failed.Schema);
        Assert.Null(failed.Fingerprint);
        Assert.Contains(rejected.Diagnostics, diagnostic => (string?)diagnostic["code"] == "ARA0900");
        Assert.Equal(expectedPe, File.ReadAllBytes(assembly));
        // An excluded publication still gets its normal hash validation.
        File.WriteAllBytes(Path.Combine(root, editor.AssemblyPath), [1, 2, 3]);
        var invalidEditor = ProjectCompiler.Compile(new(root, "content")).Modules.Single(module => module.Name == "game.editor");
        Assert.Equal("unbuilt", invalidEditor.Result);
        Assert.Null(invalidEditor.Schema);
    }
}

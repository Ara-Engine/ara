namespace Ara.CompileServer.Tests;

public sealed class ProjectInventoryTests
{
    [Fact]
    public async Task Packages_inside_authored_content_are_sources_but_project_packages_are_not()
    {
        var root = InstalledTools.Scratch("authored-packages");
        Directory.CreateDirectory(Path.Combine(root, "content", "packages", "nested"));
        Directory.CreateDirectory(Path.Combine(root, "packages", "plugin"));
        File.WriteAllText(Path.Combine(root, "content", "packages", "nested", "Utility.cs"), "public static class Utility { public static int Value => 7; }");
        File.WriteAllText(Path.Combine(root, "packages", "plugin", "Wrong.cs"), "#error project plugin packages are not authored source");
        using var result = await InstalledTools.Run("--compile-project", new { project=root, authoringRoot="content", domain="gameplay" });
        Assert.Empty(result.RootElement.GetProperty("diagnostics").EnumerateArray());
        var source = Assert.Single(result.RootElement.GetProperty("sources").EnumerateArray());
        Assert.Equal("packages/nested/Utility.cs", source.GetProperty("path").GetString());
        Assert.Equal("compiled", result.RootElement.GetProperty("modules")[0].GetProperty("result").GetString());
    }

    [Fact]
    public async Task Helper_only_modules_have_loadable_registration_and_explicit_inputs()
    {
        var root = InstalledTools.Scratch("helper-module");
        Directory.CreateDirectory(Path.Combine(root, "content"));
        File.WriteAllText(Path.Combine(root, "content", "Helper.cs"), "public static class Helper { public static int Value => 7; }");
        using var result = await InstalledTools.Run("--compile-project", new { project = root, authoringRoot = "content", domain = "gameplay" });
        Assert.Empty(result.RootElement.GetProperty("diagnostics").EnumerateArray());
        var state = Path.Combine(root, ".ara", "scripts", "game");
        Assert.Contains("[assembly: global::Ara.Hosting.AraModule(", File.ReadAllText(Path.Combine(state, "generated", "AraModule.g.cs")));
    }

    [Theory]
    [InlineData("{\"name\":\"a\",\"references\":[\"missing\"]}", "{\"name\":\"b\"}", "ARA0924")]
    [InlineData("{\"name\":\"a\",\"references\":[\"b\"]}", "{\"name\":\"b\",\"references\":[\"a\"]}", "ARA0923")]
    public async Task Invalid_reference_graphs_return_coded_errors(string a, string b, string code)
    {
        var root = InstalledTools.Scratch("invalid-graph");
        Directory.CreateDirectory(Path.Combine(root, "content", "a"));
        Directory.CreateDirectory(Path.Combine(root, "content", "b"));
        File.WriteAllText(Path.Combine(root, "content", "a", "a.module.json"), a);
        File.WriteAllText(Path.Combine(root, "content", "b", "b.module.json"), b);
        using var result = await InstalledTools.Run("--compile-project", new { project = root, authoringRoot = "content", domain = "gameplay" });
        Assert.Contains(result.RootElement.GetProperty("diagnostics").EnumerateArray(), d => d.GetProperty("code").GetString() == code);
        Assert.False(Directory.Exists(Path.Combine(root, ".ara")));
    }

    [Fact]
    public async Task Crossing_editor_boundary_changes_domain_and_gameplay_cannot_reference_it()
    {
        var root = InstalledTools.Scratch("editor-boundary");
        Directory.CreateDirectory(Path.Combine(root, "authored", "deep", "editor"));
        var source = Path.Combine(root, "authored", "Tool.cs");
        File.WriteAllText(source, "class Tool { }");
        using (var before = await InstalledTools.Run("--inventory", new { project = root, authoringRoot = "authored" }))
            Assert.Equal("game", before.RootElement.GetProperty("sources")[0].GetProperty("module").GetString());
        File.Move(source, Path.Combine(root, "authored", "deep", "editor", "Tool.cs"));
        using (var after = await InstalledTools.Run("--inventory", new { project = root, authoringRoot = "authored" }))
            Assert.Equal("game.editor", after.RootElement.GetProperty("sources")[0].GetProperty("module").GetString());
        File.WriteAllText(Path.Combine(root, "authored", "game.module.json"), "{\"name\":\"game\",\"references\":[\"game.editor\"]}");
        using var invalid = await InstalledTools.Run("--compile-project", new { project = root, authoringRoot = "authored", domain = "gameplay" });
        Assert.Contains(invalid.RootElement.GetProperty("diagnostics").EnumerateArray(), d => d.GetProperty("severity").GetString() == "error");
    }

    [Fact]
    public async Task Fresh_project_compiles_implicit_game_and_returns_derived_schemas_without_Play()
    {
        var root = InstalledTools.Scratch("schema-publication");
        Directory.CreateDirectory(Path.Combine(root, "content", "deep", "editor"));
        File.WriteAllText(Path.Combine(root, "ara.project.json"), "{\"layoutVersion\":4}");
        File.WriteAllText(Path.Combine(root, "content", "Armour.cs"), "using Ara; /// <summary>Armour in the ECS.</summary>\n[Component] public partial struct Armour { /// <summary>Remaining protection.</summary>\n public float Points; }");
        File.WriteAllText(Path.Combine(root, "content", "deep", "editor", "MustNotShip.cs"), "this editor-only source is deliberately not valid C#");
        using var result = await InstalledTools.Run("--compile-project", new { project = root, authoringRoot = "content", domain = "gameplay" });
        Assert.Empty(result.RootElement.GetProperty("diagnostics").EnumerateArray());
        var module = result.RootElement.GetProperty("modules").EnumerateArray().Single(m => m.GetProperty("name").GetString() == "game");
        Assert.Equal(".ara/scripts/game/game.types.json", module.GetProperty("schemaPath").GetString());
        Assert.Equal("compiled", module.GetProperty("result").GetString());
        Assert.Equal("component", module.GetProperty("schema").GetProperty("types").GetProperty("Armour").GetProperty("kind").GetString());
        Assert.True(File.Exists(Path.Combine(root, module.GetProperty("assemblyPath").GetString()!)));
        Assert.Empty(Directory.GetFiles(Path.Combine(root, "content"), "*.csproj", SearchOption.AllDirectories));
        Assert.Empty(Directory.GetDirectories(Path.Combine(root, "content"), "obj", SearchOption.AllDirectories));
    }

    [Fact]
    public async Task Default_named_nested_and_editor_sources_have_one_deterministic_owner()
    {
        var root = InstalledTools.Scratch("source-inventory");
        void Write(string path, string text)
        {
            var file = Path.Combine(root, path);
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, text);
        }
        Write("ara.project.json", "{\"layoutVersion\":4}");
        Write("content/Main.cs", "class MainGame { }");
        Write("content/core/core.module.json", "{\"name\":\"core\"}");
        Write("content/core/Core.cs", "public class Core { }");
        Write("content/combat/combat.module.json", "{\"name\":\"combat\",\"references\":[\"CORE\"]}");
        Write("content/combat/Shot.cs", "class Shot : Core { }");
        Write("content/combat/editor/Tools.cs", "class Tools { }");
        Write("content/combat/parts/editor/Deep.cs", "class Deep { }");
        Write("content/editor/Global.cs", "class GlobalTools { }");
        Write("content/nested/ara.project.json", "{\"layoutVersion\":4}");
        Write("content/nested/Wrong.cs", "this is not C#");
        Write("packages/Wrong.cs", "this is not C#");
        Write("settings/Wrong.cs", "this is not C#");
        using var result = await InstalledTools.Run("--inventory", new { project = root, authoringRoot = "content" });
        Assert.Empty(result.RootElement.GetProperty("diagnostics").EnumerateArray());
        var sources = result.RootElement.GetProperty("sources").EnumerateArray().ToArray();
        Assert.Equal(6, sources.Length);
        Assert.Equal(6, sources.Select(s => s.GetProperty("storagePath").GetString()).Distinct().Count());
        Assert.All(sources, s => Assert.Equal("content/" + s.GetProperty("path").GetString(), s.GetProperty("storagePath").GetString()));
        Assert.Equal("combat.editor", sources.Single(s => s.GetProperty("path").GetString() == "combat/parts/editor/Deep.cs").GetProperty("module").GetString());
        var modules = result.RootElement.GetProperty("modules").EnumerateArray().ToArray();
        Assert.Equal(new[] { "combat", "combat.editor", "core", "game", "game.editor" }, modules.Select(m => m.GetProperty("name").GetString()).ToArray());
        Assert.Equal("core", modules.Single(m => m.GetProperty("name").GetString() == "combat").GetProperty("references")[0].GetString());
        Assert.All(modules, m => Assert.StartsWith(".ara/scripts/", m.GetProperty("statePath").GetString()));
        Assert.Empty(Directory.GetFiles(Path.Combine(root, "content"), "*.csproj", SearchOption.AllDirectories));
    }
}

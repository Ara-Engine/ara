namespace Ara.CompileServer.Tests;

/// <summary>Folder nesting never changes which module owns a C# file.</summary>
public sealed class SourceOwnershipTests
{
    [Fact]
    public void Compile_session_uses_project_inventory_and_requests_restart_when_a_file_crosses_editor_boundary()
    {
        var root = InstalledTools.Scratch("compile-membership");
        var authored = Path.Combine(root, "game");
        var state = Path.Combine(root, ".ara", "scripts", "game");
        Directory.CreateDirectory(authored);
        Directory.CreateDirectory(state);
        var source = Path.Combine(authored, "Helper.cs");
        File.WriteAllText(source, "public class Helper { }");
        File.WriteAllText(Path.Combine(state, "module.inputs.json"), System.Text.Json.JsonSerializer.Serialize(new { project = root, authoringRoot = "game", name = "game" }));
        var session = new ModuleSession(0, "game", state, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", null);
        Assert.Equal([source], session.SourceFiles());
        Assert.Equal(authored, session.WatchDirectory);
        var capture = new Capture();
        using var coordinator = new Coordinator(capture, new AnalyzerSet());
        coordinator.Hello(false, "");
        coordinator.Watch(0, "game", state, false, [typeof(Behaviour).Assembly.Location], [], "gameplay", watch: false);
        Assert.Equal(Ara.Hosting.AbiConst.ReloadInitial, capture.Next().Reload().Single().Kind);
        Directory.CreateDirectory(Path.Combine(authored, "deep", "editor"));
        var destination = Path.Combine(authored, "deep", "editor", "Helper.cs");
        File.Move(source, destination);
        Assert.Empty(session.SourceFiles());
        coordinator.Change(0, [source, destination], Ara.Hosting.AbiConst.OriginHuman, System.Diagnostics.Stopwatch.GetTimestamp());
        Assert.Equal(Ara.Hosting.MessageKind.Restart, capture.Next().Kind);
    }

    [Fact]
    public void Nested_module_sources_belong_only_to_the_nested_module()
    {
        var root = Path.Combine(Path.GetTempPath(), "ara-source-ownership-" + Guid.NewGuid().ToString("N"));
        try
        {
            Directory.CreateDirectory(Path.Combine(root, "Gameplay", "Nested"));
            File.WriteAllText(Path.Combine(root, "Root.cs"), "class Root {}");
            Directory.CreateDirectory(Path.Combine(root, ".ara", "cache"));
            File.WriteAllText(Path.Combine(root, ".ara", "cache", "Generated.cs"), "#error cache output is not module source");
            Directory.CreateDirectory(Path.Combine(root, "artifacts"));
            File.WriteAllText(Path.Combine(root, "artifacts", "Generated.cs"), "#error build output is not module source");
            File.WriteAllText(Path.Combine(root, "Gameplay", "Child.cs"), "class Child {}");
            File.WriteAllText(Path.Combine(root, "Gameplay", "ara.module.toml"), "[module]\nname=\"Child\"\n");
            File.WriteAllText(Path.Combine(root, "Gameplay", "Nested", "Other.cs"), "class Other {}");
            var session = new ModuleSession(0, "Root", root, true, [], [], "gameplay", null);
            Assert.Equal([Path.Combine(root, "Root.cs")], session.SourceFiles());
            Assert.True(session.IsIgnored(Path.Combine(root, "Gameplay", "Nested", "Other.cs")));
        }
        finally { Directory.Delete(root, recursive: true); }
    }
}

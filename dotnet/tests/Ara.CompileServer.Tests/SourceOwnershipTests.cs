namespace Ara.CompileServer.Tests;

/// <summary>Folder nesting never changes which module owns a C# file.</summary>
public sealed class SourceOwnershipTests
{
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

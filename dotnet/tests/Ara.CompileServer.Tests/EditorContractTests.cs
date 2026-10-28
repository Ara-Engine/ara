namespace Ara.CompileServer.Tests;

public sealed class EditorContractTests
{
    enum Shape { Box }
    [Fact]
    public void Typed_authoring_values_match_schema_vectors_enums_and_addresses()
    {
        var commands = new Ara.Editor.AuthoringCommands();
        commands.AddComponent(new("e_test"), "Example", new {
            Position = new System.Numerics.Vector3(1, 2, 3),
            Uv = new System.Numerics.Vector2(4, 5),
            Color = new System.Numerics.Vector4(1, 0, 0, 1),
            Rotation = System.Numerics.Quaternion.Identity,
            Shape = Shape.Box,
            Target = new Ara.Editor.EntityId("e_other")
        });
        var value = commands.Calls.Single().Args.GetProperty("value");
        Assert.Equal("[1,2,3]", value.GetProperty("position").GetRawText());
        Assert.Equal("[4,5]", value.GetProperty("uv").GetRawText());
        Assert.Equal("[1,0,0,1]", value.GetProperty("color").GetRawText());
        Assert.Equal("[0,0,0,1]", value.GetProperty("rotation").GetRawText());
        Assert.Equal("box", value.GetProperty("shape").GetString());
        Assert.Equal("e_other", value.GetProperty("target").GetString());
    }
    [Fact]
    public void A_fault_discards_queued_commands_and_disables_only_that_callback_until_reload()
    {
        var root = InstalledTools.Scratch("editor-fault");
        var directory = Path.Combine(root, "game", "editor");
        Directory.CreateDirectory(directory);
        var source = Path.Combine(directory, "Tools.cs");
        const string text = """
            using Ara.Editor;
            public sealed class Tools : IEditorExtension {
                public void Register(EditorRegistry registry) {
                    registry.RegisterCommand("tools.bad", "Fault", context => {
                        context.Commands.Create("discarded");
                        throw new System.InvalidOperationException("callback failed");
                    });
                    registry.RegisterCommand("tools.good", "Create", context => context.Commands.Create("kept"));
                }
            }
            """;
        File.WriteAllText(source, text);
        using var session = new EditorRunner.Session();
        Assert.True(session.Run(new(root, "game")).Accepted);
        var failed = session.Run(new(root, "game", "invoke", "tools.bad"));
        Assert.Equal("ARA0912", failed.Diagnostics[0]["code"]?.ToString());
        Assert.Empty(failed.Authoring);
        Assert.False(failed.Commands.Single(c => c.Id == "tools.bad").Enabled);
        Assert.Contains("callback failed", failed.Commands.Single(c => c.Id == "tools.bad").DisabledReason);
        Assert.True(session.Run(new(root, "game", "invoke", "tools.good")).Accepted);
        Assert.False(session.Run(new(root, "game")).Commands.Single(c => c.Id == "tools.bad").Enabled);
        File.WriteAllText(source, text.Replace("discarded", "changed", StringComparison.Ordinal));
        Assert.True(session.Run(new(root, "game")).Commands.Single(c => c.Id == "tools.bad").Enabled);
    }

    [Fact]
    public void A_failed_editor_reload_retains_loaded_callbacks_and_a_new_generation_rejects_stale_invocations()
    {
        var root = InstalledTools.Scratch("editor-reload");
        var directory = Path.Combine(root, "game", "editor");
        Directory.CreateDirectory(directory);
        var source = Path.Combine(directory, "Tools.cs");
        const string text = """
            using Ara.Editor;
            public sealed class Tools : IEditorExtension {
                public void Register(EditorRegistry registry) {
                    registry.RegisterCommand("tools.create", "Create marker", context => context.Commands.Create("first"));
                    registry.AddMenu("Tools/Create marker", "tools.create");
                }
            }
            """;
        File.WriteAllText(source, text);
        using var session = new EditorRunner.Session();
        var catalog = session.Run(new(root, "game"));
        Assert.True(catalog.Accepted);
        Assert.Empty(catalog.Diagnostics);
        File.WriteAllText(source, "#error invalid editor candidate");
        var rejected = session.Run(new(root, "game"));
        Assert.False(rejected.Accepted);
        Assert.NotEmpty(rejected.Diagnostics);
        Assert.Equal(catalog.Generation, rejected.Generation);
        var lastGood = session.Run(new(root, "game", "invoke", "tools.create", Generation: catalog.Generation));
        Assert.Empty(lastGood.Diagnostics);
        Assert.Equal("first", lastGood.Authoring[0].Args.GetProperty("name").GetString());
        File.WriteAllText(source, text.Replace("first", "second", StringComparison.Ordinal));
        var replaced = session.Run(new(root, "game"));
        Assert.True(replaced.Accepted);
        Assert.NotEqual(catalog.Generation, replaced.Generation);
        Assert.Equal("ARA1501", session.Run(new(root, "game", "invoke", "tools.create", Generation: catalog.Generation)).Diagnostics[0]["code"]?.ToString());
        Assert.Equal("second", session.Run(new(root, "game", "invoke", "tools.create", Generation: replaced.Generation)).Authoring[0].Args.GetProperty("name").GetString());
    }
    [Fact]
    public async Task Editor_extension_registers_one_catalog_and_returns_typed_authoring_commands_without_writing_sources()
    {
        var root = InstalledTools.Scratch("editor-extension");
        var directory = Path.Combine(root, "game", "tools", "editor");
        Directory.CreateDirectory(directory);
        var source = Path.Combine(directory, "Tools.cs");
        const string text = """
            using Ara.Editor;
            public sealed class Tools : IEditorExtension
            {
                public void Register(EditorRegistry registry)
                {
                    registry.RegisterCommand("tools.mark", "Mark selection", context => {
                        var child = context.Scene.Find("Root/Child")!;
                        if (context.Selection.Entities.Single() != child.Id || context.Scene.Parent(child.Id)?.Name != "Root"
                            || context.Scene.Children(context.Scene.Find("Root")!.Id).Count != 1
                            || context.Scene.Siblings(child.Id).Count != 0) throw new System.InvalidOperationException("snapshot queries");
                        context.Commands.SetProperty(child.Id, "Transform.position", new System.Numerics.Vector3(1, 2, 3));
                        context.Commands.Reparent(child.Id, null);
                        var marker = context.Commands.Create("Marker", child.Id);
                        context.Selection.Set([marker]);
                    }, "Mark the current selection through authoring ops.");
                    registry.AddMenu("Tools/Mark selection", "tools.mark");
                }
            }
            """;
        File.WriteAllText(source, text);
        var scene = new { entities = new[] {
            new { id="e_root", name="Root", parent=(string?)null, order="a0", components=new {} },
            new { id="e_child", name="Child", parent=(string?)"e_root", order="a0", components=new {} },
        }};
        using var catalog = await InstalledTools.Run("--editor", new { project=root, authoringRoot="game", operation="catalog", scene, selection=new[]{"e_child"}, reserved=Array.Empty<string>() });
        Assert.Empty(catalog.RootElement.GetProperty("diagnostics").EnumerateArray());
        Assert.Equal("tools.mark", catalog.RootElement.GetProperty("commands")[0].GetProperty("id").GetString());
        Assert.Equal("Tools/Mark selection", catalog.RootElement.GetProperty("menus")[0].GetProperty("path").GetString());
        using var invoked = await InstalledTools.Run("--editor", new { project=root, authoringRoot="game", operation="invoke", command="tools.mark", scene, selection=new[]{"e_child"}, reserved=Array.Empty<string>() });
        Assert.Empty(invoked.RootElement.GetProperty("diagnostics").EnumerateArray());
        Assert.Equal(new[] { "property.set", "entity.reparent", "entity.create" }, invoked.RootElement.GetProperty("authoring").EnumerateArray().Select(c => c.GetProperty("cmd").GetString()).ToArray());
        Assert.StartsWith("$", invoked.RootElement.GetProperty("selection")[0].GetString());
        Assert.Equal(text, File.ReadAllText(source));
        Assert.Empty(Directory.GetFiles(Path.Combine(root, "game"), "*.csproj", SearchOption.AllDirectories));
    }
}

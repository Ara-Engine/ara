using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class OwnershipInventoryTests
{
    static void Publish(string root, ProjectModule module)
    {
        var schema = Path.Combine(root, module.SchemaPath);
        File.WriteAllText(schema, module.Schema!.ToJsonString());
        static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));
        File.WriteAllText(Path.Combine(root, module.StatePath, "published.sha256"),
            $"{module.Fingerprint}\n{Hash(File.ReadAllBytes(schema))}\n{Hash(File.ReadAllBytes(Path.Combine(root, module.AssemblyPath)))}\n");
    }

    static void SameDiscovery(ProjectInventoryResult expected, ProjectInventoryResult actual)
    {
        static string Graph(ProjectInventoryResult inventory) => JsonSerializer.Serialize(inventory.Modules
            .Select(module => module with { Schema = null, Result = "unbuilt", Fingerprint = null }));
        Assert.Equal(Graph(expected), Graph(actual));
        Assert.Equal(expected.Sources, actual.Sources);
        Assert.Equal(expected.Diagnostics.Select(row => row.ToJsonString()), actual.Diagnostics.Select(row => row.ToJsonString()));
    }

    [Fact]
    public void Ownership_scans_skip_publications_without_changing_public_inventory_or_excluded_editor_outputs()
    {
        var root = InstalledTools.Scratch("ownership-inventory");
        var content = Path.Combine(root, "content");
        Directory.CreateDirectory(Path.Combine(content, "editor"));
        File.WriteAllText(Path.Combine(content, "Counter.cs"), """
            using Ara;
            /// <summary>State used to verify actual schema publication.</summary>
            [Component] public partial struct OwnershipCounter
            {
                /// <summary>Current count.</summary>
                public int Value;
            }
            """);
        File.WriteAllText(Path.Combine(content, "editor", "Tool.cs"), "public static class OwnershipTool { public static int Value() => 7; }");
        var compiled = ProjectCompiler.Compile(new(root, "content", "editor"));
        InstalledTools.AssertNoProblems(compiled.Diagnostics);
        Assert.Equal(2, compiled.Modules.Length);
        foreach (var module in compiled.Modules)
        {
            Assert.Equal("compiled", module.Result);
            Publish(root, module);
        }

        var request = new InventoryRequest(root, "content");
        var inputs = new ModuleInputs(root, "content", "game");
        var published = ProjectInventory.Scan(request);
        var ownership = inputs.Scan();
        SameDiscovery(published, ownership);
        Assert.All(ownership.Modules, module =>
        {
            Assert.Null(module.Schema);
            Assert.Null(module.Fingerprint);
            Assert.Equal("unbuilt", module.Result);
        });
        foreach (var module in published.Modules)
        {
            var emitted = compiled.Modules.Single(candidate => candidate.Name == module.Name);
            Assert.Equal("published", module.Result);
            Assert.Equal(emitted.Fingerprint, module.Fingerprint);
            Assert.True(JsonNode.DeepEquals(emitted.Schema, module.Schema));
        }
        var publishedGame = published.Modules.Single(module => module.Profile == "gameplay");
        Assert.NotNull(publishedGame.Schema!["types"]!["OwnershipCounter"]);

        // Gameplay compilation must still hydrate the publication it excludes.
        var gameplay = ProjectCompiler.Compile(new(root, "content", "gameplay"));
        InstalledTools.AssertNoProblems(gameplay.Diagnostics);
        var editor = gameplay.Modules.Single(module => module.Profile == "editor");
        var publishedEditor = published.Modules.Single(module => module.Profile == "editor");
        Assert.Equal("published", editor.Result);
        Assert.Equal(publishedEditor.Fingerprint, editor.Fingerprint);
        Assert.True(JsonNode.DeepEquals(publishedEditor.Schema, editor.Schema));

        File.WriteAllText(Path.Combine(root, publishedGame.SchemaPath), "{corrupt schema");
        File.WriteAllBytes(Path.Combine(root, publishedGame.AssemblyPath), [1, 2, 3]);
        SameDiscovery(ownership, inputs.Scan());
        Assert.Equal("unbuilt", ProjectInventory.Scan(request).Modules.Single(module => module.Name == "game").Result);

        // Real ownership diagnostics also agree; neither mode consults derived
        // publication data to decide the graph or the owner of a source file.
        var broken = Path.Combine(content, "broken");
        Directory.CreateDirectory(broken);
        File.WriteAllText(Path.Combine(broken, "broken.module.json"), """{"name":"broken","references":["missing"]}""");
        var fullErrors = ProjectInventory.Scan(request);
        var ownershipErrors = inputs.Scan();
        Assert.Contains(fullErrors.Diagnostics, row => (string?)row["code"] == AraCodes.UnknownModule);
        SameDiscovery(fullErrors, ownershipErrors);
    }
}

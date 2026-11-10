using System.Text.Json;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class PublishedSnapshotTests
{
    const string ComponentOnly = "/// <summary>Only fields, no sequence points.</summary>\npublic struct Data { public int Value; }";

    static (string Root, string Assembly, string Source) Publication()
    {
        var root = InstalledTools.Scratch("published-identity");
        var source = Path.Combine(root, "Data.cs");
        File.WriteAllText(source, ComponentOnly);
        var compiler = new ModuleCompiler("Core", []);
        var (image, errors) = ModuleCompiler.Emit(compiler.Create([ModuleCompiler.Parse(source, ComponentOnly)], false, []));
        Assert.Empty(errors);
        Assert.NotNull(image);
        var assembly = Path.Combine(root, "Core.dll");
        File.WriteAllBytes(assembly, image.Pe);
        File.WriteAllBytes(Path.ChangeExtension(assembly, ".pdb"), image.Pdb);
        File.WriteAllText(Path.Combine(root, "input.sha256"), "fingerprint\n");
        File.WriteAllText(Path.Combine(root, "published.sha256"), $"fingerprint\nschema-hash\n{PublishedSnapshot.Hash(image.Pe)}\n");
        File.WriteAllText(Path.Combine(root, "module.inputs.json"), JsonSerializer.Serialize(new {
            project = root, name = "Core", sources = new[] { new { storagePath = "Data.cs" } },
        }));
        File.WriteAllBytes(Path.Combine(root, PublishedSnapshot.ManifestFile), PublishedSnapshot.ManifestBytes(
            "Core", false, "gameplay", [], "fingerprint", image.Pe, image.Pdb, [(source, ComponentOnly)], graph: "[]"));
        return (root, assembly, source);
    }

    [Theory]
    [InlineData("missing")]
    [InlineData("corrupt")]
    [InlineData("source")]
    [InlineData("pe")]
    [InlineData("pdb")]
    [InlineData("stamp")]
    public void Invalid_publication_identity_requires_normal_initialization(string mutation)
    {
        var (root, assembly, source) = Publication();
        Assert.NotNull(PublishedSnapshot.TryCapture(root, assembly, "Core", false, [], "gameplay"));
        switch (mutation)
        {
            case "missing": File.Delete(Path.Combine(root, PublishedSnapshot.ManifestFile)); break;
            case "corrupt": File.WriteAllText(Path.Combine(root, PublishedSnapshot.ManifestFile), "{broken"); break;
            case "source": File.WriteAllText(source, ComponentOnly.Replace("int Value", "long Value", StringComparison.Ordinal)); break;
            case "pe": File.WriteAllBytes(assembly, [1, 2, 3]); break;
            case "pdb": File.WriteAllBytes(Path.ChangeExtension(assembly, ".pdb"), [1, 2, 3]); break;
            case "stamp": File.WriteAllText(Path.Combine(root, "published.sha256"), "other\nschema-hash\nwrong\n"); break;
        }
        Assert.Null(PublishedSnapshot.TryCapture(root, assembly, "Core", false, [], "gameplay"));
    }

    [Fact]
    public void Captured_image_and_component_only_text_survive_later_path_changes_and_wire_roundtrip()
    {
        var (root, assembly, source) = Publication();
        var captured = PublishedSnapshot.TryCapture(root, assembly, "Core", false, [], "gameplay");
        Assert.NotNull(captured);
        File.WriteAllBytes(assembly, [1, 2, 3]);
        File.WriteAllText(source, "invalid later source");
        var writer = new Protocol.Writer();
        captured.Write(writer);
        var reader = new Protocol.Reader(writer.ToArray());
        var received = PublishedSnapshot.Read(reader);
        Assert.Equal(0, reader.Remaining);
        Assert.True(received.Valid("Core", false, 0));
        Assert.Equal(captured.Pe, received.Pe);
        Assert.Equal(captured.Pdb, received.Pdb);
        Assert.Equal(ComponentOnly, Assert.Single(received.Sources).Text);
        Assert.Equal("[]", received.Graph);
    }

    [Theory]
    [InlineData("missing")]
    [InlineData("corrupt")]
    [InlineData("foreign")]
    [InlineData("body")]
    public void Cached_publication_recreates_snapshot_but_reemits_invalid_symbols_and_validation_never_persists_it(string symbolMutation)
    {
        var root = InstalledTools.Scratch("published-cache-snapshot");
        Directory.CreateDirectory(Path.Combine(root, "content"));
        File.WriteAllText(Path.Combine(root, "content", "Code.cs"), "public static class Rules { public static int Value() => 1; }");
        var validation = ProjectCompiler.Compile(new(root, "content"), writeOutputs: false);
        var validated = Assert.Single(validation.Modules);
        var manifest = Path.Combine(root, validated.StatePath, PublishedSnapshot.ManifestFile);
        Assert.Equal("compiled", validated.Result);
        Assert.False(File.Exists(manifest));
        var first = Assert.Single(ProjectCompiler.Compile(new(root, "content")).Modules);
        Assert.Equal("compiled", first.Result);
        Assert.True(File.Exists(manifest));
        var assembly = Path.Combine(root, first.AssemblyPath);
        var expectedPe = File.ReadAllBytes(assembly);
        var schema = Path.Combine(root, first.SchemaPath);
        // The Rust publisher normally writes these two private publication outputs.
        File.WriteAllText(schema, first.Schema!.ToJsonString());
        File.WriteAllText(Path.Combine(root, first.StatePath, "published.sha256"),
            $"{first.Fingerprint}\n{PublishedSnapshot.Hash(File.ReadAllBytes(schema))}\n{PublishedSnapshot.Hash(expectedPe)}\n");
        File.Delete(manifest);
        var cached = Assert.Single(ProjectCompiler.Compile(new(root, "content")).Modules);
        Assert.Equal("compiled", cached.Result); // Missing trust metadata is a disposable cache miss.
        Assert.Equal(expectedPe, File.ReadAllBytes(assembly));
        using var snapshot = JsonDocument.Parse(File.ReadAllBytes(manifest));
        Assert.Equal(PublishedSnapshot.Hash(expectedPe), snapshot.RootElement.GetProperty("PeHash").GetString());
        Assert.Equal(cached.Fingerprint, snapshot.RootElement.GetProperty("Fingerprint").GetString());
        Assert.Equal("cached", Assert.Single(ProjectCompiler.Compile(new(root, "content")).Modules).Result);
        var symbols = Path.ChangeExtension(assembly, ".pdb");
        if (symbolMutation == "missing") File.Delete(symbols);
        else if (symbolMutation == "corrupt") File.WriteAllBytes(symbols, [1, 2, 3]);
        else if (symbolMutation == "body")
        {
            var altered = File.ReadAllBytes(symbols).Append((byte)0xa5).ToArray();
            Assert.True(PublishedSnapshot.SymbolsMatch(expectedPe, altered), "header identity alone misses altered PDB bytes");
            File.WriteAllBytes(symbols, altered);
        }
        else
        {
            var foreign = new ModuleCompiler("Foreign", []);
            var (foreignImage, errors) = ModuleCompiler.Emit(foreign.Create(
                [ModuleCompiler.Parse(Path.Combine(root, "Foreign.cs"), "public class Foreign { }")], false, []));
            Assert.Empty(errors);
            Assert.NotNull(foreignImage);
            File.WriteAllBytes(symbols, foreignImage.Pdb);
        }
        var repaired = Assert.Single(ProjectCompiler.Compile(new(root, "content")).Modules);
        Assert.Equal("compiled", repaired.Result);
        Assert.True(PublishedSnapshot.SymbolsMatch(File.ReadAllBytes(assembly), File.ReadAllBytes(symbols)));
    }

    [Fact]
    public void A_different_or_nonpublished_dependency_forces_normal_initialization()
    {
        var (root, assembly, source) = Publication();
        var pe = File.ReadAllBytes(assembly);
        var pdb = File.ReadAllBytes(Path.ChangeExtension(assembly, ".pdb"));
        File.WriteAllBytes(Path.Combine(root, PublishedSnapshot.ManifestFile), PublishedSnapshot.ManifestBytes(
            "Core", false, "gameplay", ["Dependency"], "fingerprint", pe, pdb, [(source, ComponentOnly)],
            new Dictionary<string, string> { ["Dependency"] = "dependency-publication" }, "[]"));
        Assert.NotNull(PublishedSnapshot.TryCapture(root, assembly, "Core", false, ["Dependency"], "gameplay", _ => "dependency-publication"));
        Assert.Null(PublishedSnapshot.TryCapture(root, assembly, "Core", false, ["Dependency"], "gameplay", _ => "different-publication"));
        Assert.Null(PublishedSnapshot.TryCapture(root, assembly, "Core", false, ["Dependency"], "gameplay", _ => null));
    }

    [Fact]
    public void Capture_allows_the_editor_to_keep_a_write_handle_open()
    {
        var (root, assembly, source) = Publication();
        using var editor = new FileStream(source, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite | FileShare.Delete);
        Assert.NotNull(PublishedSnapshot.TryCapture(root, assembly, "Core", false, [], "gameplay"));
    }

    [Fact]
    public void Readonly_capture_keeps_image_and_known_paths_without_reading_changed_source_bodies()
    {
        var (root, assembly, source) = Publication();
        File.WriteAllText(source, "this source no longer compiles");
        Assert.Null(PublishedSnapshot.TryCapture(root, assembly, "Core", false, [], "gameplay"));
        using var editor = new FileStream(source, FileMode.Open, FileAccess.ReadWrite, FileShare.None);
        var captured = PublishedSnapshot.CaptureReadOnly(root, assembly, "Core", false, [], "gameplay", _ => null);
        Assert.False(captured.HasCommittedSources);
        Assert.Empty(captured.Sources);
        Assert.Equal([source], Assert.IsType<string[]>(captured.OwnedPaths));
        Assert.Equal("fingerprint", captured.Fingerprint);
        Assert.True(captured.Valid("Core", false, 0));
    }

    [Fact]
    public void Legacy_readonly_image_has_opaque_identity_and_no_invented_source_ownership()
    {
        var (root, assembly, source) = Publication();
        File.Delete(Path.Combine(root, PublishedSnapshot.ManifestFile));
        File.WriteAllText(source, "not a valid source snapshot");
        var captured = PublishedSnapshot.CaptureReadOnly(root, assembly, "Core", false, [], "gameplay", _ => null);
        Assert.False(captured.HasCommittedSources);
        Assert.Null(captured.OwnedPaths);
        Assert.StartsWith("opaque:", captured.Fingerprint);
        var writer = new Protocol.Writer();
        captured.Write(writer);
        var received = PublishedSnapshot.Read(new(writer.ToArray()));
        Assert.False(received.HasCommittedSources);
        Assert.Empty(received.Sources);
        Assert.Null(received.OwnedPaths);
        Assert.Equal(captured.Pe, received.Pe);
        Assert.True(received.Valid("Core", false, 0));
    }
}

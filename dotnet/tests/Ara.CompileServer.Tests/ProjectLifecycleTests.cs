using System.Text.Json.Nodes;

namespace Ara.CompileServer.Tests;

public sealed class ProjectLifecycleTests
{
    static JsonObject Completion(IEnumerable<JsonObject> rows, string module) => rows.Last(row =>
        (string?)row["data"]?["module"] == module && (string?)row["data"]?["compile"]?["phase"] == "completed");

    [Fact]
    public void Project_repairs_and_editor_catalogs_keep_the_parent_owned_context_and_canonical_profile()
    {
        var root = InstalledTools.Scratch("project-lifecycle");
        Directory.CreateDirectory(Path.Combine(root, "content", "editor"));
        var source = Path.Combine(root, "content", "Counter.cs");
        File.WriteAllText(source, "public static class Counter { public static int Value => Missing; }");
        var request = new ProjectBuildRequest(root, "content", DiagnosticContext:new("project", "parent-project", 7, 41));
        var failed = ProjectCompiler.Compile(request);
        Assert.Equal("errors", (string?)Completion(failed.Diagnostics, "game")["data"]?["compile"]?["result"]);
        var error = Assert.Single(failed.Diagnostics, row => (string?)row["severity"] == "error");
        Assert.Equal("game", (string?)error["data"]?["module"]);
        Assert.Equal(41UL, (ulong)error["data"]!["compile"]!["attempt"]!);
        File.WriteAllText(source, "public static class Counter { public static int Value => 7; }");
        var repaired = ProjectCompiler.Compile(request with { DiagnosticContext=request.DiagnosticContext! with { Attempt=42 } });
        var compile = Completion(repaired.Diagnostics, "game")["data"]!["compile"]!;
        Assert.Equal("success", (string?)compile["result"]);
        Assert.Equal("current-module-inputs", (string?)compile["validation"]);
        Assert.Equal("parent-project", (string?)compile["instance"]);
        Assert.Equal(7UL, (ulong)compile["binding"]!);
        Assert.Equal(42UL, (ulong)compile["attempt"]!);
        File.WriteAllText(Path.Combine(root, "content", "editor", "Tools.cs"), "using Ara.Editor; public sealed class Tools : IEditorExtension { public void Register(EditorRegistry registry) { Missing; } }");
        using var editor = new EditorRunner.Session();
        var editorRequest = new EditorRequest(root, "content", DiagnosticContext:new("editor", "parent-editor", 8, 51));
        var brokenCatalog = editor.Run(editorRequest);
        Assert.Equal("errors", (string?)Completion(brokenCatalog.Diagnostics, "game.editor")["data"]?["compile"]?["result"]);
        File.WriteAllText(Path.Combine(root, "content", "editor", "Tools.cs"), "using Ara.Editor; public sealed class Tools : IEditorExtension { public void Register(EditorRegistry registry) { } }");
        var catalog = editor.Run(editorRequest with { DiagnosticContext=editorRequest.DiagnosticContext! with { Attempt=52 } });
        Assert.True(catalog.Accepted);
        var catalogCompile = Completion(catalog.Diagnostics, "game.editor")["data"]!["compile"]!;
        Assert.Equal("success", (string?)catalogCompile["result"]);
        Assert.Equal("editor", (string?)catalogCompile["profile"]);
        Assert.Equal("editor", (string?)catalogCompile["producer"]);
        Assert.Equal("parent-editor", (string?)catalogCompile["instance"]);
        Assert.Equal(52UL, (ulong)catalogCompile["attempt"]!);
    }

    [Fact]
    public void Tagging_preserves_existing_payload_and_subjects()
    {
        var original = new JsonObject { ["code"]="ARA0001", ["severity"]="error", ["message"]="retained", ["data"]=new JsonObject { ["custom"]=7 }, ["subjects"]=new JsonArray("system:custom") };
        var row = new CompileAttempt("Combat", "gameplay", new("project", "parent", 1, 2)).Tag(original);
        Assert.Equal(7, (int)row["data"]!["custom"]!);
        Assert.Contains(row["subjects"]!.AsArray(), subject => (string?)subject == "system:custom");
        Assert.Equal("retained", (string?)row["message"]);
    }

    [Fact]
    public void Invalid_reference_graph_reports_owned_failure_without_claiming_source_validation()
    {
        var root = InstalledTools.Scratch("graph-lifecycle");
        Directory.CreateDirectory(Path.Combine(root, "content"));
        File.WriteAllText(Path.Combine(root, "content", "combat.module.json"), "{\"name\":\"Combat\",\"references\":[\"Missing\"]}");
        var inventory = ProjectCompiler.Compile(new(root, "content", DiagnosticContext:new("project", "parent", 1, 1)));
        var completed = Completion(inventory.Diagnostics, "Combat")["data"]!["compile"]!;
        Assert.Equal("errors", (string?)completed["result"]);
        Assert.Equal("not-evaluated", (string?)completed["validation"]);
        var error = Assert.Single(inventory.Diagnostics, d => (string?)d["severity"] == "error");
        Assert.Equal("Combat", (string?)error["data"]?["module"]);
        Assert.Equal("dependency", (string?)error["data"]?["compile"]?["channel"]);
    }
}

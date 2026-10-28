using System.Collections.Immutable;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace Ara.CompileServer;

internal sealed record ProjectBuildRequest(string Project, string AuthoringRoot, string Domain = "gameplay", CompileRequestContext? DiagnosticContext = null);

/// <summary>Compile and describe modules before scene validation; never start a gameplay world.</summary>
internal static class ProjectCompiler
{
    static readonly CompileProducer ProjectDiagnostics = new("project");
    static readonly CompileProducer EditorDiagnostics = new("editor");
    internal static ProjectInventoryResult Compile(ProjectBuildRequest request, bool writeOutputs = true)
    {
        var inventory = ProjectInventory.Scan(new(request.Project, request.AuthoringRoot));
        if (inventory.Diagnostics.Any(d => (string?)d["severity"] == "error"))
        {
            // Ownership comes from discovery, never the diagnostic's mapped path.
            // Unowned manifest/I/O failures stay conservative configuration rows.
            var graphRows = inventory.Diagnostics.Where(d => d["data"]?["module"] is null).ToList();
            foreach (var module in inventory.Modules.Where(m => request.Domain == "editor" || m.Profile == "gameplay"))
            {
                var attempt = Attempt(request, module);
                var owned = inventory.Diagnostics.Where(d => string.Equals((string?)d["data"]?["module"], module.Name, StringComparison.OrdinalIgnoreCase)).ToArray();
                graphRows.Add((attempt with { Request = attempt.Request with { Attempt = 0 } }).Marker("bound"));
                graphRows.Add(attempt.Marker("started"));
                graphRows.AddRange(attempt.Complete(owned, owned.Any(d => (string?)d["severity"] == "error") ? "errors" : "cancelled", validated:false));
            }
            return inventory with { Modules = inventory.Modules.Select(m => m with { Result = "failed", Schema = null, Fingerprint = null }).ToArray(), Diagnostics=[.. graphRows] };
        }
        var root = ProjectInventory.ProjectPath(request.Project);
        var modules = inventory.Modules.ToDictionary(m => m.Name, StringComparer.OrdinalIgnoreCase);
        var built = new Dictionary<string, (MetadataReference Reference, string Hash)>(StringComparer.OrdinalIgnoreCase);
        var results = new Dictionary<string, ProjectModule>(StringComparer.OrdinalIgnoreCase);
        var diagnostics = new List<JsonObject>();
        var analysis = new AnalyzerSet();
        var toolHash = Hash(File.ReadAllBytes(typeof(ProjectCompiler).Assembly.Location));
        var runtimeHash = Hash(File.ReadAllBytes(typeof(Behaviour).Assembly.Location));
        var generatorHash = Hash(File.ReadAllBytes(typeof(Ara.Generators.ModuleGenerator).Assembly.Location));

        bool Build(string name)
        {
            if (built.ContainsKey(name)) return true;
            if (results.ContainsKey(name)) return false;
            var module = modules[name];
            var attempt = Attempt(request, module);
            var rows = new List<JsonObject>();
            var validated = false;
            diagnostics.Add((attempt with { Request = attempt.Request with { Attempt = 0 } }).Marker("bound"));
            diagnostics.Add(attempt.Marker("started"));
            try
            {
                foreach (var dependency in module.References)
                    if (!Build(dependency))
                    {
                        rows.Add(Diagnostics.Row(AraCodes.DependencyFailed, "error", $"module `{name}` cannot compile because `{dependency}` failed", module.Directory));
                        results[name] = module with { Result = "failed", Schema = null, Fingerprint = null };
                        return false;
                    }
                var sources = module.Sources.Select(s => (Source:s, Text:File.ReadAllText(Path.Combine(root, s.StoragePath)))).ToArray();
                var fingerprint = Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new
                {
                    module.Name, module.Hot, module.Profile, toolHash, runtimeHash, generatorHash,
                    editorHash = module.Profile == "editor" ? Hash(File.ReadAllBytes(typeof(Ara.Editor.EditorRegistry).Assembly.Location)) : "",
                    sources = sources.Select(s => new { s.Source.StoragePath, hash = Hash(Encoding.UTF8.GetBytes(s.Text)) }),
                    references = module.References.Select(r => new { name = r, hash = built[r].Hash }),
                })));
                var state = Path.Combine(root, module.StatePath);
                var assembly = Path.Combine(root, module.AssemblyPath);
                var stamp = Path.Combine(state, "input.sha256");
                var schemaFile = Path.Combine(root, module.SchemaPath);
                var publicationFile = Path.Combine(state, "published.sha256");
                var publication = File.Exists(publicationFile) ? File.ReadAllLines(publicationFile) : [];
                if (File.Exists(stamp) && File.ReadAllText(stamp).Trim() == fingerprint && File.Exists(assembly) && File.Exists(schemaFile)
                    && publication.Length == 3 && publication[0] == fingerprint && publication[1] == Hash(File.ReadAllBytes(schemaFile)) && publication[2] == Hash(File.ReadAllBytes(assembly)))
                {
                    var schema = JsonNode.Parse(File.ReadAllText(schemaFile));
                    if (schema?["$schema"]?.GetValue<string>() == "ara://schema/types/1")
                    {
                        built[name] = (MetadataReference.CreateFromImage(File.ReadAllBytes(assembly)), fingerprint);
                        results[name] = module with { Schema = schema, Result = "cached", Fingerprint = fingerprint };
                        validated = true;
                        return true;
                    }
                }
                var extra = new List<string> { typeof(Behaviour).Assembly.Location };
                if (module.Profile == "editor")
                {
                    var editor = Path.Combine(AppContext.BaseDirectory, "Ara.Editor.dll");
                    if (File.Exists(editor)) extra.Add(editor);
                }
                var compiler = new ModuleCompiler(name, extra);
                var trees = sources.Select(s => ModuleCompiler.Parse(Path.Combine(root, s.Source.StoragePath), s.Text)).ToArray();
                var compilation = Microsoft.CodeAnalysis.CSharp.CSharpCompilation.Create(name,
                    [.. trees, ModuleCompiler.Parse("<ara-global-usings>.cs", ModuleCompiler.GlobalUsings)],
                    [.. compiler.References, .. module.References.Select(r => built[r].Reference)], ModuleCompiler.Options(module.Hot));
                var driver = ModuleCompiler.Driver().RunGeneratorsAndUpdateCompilation(compilation, out var final, out var generatorDiagnostics);
                var problems = generatorDiagnostics.Concat(final.GetDiagnostics()).Concat(analysis.Analyze(final, trees, module.Profile))
                    .Where(Diagnostics.Reportable).ToArray();
                validated = true;
                foreach (var diagnostic in problems) rows.Add(ToJson(diagnostic));
                if (problems.Any(d => d.Severity == DiagnosticSeverity.Error)) { results[name] = module with { Result = "failed", Schema = null, Fingerprint = null }; return false; }
                var (image, errors) = ModuleCompiler.Emit(final);
                if (image is null)
                {
                    rows.AddRange(errors.Select(ToJson));
                    results[name] = module with { Result = "failed", Schema = null, Fingerprint = null };
                    return false;
                }
                var schemaTypes = new SortedDictionary<string, JsonNode?>(StringComparer.Ordinal);
                foreach (var generated in driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources).Where(s => s.HintName == "AraModule.g.cs"))
                {
                    foreach (var invocation in generated.SyntaxTree.GetRoot().DescendantNodes().OfType<InvocationExpressionSyntax>())
                    {
                        if (invocation.Expression is not MemberAccessExpressionSyntax { Expression: IdentifierNameSyntax { Identifier.ValueText: "b" }, Name.Identifier.ValueText: "Type" }
                            || invocation.ArgumentList.Arguments is not { Count: 2 } arguments
                            || arguments[0].Expression is not LiteralExpressionSyntax type
                            || arguments[1].Expression is not LiteralExpressionSyntax json) continue;
                        var definition = JsonNode.Parse(json.Token.ValueText)!.AsObject();
                        definition["module"] = name;
                        schemaTypes[type.Token.ValueText] = definition;
                    }
                }
                var types = new JsonObject();
                foreach (var (key, value) in schemaTypes) types.Add(key, value);
                var document = new JsonObject { ["$schema"] = "ara://schema/types/1", ["types"] = types };
                if (writeOutputs)
                {
                    Directory.CreateDirectory(Path.GetDirectoryName(assembly)!);
                    AtomicWrite(assembly, image.Pe);
                    AtomicWrite(Path.ChangeExtension(assembly, ".pdb"), image.Pdb);
                    foreach (var generated in driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources))
                        AtomicWrite(Path.Combine(state, "generated", generated.HintName), Encoding.UTF8.GetBytes(generated.SourceText.ToString()));
                    AtomicWrite(stamp, Encoding.UTF8.GetBytes(fingerprint + "\n"));
                }
                // Rust publishes this JSON through the canonical writer. The compiler
                // returns it explicitly; no consumer scans private output directories.
                built[name] = (MetadataReference.CreateFromImage(image.Pe), fingerprint);
                results[name] = module with { Schema = document, Result = "compiled", Fingerprint = fingerprint };
                return true;
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException)
            {
                rows.Add(ProjectInventory.Error("ARA1003", error.Message, module.Directory));
                validated = false;
                results[name] = module with { Result = "failed", Schema = null, Fingerprint = null };
                return false;
            }
            finally
            {
                var result = rows.Any(d => (string?)d["severity"] == "error") ? "errors" : validated ? "success" : "cancelled";
                diagnostics.AddRange(attempt.Complete(rows, result, validated));
            }
        }

        foreach (var module in inventory.Modules.Where(m => request.Domain == "editor" || m.Profile == "gameplay")) Build(module.Name);
        return inventory with { Modules = inventory.Modules.Select(m => results.GetValueOrDefault(m.Name, m)).ToArray(), Diagnostics = [.. inventory.Diagnostics, .. diagnostics] };
    }

    static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));

    static CompileAttempt Attempt(ProjectBuildRequest request, ProjectModule module) => request.DiagnosticContext is { } context
        ? new(module.Name, module.Profile, context)
        : (request.Domain == "editor" ? EditorDiagnostics : ProjectDiagnostics).Next(module.Name, module.Profile);

    static JsonObject ToJson(Diagnostic diagnostic)
    {
        return Diagnostics.Row(diagnostic);
    }

    static void AtomicWrite(string path, byte[] bytes)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var temporary = path + ".tmp-" + System.Environment.ProcessId + "-" + Guid.NewGuid().ToString("N");
        try { File.WriteAllBytes(temporary, bytes); File.Move(temporary, path, overwrite: true); }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }
}

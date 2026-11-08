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
    // A frontier shares only completed dependency images; each worker owns its
    // Roslyn state and diagnostic rows. Requests on a session remain sequential.
    sealed class BuildState(ProjectModule module, CompileAttempt attempt)
    {
        internal ProjectModule Module = module;
        internal MetadataReference? Reference;
        internal readonly CompileAttempt Attempt = attempt;
        internal readonly List<JsonObject> Diagnostics = [];
    }

    internal static ProjectInventoryResult Compile(ProjectBuildRequest request, bool writeOutputs = true, ProjectCompilations? compilations = null, int maxParallelism = 2)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(maxParallelism, 1);
        compilations ??= new ProjectCompilations();
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
        compilations.Retain(inventory.Modules.Select(module => Path.Combine(root, module.AssemblyPath)));
        var orderedBuilds = inventory.Modules.Where(m => request.Domain == "editor" || m.Profile == "gameplay")
            .Select(m => new BuildState(m, Attempt(request, m))).ToArray();
        var builds = orderedBuilds.ToDictionary(b => b.Module.Name, StringComparer.OrdinalIgnoreCase);
        var toolHash = Hash(File.ReadAllBytes(typeof(ProjectCompiler).Assembly.Location));
        var runtimeHash = Hash(File.ReadAllBytes(typeof(Behaviour).Assembly.Location));
        var generatorHash = Hash(File.ReadAllBytes(typeof(Ara.Generators.ModuleGenerator).Assembly.Location));

        void Build(BuildState build)
        {
            var module = build.Module;
            var name = module.Name;
            var attempt = build.Attempt;
            var rows = new List<JsonObject>();
            var validated = false;
            build.Diagnostics.Add((attempt with { Request = attempt.Request with { Attempt = 0 } }).Marker("bound"));
            build.Diagnostics.Add(attempt.Marker("started"));
            try
            {
                foreach (var dependency in module.References)
                    if (builds[dependency].Reference is null)
                    {
                        rows.Add(Diagnostics.Row(AraCodes.DependencyFailed, "error", $"module `{name}` cannot compile because `{dependency}` failed", module.Directory));
                        build.Module = module with { Result = "failed", Schema = null, Fingerprint = null };
                        return;
                    }
                var sources = module.Sources.Select(s => (Source:s, Text:File.ReadAllText(Path.Combine(root, s.StoragePath)))).ToArray();
                var configuration = Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new
                {
                    module.Name, module.Hot, module.Profile, toolHash, runtimeHash, generatorHash,
                    editorHash = module.Profile == "editor" ? Hash(File.ReadAllBytes(typeof(Ara.Editor.EditorRegistry).Assembly.Location)) : "",
                    references = module.References.Select(r => new { name = r, hash = builds[r].Module.Fingerprint }),
                })));
                var fingerprint = Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new
                {
                    configuration,
                    sources = sources.Select(s => new { s.Source.StoragePath, hash = Hash(Encoding.UTF8.GetBytes(s.Text)) }),
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
                        build.Reference = MetadataReference.CreateFromImage(File.ReadAllBytes(assembly));
                        build.Module = module with { Schema = schema, Result = "cached", Fingerprint = fingerprint };
                        validated = true;
                        return;
                    }
                }
                var extra = new List<string> { typeof(Behaviour).Assembly.Location };
                if (module.Profile == "editor")
                {
                    var editor = Path.Combine(AppContext.BaseDirectory, "Ara.Editor.dll");
                    if (File.Exists(editor)) extra.Add(editor);
                }
                var compiler = new ModuleCompiler(name, extra);
                var (driver, final, generatorDiagnostics, _) = compilations.Generate(assembly, configuration, compiler, module.Hot,
                    sources.Select(s => (Path.Combine(root, s.Source.StoragePath), s.Text)).ToArray(), module.References.Select(r => builds[r].Reference!));
                // Analysis and emission share immutable Roslyn state. Keep both off the
                // game thread, and let emit supply compiler diagnostics in the same pass.
                var analyzing = Task.Run(() => new AnalyzerSet().AnalyzeProject(final, module.Profile));
                var (image, compilerDiagnostics) = ModuleCompiler.EmitWithDiagnostics(final);
                var problems = generatorDiagnostics.Concat(compilerDiagnostics).Concat(analyzing.GetAwaiter().GetResult())
                    .Where(Diagnostics.Reportable).ToArray();
                validated = true;
                foreach (var diagnostic in problems) rows.Add(ToJson(diagnostic));
                if (problems.Any(d => d.Severity == DiagnosticSeverity.Error)) { build.Module = module with { Result = "failed", Schema = null, Fingerprint = null }; return; }
                if (image is null)
                {
                    build.Module = module with { Result = "failed", Schema = null, Fingerprint = null };
                    return;
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
                build.Reference = MetadataReference.CreateFromImage(image.Pe);
                build.Module = module with { Schema = document, Result = "compiled", Fingerprint = fingerprint };
                return;
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException)
            {
                rows.Add(ProjectInventory.Error("ARA1003", error.Message, module.Directory));
                validated = false;
                build.Module = module with { Result = "failed", Schema = null, Fingerprint = null };
                return;
            }
            finally
            {
                var result = rows.Any(d => (string?)d["severity"] == "error") ? "errors" : validated ? "success" : "cancelled";
                build.Diagnostics.AddRange(attempt.Complete(rows, result, validated));
            }
        }

        var remaining = builds.ToDictionary(p => p.Key, p => p.Value.Module.References.Length, StringComparer.OrdinalIgnoreCase);
        var dependents = builds.Keys.ToDictionary(name => name, _ => new List<BuildState>(), StringComparer.OrdinalIgnoreCase);
        foreach (var build in orderedBuilds)
            foreach (var dependency in build.Module.References) dependents[dependency].Add(build);
        var ready = orderedBuilds.Where(b => remaining[b.Module.Name] == 0).ToArray();
        var completed = 0;
        while (ready.Length > 0)
        {
            if (maxParallelism == 1 || ready.Length == 1)
                foreach (var build in ready) Build(build);
            else
                Parallel.ForEach(ready, new ParallelOptions { MaxDegreeOfParallelism = maxParallelism }, Build);
            completed += ready.Length;
            var next = new List<BuildState>();
            foreach (var build in ready)
                foreach (var dependent in dependents[build.Module.Name])
                    if (--remaining[dependent.Module.Name] == 0) next.Add(dependent);
            ready = next.OrderBy(b => b.Module.Name, StringComparer.Ordinal).ToArray();
        }
        if (completed != builds.Count) throw new InvalidOperationException("the validated module graph has no dependency frontier");
        // Completion order is deliberately invisible to the protocol and consumers.
        return inventory with
        {
            Modules = inventory.Modules.Select(m => builds.TryGetValue(m.Name, out var b) ? b.Module : m).ToArray(),
            Diagnostics = [.. inventory.Diagnostics, .. orderedBuilds.SelectMany(b => b.Diagnostics)],
        };
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

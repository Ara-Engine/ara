using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Hosting;

namespace Ara.CompileServer;

/// <summary>Root name comes from Rust's ara_doc::AUTHORING_ROOT, never a second constant.</summary>
internal sealed record InventoryRequest(string Project, string AuthoringRoot);
internal sealed record ModuleInputs(string Project, string AuthoringRoot, string Name)
{
    internal static ModuleInputs? Read(string directory)
    {
        var path = System.IO.Path.Combine(directory, "module.inputs.json");
        var inputs = File.Exists(path) ? JsonSerializer.Deserialize<ModuleInputs>(File.ReadAllText(path), new JsonSerializerOptions { PropertyNameCaseInsensitive = true }) : null;
        return inputs is null ? null : inputs with { Project = ProjectInventory.ProjectPath(inputs.Project) };
    }
    internal ProjectInventoryResult Scan() => ProjectInventory.Scan(new(Project, AuthoringRoot));
}
internal sealed record SourceFile(string Path, string StoragePath, string Module, bool Editor);
internal sealed record ProjectModule(string Name, string Directory, bool Hot, string[] References, string Profile,
    string StatePath, string SchemaPath, string AssemblyPath, SourceFile[] Sources, JsonNode? Schema = null, string Result = "unbuilt", string? Fingerprint = null);
internal sealed record ProjectInventoryResult(ProjectModule[] Modules, SourceFile[] Sources, JsonObject[] Diagnostics);

/// <summary>The one module/source ownership inventory, shared by the compiler and Rust consumers.</summary>
internal static class ProjectInventory
{
    // Rust canonical paths use Win32's extended prefix. Normal paths let .NET
    // combine the contract's portable '/' storage paths on Windows as well.
    internal static string ProjectPath(string path) => Path.GetFullPath(path.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)
        ? @"\\" + path[8..] : path.StartsWith(@"\\?\", StringComparison.Ordinal) ? path[4..] : path);
    internal const string ManifestSuffix = ".module.json";
    internal const string EditorFolder = "editor";
    sealed record Owner(string Name, string Directory, bool Hot, string[] References, bool EditorManifest);
    sealed class Module(Owner owner, bool editor, bool automaticEditor)
    {
        internal readonly Owner Owner = owner;
        internal readonly bool Editor = editor;
        internal readonly bool AutomaticEditor = automaticEditor;
        internal readonly List<SourceFile> Sources = [];
        internal string Name => Owner.Name + (Editor ? ".editor" : "");
        internal string[] References => AutomaticEditor ? [Owner.Name, .. Owner.References] : Owner.References;
    }

    internal static JsonObject Error(string code, string message, string? file = null)
    {
        var result = new JsonObject { ["code"] = code, ["severity"] = "error", ["message"] = message };
        if (file is not null) result["location"] = new JsonObject { ["file"] = file.Replace('\\', '/') };
        return result;
    }

    internal static ProjectInventoryResult Scan(InventoryRequest request)
    {
        var diagnostics = new List<JsonObject>();
        var modules = new Dictionary<string, Module>(StringComparer.OrdinalIgnoreCase);
        var declared = new Dictionary<string, Owner>(StringComparer.OrdinalIgnoreCase);
        var project = ProjectPath(request.Project);
        if (string.IsNullOrWhiteSpace(request.AuthoringRoot) || request.AuthoringRoot.Contains('/') || request.AuthoringRoot.Contains('\\') || request.AuthoringRoot is "." or "..")
            return new([], [], [Error("ARA1105", "authoring root must be one directory name")]);
        var authored = Path.Combine(project, request.AuthoringRoot);
        string Storage(string path) => Path.GetRelativePath(project, path).Replace('\\', '/');

        Module Get(Owner owner, bool editor, bool automaticEditor)
        {
            var candidate = new Module(owner, editor, automaticEditor);
            if (modules.TryGetValue(candidate.Name, out var found))
            {
                if (!ReferenceEquals(found.Owner, owner)) diagnostics.Add(Error("ARA1105", $"module name `{candidate.Name}` is not unique (case-insensitive)", owner.Directory));
                return found;
            }
            modules.Add(candidate.Name, candidate);
            if (automaticEditor) Get(owner, false, false); // its gameplay assembly exists even when empty
            return candidate;
        }

        void Walk(string directory, Owner owner, bool editor)
        {
            if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0) return;
            if (directory != authored && File.Exists(Path.Combine(directory, "ara.project.json"))) return;
            editor |= Path.GetFileName(directory) == EditorFolder;
            var entries = Directory.GetFileSystemEntries(directory).Order(StringComparer.Ordinal).ToArray();
            var manifests = entries.Where(p => p.EndsWith(ManifestSuffix, StringComparison.OrdinalIgnoreCase) && File.Exists(p)).ToArray();
            if (manifests.Length > 1) { diagnostics.Add(Error("ARA1105", "one module manifest is allowed per folder", directory)); return; }
            if (manifests.Length == 1)
            {
                var manifest = manifests[0];
                try
                {
                    if (Path.GetFileName(manifest) != Path.GetFileName(manifest).ToLowerInvariant()) throw new JsonException("module manifest filenames must be lowercase");
                    using var json = JsonDocument.Parse(File.ReadAllText(manifest));
                    if (json.RootElement.ValueKind != JsonValueKind.Object) throw new JsonException("module manifest must be an object");
                    var properties = json.RootElement.EnumerateObject().ToArray();
                    if (properties.Select(p => p.Name).Distinct(StringComparer.Ordinal).Count() != properties.Length) throw new JsonException("duplicate module manifest key");
                    if (properties.Any(p => p.Name is not ("name" or "references" or "hot"))) throw new JsonException("module manifest accepts only name, references and hot");
                    var name = json.RootElement.GetProperty("name").GetString() ?? "";
                    if (name.Length is 0 or > 128 || name.Any(c => !char.IsAsciiLetterOrDigit(c) && c is not ('.' or '_' or '-')) || name is "." or "..") throw new JsonException("invalid module name");
                    if (Path.GetFileName(manifest) != name.ToLowerInvariant() + ManifestSuffix) throw new JsonException("manifest filename must be the lowercase module name plus .module.json");
                    var references = json.RootElement.TryGetProperty("references", out var refs) ? refs.EnumerateArray().Select(r => r.GetString() ?? throw new JsonException("module references must be names")).ToArray() : [];
                    var hot = json.RootElement.TryGetProperty("hot", out var h) && h.GetBoolean();
                    owner = new Owner(name, directory, hot, references, editor);
                    if (!declared.TryAdd(name, owner)) diagnostics.Add(Error("ARA1105", $"module name `{name}` is not unique (case-insensitive)", manifest));
                    Get(owner, editor, false);
                }
                catch (Exception error) when (error is JsonException or InvalidOperationException or KeyNotFoundException)
                { diagnostics.Add(Error("ARA1105", error.Message, manifest)); return; }
            }
            if (File.Exists(Path.Combine(directory, "ara.module.toml")))
            { diagnostics.Add(Error("ARA1105", "legacy ara.module.toml must be converted with ara project migrate", directory)); return; }
            foreach (var path in entries)
            {
                var attributes = File.GetAttributes(path);
                if ((attributes & FileAttributes.ReparsePoint) != 0) continue;
                if ((attributes & FileAttributes.Directory) != 0)
                {
                    var name = Path.GetFileName(path);
                    if (name is ".ara" or ".git") continue;
                    Walk(path, owner, editor);
                }
                else if (Path.GetExtension(path).Equals(".cs", StringComparison.OrdinalIgnoreCase))
                {
                    var module = Get(owner, editor, editor && !owner.EditorManifest);
                    module.Sources.Add(new SourceFile(Path.GetRelativePath(authored, path).Replace('\\', '/'), Storage(path), module.Name, editor));
                }
            }
        }

        try
        {
            if (Directory.Exists(authored)) Walk(authored, new Owner("game", authored, false, [], false), false);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        { diagnostics.Add(Error("ARA1003", error.Message, authored)); }

        // Resolve once to the canonical spelling; neither runtime nor export guesses missing refs.
        var resolved = new Dictionary<string, string[]>(StringComparer.OrdinalIgnoreCase);
        foreach (var (name, module) in modules.OrderBy(p => p.Key, StringComparer.Ordinal))
        {
            var references = new List<string>();
            foreach (var reference in module.References.Distinct(StringComparer.OrdinalIgnoreCase))
            {
                if (!modules.TryGetValue(reference, out var target) && !(module.Editor && modules.TryGetValue(reference + ".editor", out target)))
                {
                    // A gameplay reference to an editor manifest gets an explicit domain error.
                    var message = declared.TryGetValue(reference, out var declaration) && declaration.EditorManifest
                        ? $"gameplay module `{name}` cannot reference editor module `{reference}`"
                        : $"module `{name}` references unknown module `{reference}`";
                    diagnostics.Add(Error(AraCodes.UnknownModule, message, module.Owner.Directory));
                    continue;
                }
                if (!module.Editor && target.Editor) { diagnostics.Add(Error(AraCodes.UnknownModule, $"gameplay module `{name}` cannot reference editor module `{reference}`", module.Owner.Directory)); continue; }
                references.Add(target.Name);
            }
            resolved[name] = [.. references.Order(StringComparer.Ordinal)];
        }
        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var active = new List<string>();
        void Visit(string name)
        {
            if (active.Contains(name, StringComparer.OrdinalIgnoreCase)) { diagnostics.Add(Error(AraCodes.ModuleCycle, "module reference cycle: " + string.Join(" -> ", active.Append(name)))); return; }
            if (!visited.Add(name)) return;
            active.Add(name);
            foreach (var reference in resolved[name]) Visit(reference);
            active.RemoveAt(active.Count - 1);
        }
        foreach (var name in modules.Keys.Order(StringComparer.Ordinal)) Visit(name);
        var result = modules.Values.OrderBy(m => m.Name, StringComparer.Ordinal).Select(m =>
        {
            var state = ".ara/scripts/" + m.Name;
            var module = new ProjectModule(m.Name, Storage(m.Owner.Directory), m.Owner.Hot, resolved[m.Name], m.Editor ? "editor" : "gameplay", state,
                state + "/" + m.Name + ".types.json", state + "/bin/" + m.Name + ".dll", [.. m.Sources.OrderBy(s => s.Path, StringComparer.Ordinal)]);
            var marker = Path.Combine(project, state, "published.sha256");
            if (File.Exists(marker))
            {
                try
                {
                    var lines = File.ReadAllLines(marker);
                    var schema = File.ReadAllBytes(Path.Combine(project, module.SchemaPath));
                    var assembly = File.ReadAllBytes(Path.Combine(project, module.AssemblyPath));
                    static string Hash(byte[] bytes) => Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(bytes));
                    if (lines.Length == 3 && Hash(schema) == lines[1] && Hash(assembly) == lines[2])
                        return module with { Schema = JsonNode.Parse(schema), Result = "published", Fingerprint = lines[0] };
                }
                catch (Exception error) when (error is IOException or JsonException) { /* rebuild disposable derived output */ }
            }
            return module;
        }).ToArray();
        return new(result, [.. result.SelectMany(m => m.Sources).OrderBy(s => s.Path, StringComparer.Ordinal)], [.. diagnostics]);
    }
}

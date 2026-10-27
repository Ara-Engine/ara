using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Editor;

namespace Ara.CompileServer;

internal sealed record EditorRow(string Id, string Name, string? Parent = null, string Order = "", Dictionary<string, JsonElement>? Components = null);
internal sealed record EditorSceneInput(EditorRow[] Entities);
internal sealed record EditorRequest(string Project, string AuthoringRoot, string Operation = "catalog", string? Command = null,
    EditorSceneInput? Scene = null, string[]? Selection = null, string[]? Reserved = null, string? Generation = null);
internal sealed record EditorResult(EditorCommand[] Commands, EditorMenu[] Menus, AuthoringCall[] Authoring, string[]? Selection,
    JsonObject[] Diagnostics, string? Generation = null, bool Accepted = false, ProjectInventoryResult? Inventory = null);

/// <summary>Editor-only compilation and callbacks in a disposable tool process, never the player.</summary>
internal static class EditorRunner
{
    internal static EditorResult Run(EditorRequest request)
    {
        using var session = new Session();
        return session.Run(request);
    }

    // One hidden worker retains the last good loaded registry. A failed candidate
    // never replaces callbacks or menu metadata; invocation does not compile again.
    internal sealed class Session : IDisposable
    {
        Extensions? _loader;
        EditorRegistry? _registry;
        string? _project;
        string? _generation;

        public EditorResult Run(EditorRequest request)
        {
            try
            {
                var root = ProjectInventory.ProjectPath(request.Project);
                if (_project is not null && _project != root) throw new ArgumentException("an editor worker belongs to one project");
                _project = root;
                ProjectInventoryResult? inventory = null;
                if (_registry is null || request.Operation == "catalog")
                {
                    inventory = ProjectCompiler.Compile(new(root, request.AuthoringRoot, "editor"));
                    if (inventory.Diagnostics.Any(d => (string?)d["severity"] == "error")) return Result(inventory.Diagnostics, inventory: inventory);
                    var generation = Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(
                        string.Join('\n', inventory.Modules.Select(m => m.Name + ":" + m.Fingerprint)))));
                    if (_registry is not null && _generation == generation) return Result(inventory.Diagnostics, accepted: true, inventory: inventory);
                    var paths = inventory.Modules.ToDictionary(m => m.Name, m => Path.Combine(root, m.AssemblyPath), StringComparer.OrdinalIgnoreCase);
                    var candidate = new Extensions(paths);
                    try
                    {
                        var registry = new EditorRegistry(request.Reserved);
                        foreach (var module in inventory.Modules.Where(m => m.Profile == "editor"))
                        {
                            var assembly = candidate.LoadFromAssemblyName(new AssemblyName(module.Name));
                            foreach (var type in assembly.GetTypes().Where(t => !t.IsAbstract && typeof(IEditorExtension).IsAssignableFrom(t)).OrderBy(t => t.FullName, StringComparer.Ordinal))
                                ((IEditorExtension)(Activator.CreateInstance(type) ?? throw new InvalidOperationException($"cannot construct editor extension {type.FullName}"))).Register(registry);
                        }
                        _loader?.Unload();
                        _loader = candidate;
                        _registry = registry;
                        _generation = generation;
                    }
                    catch { candidate.Unload(); throw; }
                }
                if (request.Operation == "catalog") return Result(inventory?.Diagnostics ?? [], accepted: true, inventory: inventory);
                if (request.Operation != "invoke") throw new ArgumentException("editor operation must be catalog or invoke");
                if (request.Generation is not null && request.Generation != _generation)
                    return Result([ProjectInventory.Error("ARA1501", "editor extension generation changed; refresh the command registry")]);
                var scene = new EditorScene((request.Scene?.Entities ?? []).Select(e => new SceneEntity(new(e.Id), e.Name,
                    e.Parent is null ? null : new EntityId(e.Parent), e.Order, e.Components ?? [])));
                var selection = new EditorSelection((request.Selection ?? []).Select(id => new EntityId(id)));
                var context = new EditorContext(scene, selection);
                _registry!.Invoke(request.Command ?? throw new ArgumentException("missing command id"), context);
                return new([.. _registry.Commands], [.. _registry.Menus], [.. context.Commands.Calls], selection.Requested?.Select(id => id.Value).ToArray(), [], _generation, true, inventory);
            }
            catch (Exception error)
            {
                if (request.Operation == "invoke" && request.Command is { } command) _registry?.Disable(command, error.GetBaseException().Message);
                return Result([ProjectInventory.Error(request.Operation == "invoke" ? "ARA0912" : "ARA0920", error.GetBaseException().Message)]);
            }
        }

        EditorResult Result(JsonObject[] diagnostics, bool accepted = false, ProjectInventoryResult? inventory = null) =>
            new(_registry?.Commands.ToArray() ?? [], _registry?.Menus.ToArray() ?? [], [], null, diagnostics, _generation, accepted, inventory);

        public void Dispose()
        {
            _registry = null;
            _loader?.Unload();
            _loader = null;
        }
    }

    sealed class Extensions(Dictionary<string, string> paths) : AssemblyLoadContext(isCollectible: true)
    {
        readonly Dictionary<string, byte[]> _assemblies = paths.ToDictionary(p => p.Key, p => File.ReadAllBytes(p.Value), StringComparer.OrdinalIgnoreCase);
        protected override Assembly? Load(AssemblyName name)
        {
            if (name.Name == typeof(EditorRegistry).Assembly.GetName().Name) return typeof(EditorRegistry).Assembly;
            if (name.Name == typeof(Behaviour).Assembly.GetName().Name) return typeof(Behaviour).Assembly;
            if (name.Name is null || !_assemblies.TryGetValue(name.Name, out var bytes)) return null;
            using var pe = new MemoryStream(bytes, writable: false);
            return LoadFromStream(pe);
        }
    }
}

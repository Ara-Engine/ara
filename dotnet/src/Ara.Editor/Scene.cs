using System.Collections.ObjectModel;
using System.Text.Json;

namespace Ara.Editor;

/// <summary>An authored entity address, including a prefab address or batch symbol.</summary>
/// <param name="Value">Stable entity address, or a newly created command symbol.</param>
public readonly record struct EntityId(string Value);

/// <summary>One immutable entity from the authority's scene snapshot.</summary>
public sealed class SceneEntity
{
    /// <summary>Creates an immutable scene-query row; this never changes a project.</summary>
    public SceneEntity(EntityId id, string name, EntityId? parent, string order, IReadOnlyDictionary<string, JsonElement> components)
    {
        Id = id; Name = name; Parent = parent; Order = order;
        Components = new ReadOnlyDictionary<string, JsonElement>(components.ToDictionary(p => p.Key, p => p.Value.Clone(), StringComparer.Ordinal));
    }
    /// <summary>Stable authored address.</summary>
    public EntityId Id { get; }
    /// <summary>Display name.</summary>
    public string Name { get; }
    /// <summary>Parent address, or null for a root.</summary>
    public EntityId? Parent { get; }
    /// <summary>Sibling ordering key.</summary>
    public string Order { get; }
    /// <summary>Read-only component JSON. Mutations use <see cref="AuthoringCommands"/>.</summary>
    public IReadOnlyDictionary<string, JsonElement> Components { get; }
}

/// <summary>Read-only, deterministic scene queries over the invocation snapshot.</summary>
public sealed class EditorScene
{
    readonly Dictionary<EntityId, SceneEntity> _entities;
    /// <summary>Builds a read-only query view over an authority snapshot.</summary>
    public EditorScene(IEnumerable<SceneEntity> entities)
    {
        var ordered = entities.OrderBy(e => e.Order, StringComparer.Ordinal).ThenBy(e => e.Id.Value, StringComparer.Ordinal).ToArray();
        Entities = Array.AsReadOnly(ordered);
        _entities = ordered.ToDictionary(e => e.Id);
    }
    /// <summary>Every entity, ordered by sibling key and address.</summary>
    public IReadOnlyList<SceneEntity> Entities { get; }
    /// <summary>Find by stable address, exact name or root-relative name path; ambiguous names fail.</summary>
    public SceneEntity? Find(string selector)
    {
        selector = selector.StartsWith("name:", StringComparison.Ordinal) ? selector[5..] : selector;
        selector = selector.Trim('/');
        if (_entities.TryGetValue(new(selector), out var identified)) return identified;
        var found = Entities.Where(e => selector.Contains('/') ? Path(e) == selector : e.Name == selector).Take(2).ToArray();
        return found.Length > 1 ? throw new InvalidOperationException($"ambiguous entity `{selector}`; use its path or id") : found.SingleOrDefault();
    }
    /// <summary>Parent row, or null for a root or missing address.</summary>
    public SceneEntity? Parent(EntityId entity) => _entities.TryGetValue(entity, out var row) && row.Parent is { } parent ? _entities.GetValueOrDefault(parent) : null;
    /// <summary>Direct children, in deterministic sibling order.</summary>
    public IReadOnlyList<SceneEntity> Children(EntityId entity) => Array.AsReadOnly(Entities.Where(e => e.Parent == entity).ToArray());
    /// <summary>Siblings excluding the entity, in deterministic sibling order.</summary>
    public IReadOnlyList<SceneEntity> Siblings(EntityId entity) => _entities.TryGetValue(entity, out var row)
        ? Array.AsReadOnly(Entities.Where(e => e.Id != entity && e.Parent == row.Parent).ToArray()) : Array.Empty<SceneEntity>();
    string Path(SceneEntity row)
    {
        var names = new List<string>();
        var seen = new HashSet<EntityId>();
        while (true)
        {
            if (!seen.Add(row.Id)) throw new InvalidOperationException("scene snapshot contains a hierarchy cycle");
            names.Add(row.Name);
            if (Parent(row.Id) is not { } parent) break;
            row = parent;
        }
        names.Reverse();
        return string.Join('/', names);
    }
}

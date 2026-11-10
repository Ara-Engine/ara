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
    readonly Lazy<Dictionary<string, SceneEntity?>> _names;
    readonly Lazy<HierarchyIndex> _hierarchy;
    readonly Lazy<ScenePathIndex> _paths;

    /// <summary>Builds the immutable snapshot; derived query indices are created only when used.</summary>
    public EditorScene(IEnumerable<SceneEntity> entities)
    {
        var ordered = entities.OrderBy(e => e.Order, StringComparer.Ordinal).ThenBy(e => e.Id.Value, StringComparer.Ordinal).ToArray();
        Entities = Array.AsReadOnly(ordered);
        _entities = ordered.ToDictionary(e => e.Id);
        _names = new(() => Names(ordered));
        _hierarchy = new(() => new HierarchyIndex(ordered));
        _paths = new(() => new ScenePathIndex(ordered, _entities));
    }

    /// <summary>Every entity, ordered by sibling key and address.</summary>
    public IReadOnlyList<SceneEntity> Entities { get; }

    /// <summary>Find by stable address, exact name or root-relative name path; ambiguous names fail.</summary>
    public SceneEntity? Find(string selector)
    {
        selector = selector.StartsWith("name:", StringComparison.Ordinal) ? selector[5..] : selector;
        selector = selector.Trim('/');
        if (_entities.TryGetValue(new(selector), out var identified)) return identified;
        if (selector.Contains('/')) return _paths.Value.Find(selector);
        if (!_names.Value.TryGetValue(selector, out var named)) return null;
        return named ?? throw Ambiguous(selector);
    }

    /// <summary>Parent row, or null for a root or missing address.</summary>
    public SceneEntity? Parent(EntityId entity) => _entities.TryGetValue(entity, out var row)
        && row.Parent is { } parent ? _entities.GetValueOrDefault(parent) : null;

    /// <summary>Direct children in deterministic sibling order, backed by the immutable snapshot.</summary>
    public IReadOnlyList<SceneEntity> Children(EntityId entity) => _hierarchy.Value.Children.GetValueOrDefault(entity) ?? Array.Empty<SceneEntity>();

    /// <summary>Siblings excluding the entity, in deterministic sibling order, without copying the sibling group.</summary>
    public IReadOnlyList<SceneEntity> Siblings(EntityId entity) => _hierarchy.Value.Siblings.GetValueOrDefault(entity) ?? Array.Empty<SceneEntity>();

    internal static InvalidOperationException Ambiguous(string selector) => new($"ambiguous entity `{selector}`; use its path or id");

    static Dictionary<string, SceneEntity?> Names(SceneEntity[] entities)
    {
        var names = new Dictionary<string, SceneEntity?>(entities.Length, StringComparer.Ordinal);
        foreach (var entity in entities)
            if (!names.TryAdd(entity.Name, entity)) names[entity.Name] = null;
        return names;
    }

    sealed class HierarchyIndex
    {
        internal readonly Dictionary<EntityId, IReadOnlyList<SceneEntity>> Children = [];
        internal readonly Dictionary<EntityId, IReadOnlyList<SceneEntity>> Siblings;
        internal HierarchyIndex(SceneEntity[] entities)
        {
            Siblings = new(entities.Length);
            foreach (var group in entities.GroupBy(e => e.Parent))
            {
                var rows = group.ToArray();
                if (group.Key is { } parent) Children.Add(parent, Array.AsReadOnly(rows));
                if (rows.Length < 2) continue;
                for (var i = 0; i < rows.Length; i++) Siblings.Add(rows[i].Id, new SiblingView(rows, i));
            }
        }
    }

    sealed class SiblingView(SceneEntity[] rows, int omitted) : IReadOnlyList<SceneEntity>
    {
        public int Count => rows.Length - 1;
        public SceneEntity this[int index] => (uint)index < (uint)Count
            ? rows[index < omitted ? index : index + 1] : throw new ArgumentOutOfRangeException(nameof(index));
        public IEnumerator<SceneEntity> GetEnumerator()
        {
            for (var i = 0; i < rows.Length; i++) if (i != omitted) yield return rows[i];
        }
        System.Collections.IEnumerator System.Collections.IEnumerable.GetEnumerator() => GetEnumerator();
    }
}

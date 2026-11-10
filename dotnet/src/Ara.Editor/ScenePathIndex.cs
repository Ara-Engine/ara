namespace Ara.Editor;

// Matches names along hierarchy edges without storing a full path per entity.
// Deep snapshots therefore retain linear storage instead of quadratic path strings.
internal sealed class ScenePathIndex
{
    readonly Dictionary<EntityId, NameIndex> _children = [];
    readonly NameIndex _roots;
    readonly bool _cyclic;

    internal ScenePathIndex(SceneEntity[] entities, Dictionary<EntityId, SceneEntity> byId)
    {
        foreach (var group in entities.Where(e => e.Parent is not null).GroupBy(e => e.Parent!.Value))
            _children.Add(group.Key, new NameIndex(group));
        _roots = new NameIndex(entities.Where(e => e.Parent is not { } parent || !byId.ContainsKey(parent)));
        _cyclic = HasCycle(entities, byId);
    }

    internal SceneEntity? Find(string selector)
    {
        if (_cyclic) throw new InvalidOperationException("scene snapshot contains a hierarchy cycle");
        SceneEntity? found = null;
        var pending = new Stack<(NameIndex Names, int Start)>();
        pending.Push((_roots, 0));
        while (pending.TryPop(out var step))
        {
            // Names may contain '/' or be empty, so match complete names at each
            // edge rather than splitting the selector into assumed name segments.
            var lookup = step.Names.Rows.GetAlternateLookup<ReadOnlySpan<char>>();
            foreach (var length in step.Names.Lengths)
            {
                var end = step.Start + length;
                if (end > selector.Length) break;
                if (end < selector.Length && selector[end] != '/') continue;
                if (!lookup.TryGetValue(selector.AsSpan(step.Start, length), out var rows)) continue;
                for (var i = -1; i < (rows.More?.Count ?? 0); i++)
                {
                    var row = i < 0 ? rows.First : rows.More![i];
                    if (end == selector.Length)
                    {
                        if (found is not null) throw EditorScene.Ambiguous(selector);
                        found = row;
                    }
                    else if (_children.TryGetValue(row.Id, out var children)) pending.Push((children, end + 1));
                }
            }
        }
        return found;
    }

    static bool HasCycle(SceneEntity[] entities, Dictionary<EntityId, SceneEntity> byId)
    {
        var visits = new Dictionary<EntityId, bool>(byId.Count);
        var path = new List<EntityId>();
        foreach (var entity in entities)
        {
            path.Clear();
            var current = entity.Id;
            while (byId.TryGetValue(current, out var row))
            {
                if (visits.TryGetValue(current, out var complete))
                {
                    if (!complete) return true;
                    break;
                }
                visits.Add(current, false);
                path.Add(current);
                if (row.Parent is not { } parent) break;
                current = parent;
            }
            foreach (var visited in path) visits[visited] = true;
        }
        return false;
    }

    sealed class NameIndex
    {
        internal readonly Dictionary<string, NameRows> Rows = new(StringComparer.Ordinal);
        internal readonly int[] Lengths;
        internal NameIndex(IEnumerable<SceneEntity> entities)
        {
            foreach (var entity in entities)
            {
                if (!Rows.TryGetValue(entity.Name, out var named)) Rows.Add(entity.Name, new(entity, null));
                else
                {
                    var more = named.More;
                    if (more is null) Rows[entity.Name] = named with { More = more = [] };
                    more.Add(entity);
                }
            }
            Lengths = Rows.Keys.Select(name => name.Length).Distinct().Order().ToArray();
        }
        internal readonly record struct NameRows(SceneEntity First, List<SceneEntity>? More);
    }
}

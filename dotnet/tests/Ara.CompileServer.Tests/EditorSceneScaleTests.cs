using Ara.Editor;
using System.Text.Json;

namespace Ara.CompileServer.Tests;

public sealed class EditorSceneScaleTests
{
    static SceneEntity Entity(string id, string name, string? parent = null, string order = "") =>
        new(new(id), name, parent is null ? null : new EntityId(parent), order, new Dictionary<string, JsonElement>());

    [Fact]
    public void Hierarchy_queries_preserve_order_and_share_immutable_snapshot_views()
    {
        var first = Entity("e_first", "First", "e_root", "a0");
        var second = Entity("e_second", "Second", "e_root", "a0");
        var last = Entity("e_last", "Last", "e_root", "b0");
        var root = Entity("e_root", "Root");
        var input = new List<SceneEntity> { last, second, root, first };
        var scene = new EditorScene(input);
        input.Clear();
        Assert.Equal([first, second, last], scene.Children(root.Id));
        Assert.Equal([first, last], scene.Siblings(second.Id));
        Assert.Equal(first, scene.Siblings(second.Id)[0]);
        Assert.Equal(last, scene.Siblings(second.Id)[1]);
        Assert.Throws<ArgumentOutOfRangeException>(() => scene.Siblings(second.Id)[-1]);
        Assert.Throws<ArgumentOutOfRangeException>(() => scene.Siblings(second.Id)[2]);
        Assert.Same(scene.Children(root.Id), scene.Children(root.Id));
        Assert.Same(scene.Siblings(first.Id), scene.Siblings(first.Id));
        Assert.Empty(scene.Siblings(root.Id));
        Assert.Empty(scene.Siblings(new("missing")));
        Assert.False(scene.Children(root.Id) is SceneEntity[]);
        Assert.False(scene.Siblings(first.Id) is SceneEntity[]);
        Assert.Same(root, scene.Parent(first.Id));
    }

    [Fact]
    public void Name_and_path_queries_preserve_ambiguity_and_stable_address_precedence()
    {
        var left = Entity("e_left", "Left");
        var right = Entity("e_right", "Right");
        var a = Entity("e_a", "Leaf", "e_left");
        var b = Entity("e_b", "Leaf", "e_right");
        var misleading = Entity("e_other", "e_a");
        var scene = new EditorScene([left, right, a, b, misleading]);
        Assert.Same(a, scene.Find("e_a"));
        Assert.Same(a, scene.Find("name:e_a"));
        Assert.Same(a, scene.Find("/Left/Leaf/"));
        Assert.Same(b, scene.Find("Right/Leaf"));
        Assert.Throws<InvalidOperationException>(() => scene.Find("Leaf"));
        Assert.Null(scene.Find("Missing/Leaf"));
        Assert.Null(scene.Find("Left/Missing"));
        var ambiguousPath = new EditorScene([left, a, Entity("e_left2", "Left"), Entity("e_c", "Leaf", "e_left2")]);
        Assert.Throws<InvalidOperationException>(() => ambiguousPath.Find("Left/Leaf"));
    }

    [Fact]
    public void Paths_keep_slashes_empty_names_and_dangling_parent_semantics()
    {
        var root = Entity("e_root", "Root");
        var empty = Entity("e_empty", "", "e_root");
        var leaf = Entity("e_leaf", "Leaf", "e_empty");
        var slash = Entity("e_slash", "Root/Group");
        var slashLeaf = Entity("e_slashleaf", "Leaf", "e_slash");
        var orphan = Entity("e_orphan", "Orphan", "e_missing");
        var orphanSibling = Entity("e_orphan2", "Other", "e_missing");
        var orphanChild = Entity("e_orphanchild", "Child", "e_orphan");
        var scene = new EditorScene([root, empty, leaf, slash, slashLeaf, orphan, orphanSibling, orphanChild]);
        Assert.Same(leaf, scene.Find("Root//Leaf"));
        Assert.Same(slash, scene.Find("Root/Group"));
        Assert.Same(slashLeaf, scene.Find("Root/Group/Leaf"));
        Assert.Same(orphanChild, scene.Find("Orphan/Child"));
        Assert.Null(scene.Parent(orphan.Id));
        Assert.Equal([orphan, orphanSibling], scene.Children(new("e_missing")));
        Assert.Equal([orphanSibling], scene.Siblings(orphan.Id));
        var ambiguous = new EditorScene([root, slash, Entity("e_group", "Group", "e_root")]);
        Assert.Throws<InvalidOperationException>(() => ambiguous.Find("Root/Group"));
    }

    [Fact]
    public void A_deep_snapshot_uses_linear_storage_and_iterative_path_resolution()
    {
        const int count = 20_000;
        var rows = Enumerable.Range(0, count).Select(i => Entity($"e_{i}", "Node", i == 0 ? null : $"e_{i - 1}")).ToArray();
        var scene = new EditorScene(rows);
        Assert.Same(rows[^1], scene.Find(string.Join('/', Enumerable.Repeat("Node", count))));
        Assert.Same(rows[^2], scene.Parent(rows[^1].Id));
        Assert.Single(scene.Children(rows[^2].Id));
    }

    [Fact]
    public void Cycles_fail_path_queries_without_breaking_id_or_name_lookup()
    {
        var a = Entity("e_a", "A", "e_b");
        var b = Entity("e_b", "B", "e_a");
        var scene = new EditorScene([a, b]);
        Assert.Same(a, scene.Find("e_a"));
        Assert.Same(a, scene.Find("A"));
        Assert.Throws<InvalidOperationException>(() => scene.Find("A/B"));
    }

    [Fact]
    public void Repeated_hierarchy_queries_do_not_allocate_per_entity_results()
    {
        var rows = Enumerable.Range(0, 1000).Select(i => Entity($"e_{i}", $"Node{i}", "e_root")).ToArray();
        var scene = new EditorScene([Entity("e_root", "Root"), .. rows]);
        for (var i = 0; i < 100; i++) { _ = scene.Children(new("e_root")).Count; _ = scene.Siblings(rows[0].Id).Count; }
        var before = GC.GetAllocatedBytesForCurrentThread();
        var count = 0;
        for (var i = 0; i < 10_000; i++) count += scene.Children(new("e_root")).Count + scene.Siblings(rows[0].Id).Count;
        var allocated = GC.GetAllocatedBytesForCurrentThread() - before;
        Assert.Equal(19_990_000, count);
        Assert.Equal(0, allocated);
    }
}

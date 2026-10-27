using Ara;

namespace Ara.Examples;

/// <summary>Runtime hierarchy recipes over authoritative ECS components.</summary>
public static class EntityExamples
{
    #region topics/csharp-scene-queries
    /// <summary>Find a named child, traverse its family and queue a keep-local reparent.</summary>
    public static void MoveChild(in Commands commands, Entity destination)
    {
        var child = World.Find("Root/Child"); // Exact names and stable entity ids also work.
        if (child.IsNull) return;
        var parent = World.Parent(child);
        Span<Entity> scratch = stackalloc Entity[32];
        foreach (var sibling in World.Siblings(child, scratch))
            _ = World.Parent(sibling);
        foreach (var descendant in World.Children(parent, scratch))
            _ = descendant.IsAlive;
        commands.SetParent(child, destination); // Applied at the ordinary ECS sync point.
    }
    #endregion
}

#region topics/csharp-destroy
/// <summary>Cleanup state lives in the ECS, independently of behaviour wrappers.</summary>
[Resource]
public partial struct CleanupCount
{
    /// <summary>Number of removed behaviour instances.</summary>
    public int Removed;
}

/// <summary>Receives cleanup after its ECS behaviour component or entity disappears.</summary>
public sealed partial class CleanupExample : Behaviour
{
    /// <summary>Runs once at the next fixed-tick membership sweep, in stable handle order.</summary>
    protected override void OnDestroy() => World.Resource<CleanupCount>().Removed++;
}
#endregion

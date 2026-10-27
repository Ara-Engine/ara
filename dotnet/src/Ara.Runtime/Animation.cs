using Ara.Hosting;

namespace Ara;

/// <summary>Typed inputs to an entity's authored native animation controller.</summary>
public static class Animation
{
    /// <summary>Sets a boolean controller parameter after this gameplay callback.</summary>
    public static void SetBool(Entity entity, string parameter, bool value) => Host.AnimationBool(entity, parameter, value);
}

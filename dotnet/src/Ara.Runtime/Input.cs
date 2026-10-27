using System.Numerics;
using Ara.Hosting;

namespace Ara;

/// <summary>Replayable player actions at the current fixed tick. Behaviours and commands only.</summary>
public static class Input
{
    /// <summary>The input owner assigned by the engine's player controller.</summary>
    public static ulong Player(Entity entity) => Host.InputPlayer(entity);
    /// <summary>Whether a named button is held by the authenticated player.</summary>
    public static bool Button(ulong player, string action) => Host.InputAxis(player, action, 0) != 0;

    /// <summary>A named two-axis action; an absent action is zero.</summary>
    public static Vector2 Axis2(ulong player, string action) =>
        new(Host.InputAxis(player, action, 0), Host.InputAxis(player, action, 1));
}

using Ara.Hosting;

namespace Ara;

/// <summary>Fixed-tick controls for native physics characters.</summary>
public static class Movement
{
    /// <summary>Suppresses desired movement and jump for this tick. Call each tick while downed.</summary>
    public static void Stop(Entity entity) => Host.CharacterStop(entity);
}

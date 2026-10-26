using System.Numerics;
using Ara.Hosting;

namespace Ara;

/// <summary>Server-authoritative goals for authored NavAgent components; paths remain in ECS.</summary>
public static class Navigation
{
    /// <summary>Requests a reachable navmesh route, using the agent's authored profile.</summary>
    public static void GoTo(Entity entity, Vector3 destination) => Host.Navigate(entity, destination, true);

    /// <summary>Clears the agent's route at the next navigation boundary.</summary>
    public static void Stop(Entity entity) => Host.Navigate(entity, Vector3.Zero, false);
}

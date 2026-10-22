using System.Numerics;
using Ara;

namespace Game;

/// <summary>Steering forces that keep the crowd moving.</summary>
public static class Steering
{
    /// <summary>Pushes entities around the world origin, weaker with distance.</summary>
    public static void Swirl(AccelChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:swirl
        float dt = t.Dt;
        Span<Velocity> vel = c.Velocity;
        ReadOnlySpan<Transform> tr = c.Transform;
        for (int i = 0; i < vel.Length; i++)
        {
            Vector3 p = tr[i].Position;
            Vector3 tangent = new(-p.Z, 0f, p.X);
            float r2 = p.X * p.X + p.Z * p.Z + 1f;
            vel[i].Linear += tangent * (Tuning.SwirlStrength / r2) * dt;
        }
    }
}

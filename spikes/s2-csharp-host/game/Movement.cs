using System.Numerics;
using Ara;

namespace Game;

/// <summary>Integrates transforms from velocities and keeps entities inside the play area.</summary>
public static class Movement
{
    /// <summary>position += linear * dt; rotation integrated by angular velocity.</summary>
    public static void Integrate(MoveChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:integrate
        float dt = t.Dt;
        Span<Transform> tr = c.Transform;
        ReadOnlySpan<Velocity> vel = c.Velocity;
        for (int i = 0; i < tr.Length; i++)
        {
            ref Transform x = ref tr[i];
            ref readonly Velocity v = ref vel[i];
            x.Position += v.Linear * dt;
            x.Rotation = Spin(x.Rotation, v.Angular, dt);
        }
    }

    /// <summary>First-order quaternion integration of angular velocity <paramref name="w"/> over <paramref name="dt"/>.</summary>
    static Quaternion Spin(Quaternion q, Vector3 w, float dt)
    {
        Quaternion dq = new Quaternion(w.X, w.Y, w.Z, 0f) * q;
        float h = 0.5f * dt;
        return Quaternion.Normalize(new Quaternion(q.X + h * dq.X, q.Y + h * dq.Y, q.Z + h * dq.Z, q.W + h * dq.W));
    }

    /// <summary>Wraps positions on X and Z into the play area.</summary>
    public static void Bounds(PlaceChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:bounds
        Span<Transform> tr = c.Transform;
        for (int i = 0; i < tr.Length; i++)
        {
            ref Vector3 p = ref tr[i].Position;
            p.X = Wrap(p.X, Tuning.HalfExtent);
            p.Z = Wrap(p.Z, Tuning.HalfExtent);
        }
    }

    static float Wrap(float v, float half) => v > half ? v - 2f * half : v < -half ? v + 2f * half : v;
}

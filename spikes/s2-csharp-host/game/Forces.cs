using System.Numerics;
using Ara;

namespace Game;

/// <summary>Forces that change velocity: gravity with ground bounce, and drag.</summary>
public static class Forces
{
    /// <summary>Applies gravity and bounces entities that fall below the ground plane.</summary>
    public static void Gravity(AccelChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:gravity
        float dt = t.Dt;
        Span<Velocity> vel = c.Velocity;
        ReadOnlySpan<Transform> tr = c.Transform;
        for (int i = 0; i < vel.Length; i++)
        {
            ref Velocity v = ref vel[i];
            v.Linear.Y -= Tuning.Gravity * dt;
            if (tr[i].Position.Y < 0f && v.Linear.Y < 0f) v.Linear.Y = -v.Linear.Y * Tuning.Restitution;
        }
    }

    /// <summary>Frame-rate independent exponential drag on linear and angular velocity.</summary>
    public static void Drag(DampChunk c, in Tick t)
    {
        Probe.Stamp(0); // @stamp:drag
        float k = MathF.Pow(Tuning.LinearDamping, t.Dt * 60f);
        float ka = MathF.Pow(Tuning.AngularDamping, t.Dt * 60f);
        Span<Velocity> vel = c.Velocity;
        for (int i = 0; i < vel.Length; i++)
        {
            ref Velocity v = ref vel[i];
            v.Linear *= k;
            v.Angular *= ka;
        }
    }
}

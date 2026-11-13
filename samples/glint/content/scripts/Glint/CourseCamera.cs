namespace Glint;

/// <summary>
/// The follow camera's director: it turns the rig's yaw toward the route ahead of the ball
/// with a little lag (so the stick always means "along the course"), pulls the camera back and
/// widens the field of view as the ball speeds up, and lets it settle again.
/// </summary>
public sealed partial class CourseCamera : Behaviour
{
    /// <summary>How fast the yaw may turn, degrees per second.</summary>
    [Property, Default(110f)] public partial float TurnRate { get; set; }
    /// <summary>Distance behind the ball at rest, metres.</summary>
    [Property, Default(5.4f)] public partial float RestDistance { get; set; }
    /// <summary>Extra distance at full speed, metres.</summary>
    [Property, Default(2.2f)] public partial float SpeedDistance { get; set; }
    /// <summary>Vertical field of view at rest, degrees.</summary>
    [Property, Default(56f)] public partial float RestFov { get; set; }
    /// <summary>Extra field of view at full speed, degrees.</summary>
    [Property, Default(18f)] public partial float SpeedFov { get; set; }
    /// <summary>How far ahead of the ball, along the route, the camera looks, metres.</summary>
    [Property, Default(9f)] public partial float Lookahead { get; set; }
    /// <summary>Route segment the ball is on (searched near the last value, so loops cannot jump).</summary>
    [Property] public partial int Segment { get; set; }
    /// <summary>The ball's speed that counts as full speed, m/s.</summary>
    [Property, Default(16f)] public partial float FullSpeed { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        var ball = World.Find("Ball");
        if (ball.IsNull) return;
        var position = World.Read<PhysicsPose>(ball).Position;
        var velocity = World.Read<Velocity>(ball).Linear;
        var speed = new Vector2(velocity.X, velocity.Z).Length();

        // The route: waypoints in index order.
        Span<Entity> buffer = stackalloc Entity[128];
        Span<Vector3> points = stackalloc Vector3[128];
        var count = 0;
        foreach (var e in World.Query<Waypoint>(buffer))
        {
            var index = World.Read<Waypoint>(e).Index;
            if (index < 0 || index >= 128) continue;
            points[index] = World.Read<Transform>(e).Position;
            count = Math.Max(count, index + 1);
        }
        if (count >= 2)
        {
            // Nearest segment within a window around the current one.
            var best = Math.Clamp(Segment, 0, count - 2);
            var bestDistance = float.MaxValue;
            for (var i = Math.Max(0, Segment - 1); i <= Math.Min(count - 2, Segment + 3); i++)
            {
                var d = DistanceToSegment(position, points[i], points[i + 1]);
                if (d < bestDistance) { bestDistance = d; best = i; }
            }
            if (bestDistance > 12f)
            {
                // The ball was moved (a respawn or a restart): look at the whole route.
                for (var i = 0; i <= count - 2; i++)
                {
                    var d = DistanceToSegment(position, points[i], points[i + 1]);
                    if (d < bestDistance) { bestDistance = d; best = i; }
                }
            }
            Segment = best;
            // The point `Lookahead` metres further along the polyline.
            var ahead = position;
            var remaining = Lookahead;
            var from = ClosestOnSegment(position, points[best], points[best + 1]);
            var segment = best;
            while (segment <= count - 2)
            {
                var to = points[segment + 1];
                var leg = Vector3.Distance(from, to);
                if (leg >= remaining) { ahead = from + Vector3.Normalize(to - from) * remaining; break; }
                remaining -= leg;
                from = to;
                ahead = to;
                segment++;
            }
            var delta = ahead - position;
            if (new Vector2(delta.X, delta.Z).LengthSquared() > 0.25f)
            {
                var target = MathF.Atan2(delta.X, delta.Z) * (180f / MathF.PI);
                ref var rig = ref Get<FollowCamera>();
                var diff = target - rig.Yaw;
                while (diff > 180f) diff -= 360f;
                while (diff < -180f) diff += 360f;
                var max = TurnRate * tick.Dt;
                rig.Yaw += Math.Clamp(diff, -max, max);
            }
        }
        var k = Math.Clamp(speed / FullSpeed, 0f, 1f);
        k = k * k * (3f - 2f * k);
        ref var follow = ref Get<FollowCamera>();
        follow.Distance = RestDistance + SpeedDistance * k;
        Get<Camera>().Fov = RestFov + SpeedFov * k;
    }

    static Vector3 ClosestOnSegment(Vector3 p, Vector3 a, Vector3 b)
    {
        var ab = b - a;
        var t = ab.LengthSquared() < 1e-6f ? 0f : Math.Clamp(Vector3.Dot(p - a, ab) / ab.LengthSquared(), 0f, 1f);
        return a + ab * t;
    }

    static float DistanceToSegment(Vector3 p, Vector3 a, Vector3 b) => Vector3.Distance(p, ClosestOnSegment(p, a, b));
}

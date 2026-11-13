namespace Glint;

/// <summary>
/// Steers the rolling ball with momentum. The ball is a dynamic sphere; this behaviour only
/// writes its velocity (planar acceleration toward the stick along whatever surface it rests
/// on, a jump, rolling spin), so the physics engine still resolves every contact. Grounding
/// and the surface's tilt come from the course's <see cref="Surface"/> boxes, because scripts
/// have no contact events or ray casts.
/// </summary>
public sealed partial class BallController : Behaviour
{
    /// <summary>Top speed along a surface in metres per second.</summary>
    [Property, Default(9.5f)] public partial float TopSpeed { get; set; }
    /// <summary>Acceleration on a surface, m/s squared.</summary>
    [Property, Default(26f)] public partial float GroundAccel { get; set; }
    /// <summary>Acceleration in the air, m/s squared.</summary>
    [Property, Default(6f)] public partial float AirAccel { get; set; }
    /// <summary>Rolling resistance when no direction is held, m/s squared.</summary>
    [Property, Default(6f)] public partial float Resistance { get; set; }
    /// <summary>Upward speed given by a jump, m/s.</summary>
    [Property, Default(6f)] public partial float JumpSpeed { get; set; }
    /// <summary>The ball's radius in metres.</summary>
    [Property, Default(0.5f)] public partial float Radius { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        ref var ball = ref Get<Ball>();
        ball.Player = Input.Player(Entity);
        var position = World.Read<PhysicsPose>(Entity).Position;
        ref var body = ref World.Get<Velocity>(Entity);
        if (ball.Held)
        {
            ball.Speed = 0;
            ball.Grounded = false;
            return;
        }
        var grounded = Course.Resting(position, Radius, out var normal, out var under);
        if (grounded)
        {
            if (!ball.Grounded && ball.FallSpeed > 4f) ball.LandTick = tick.Frame;
            ball.Flying = false;
            ball.LastGroundY = position.Y;
            ball.FallSpeed = 0;
        }
        else ball.FallSpeed = MathF.Max(ball.FallSpeed, -body.Linear.Y);
        // A fall: well below the last surface the ball touched (far below when flying from the cannon).
        var drop = ball.Flying ? 30f : 9f;
        if (position.Y < ball.LastGroundY - drop || position.Y < -90f)
        {
            ball.Falls++;
            ball.Flying = false;
            ball.FallSpeed = 0;
            Course.Teleport(Entity, ball.Spawn);
            ball.LastGroundY = ball.Spawn.Y;
            return;
        }
        ball.Grounded = grounded;
        ball.CoyoteTicks = grounded ? 6 : Math.Max(0, ball.CoyoteTicks - 1);

        // The stick is relative to the camera's yaw (which follows the route).
        var stick = Input.Axis2(ball.Player, "Move");
        var camera = World.Find("Camera");
        var yaw = camera.IsNull ? 0f : World.Read<FollowCamera>(camera).Yaw * (MathF.PI / 180f);
        var forward = new Vector3(MathF.Sin(yaw), 0, MathF.Cos(yaw));
        var right = new Vector3(-MathF.Cos(yaw), 0, MathF.Sin(yaw));
        // No steering during the cannon's shot: the arc is the cannon's.
        var wish = ball.Flying ? Vector3.Zero : forward * stick.Y + right * stick.X;
        if (wish.LengthSquared() > 1f) wish = Vector3.Normalize(wish);

        var carry = grounded && World.Has<Mover>(under) ? Course.MoverVelocity(under, in tick) : Vector3.Zero;
        var up = grounded ? normal : Vector3.UnitY;
        var relative = body.Linear - carry;
        var across = up * Vector3.Dot(relative, up);
        var along = relative - across;
        // Steer along the surface: project the wish onto its plane.
        var plane = wish - up * Vector3.Dot(wish, up);
        var step = tick.Dt;
        var accel = grounded ? GroundAccel : AirAccel;
        if (wish != Vector3.Zero)
        {
            var delta = plane * TopSpeed - along;
            var limit = accel * step;
            along += delta.Length() > limit ? Vector3.Normalize(delta) * limit : delta;
        }
        else if (grounded)
        {
            var slow = Resistance * step;
            along = along.Length() > slow ? along - Vector3.Normalize(along) * slow : Vector3.Zero;
        }
        var velocity = along + across + carry;

        var jump = Input.Button(ball.Player, "Jump");
        if (jump && !ball.JumpHeld && ball.CoyoteTicks > 0)
        {
            velocity.Y = JumpSpeed;
            ball.CoyoteTicks = 0;
            ball.JumpTick = tick.Frame;
        }
        ball.JumpHeld = jump;
        body.Linear = velocity;
        // Rolling without slipping: the spin axis is up x velocity, over the radius.
        if (grounded) body.Angular = Vector3.Cross(up, along) / Radius;
        ball.Speed = new Vector2(along.X, along.Z).Length();
    }
}

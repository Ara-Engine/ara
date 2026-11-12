namespace Glint;

/// <summary>
/// Steers the rolling ball with momentum. The ball is a dynamic sphere; this behaviour
/// only writes its velocity (planar acceleration toward the stick, a jump, rolling spin),
/// so the physics engine still resolves every contact. Grounding comes from the course's
/// <see cref="Surface"/> faces because scripts have no contact events or ray casts.
/// </summary>
public sealed partial class BallController : Behaviour
{
    /// <summary>Top planar speed in metres per second.</summary>
    [Property, Default(8f)] public partial float TopSpeed { get; set; }
    /// <summary>Planar acceleration on the ground, m/s squared.</summary>
    [Property, Default(24f)] public partial float GroundAccel { get; set; }
    /// <summary>Planar acceleration in the air, m/s squared.</summary>
    [Property, Default(7f)] public partial float AirAccel { get; set; }
    /// <summary>Rolling resistance when no direction is held, m/s squared.</summary>
    [Property, Default(5f)] public partial float Resistance { get; set; }
    /// <summary>Upward speed given by a jump, m/s.</summary>
    [Property, Default(7.5f)] public partial float JumpSpeed { get; set; }
    /// <summary>The ball's radius in metres.</summary>
    [Property, Default(0.5f)] public partial float Radius { get; set; }
    /// <summary>Height below which the ball counts as fallen, metres.</summary>
    [Property, Default(-12f)] public partial float KillHeight { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        ref var ball = ref Get<Ball>();
        ball.Player = Input.Player(Entity);
        if (!World.Has<Velocity>(Entity) || !World.Has<PhysicsPose>(Entity)) return;
        var position = World.Read<PhysicsPose>(Entity).Position;
        if (position.Y < KillHeight)
        {
            ball.Falls++;
            Course.Teleport(Entity, ball.Spawn);
            return;
        }
        ref var body = ref World.Get<Velocity>(Entity);
        var grounded = Course.Resting(position, Radius, out _, out var under);
        // A ball resting on a sliding platform moves with it: steer relative to the platform.
        var carry = grounded && World.Has<Mover>(under) ? Course.MoverVelocity(under, in tick) : Vector3.Zero;
        ball.Grounded = grounded;
        ball.CoyoteTicks = grounded ? 6 : Math.Max(0, ball.CoyoteTicks - 1);

        // The stick is relative to the camera's yaw, as the engine's own player rig does it.
        var stick = Input.Axis2(ball.Player, "Move");
        var camera = World.Find("Camera");
        var yaw = camera.IsNull ? 0f : World.Read<FollowCamera>(camera).Yaw * (MathF.PI / 180f);
        var forward = new Vector3(MathF.Sin(yaw), 0, MathF.Cos(yaw));
        var right = new Vector3(-MathF.Cos(yaw), 0, MathF.Sin(yaw));
        var wish = forward * stick.Y + right * stick.X;
        if (wish.LengthSquared() > 1f) wish = Vector3.Normalize(wish);

        var velocity = body.Linear;
        var planar = new Vector3(velocity.X - carry.X, 0, velocity.Z - carry.Z);
        var step = tick.Dt;
        if (wish != Vector3.Zero)
        {
            var delta = wish * TopSpeed - planar;
            var limit = (grounded ? GroundAccel : AirAccel) * step;
            planar += delta.Length() > limit ? Vector3.Normalize(delta) * limit : delta;
        }
        else if (grounded)
        {
            var slow = Resistance * step;
            planar = planar.Length() > slow ? planar - Vector3.Normalize(planar) * slow : Vector3.Zero;
        }
        velocity.X = planar.X + carry.X;
        velocity.Z = planar.Z + carry.Z;

        var jump = Input.Button(ball.Player, "Jump");
        if (jump && !ball.JumpHeld && ball.CoyoteTicks > 0)
        {
            velocity.Y = JumpSpeed;
            ball.CoyoteTicks = 0;
        }
        ball.JumpHeld = jump;
        body.Linear = velocity;
        // Rolling without slipping: spin axis is up x velocity over the radius.
        if (grounded) body.Angular = Vector3.Cross(Vector3.UnitY, planar) / Radius * -1f;
    }
}

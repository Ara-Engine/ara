namespace Glint;

/// <summary>
/// Presentation that rides the ball's speed: a lit trail and sparks that grow above a walking
/// pace, and the roll sound's pitch and volume. It reads the ball and writes only its own entity.
/// </summary>
public sealed partial class BallFx : Behaviour
{
    /// <summary>Speed in m/s where sparks begin.</summary>
    [Property, Default(5f)] public partial float SparkStart { get; set; }
    /// <summary>Speed in m/s where sparks and the roll sound are at full strength.</summary>
    [Property, Default(9.5f)] public partial float SparkFull { get; set; }

    /// <summary>The trail's length at full speed, metres.</summary>
    [Property, Default(4.2f)] public partial float TrailLength { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        var ball = World.Find("Ball");
        if (ball.IsNull) return;
        ref readonly var control = ref World.Read<Ball>(ball);
        var velocity = World.Read<Velocity>(ball).Linear;
        var speed = velocity.Length();
        // The trail lies on the surface behind the ball, stretched by its pace; off the ground
        // or at a walking pace it has no length.
        ref var transform = ref Get<Transform>();
        var pace = control.Grounded ? Math.Clamp((control.Speed - SparkStart) / (SparkFull - SparkStart), 0f, 1f) : 0f;
        transform.Position = World.Read<Transform>(ball).Position - new Vector3(0, 0.46f, 0);
        if (pace > 0f && speed > 0.5f)
        {
            var up = Physics.Raycast(World.Read<Transform>(ball).Position, -Vector3.UnitY, 1.5f, out var floor, ball) ? floor.Normal : Vector3.UnitY;
            transform.Rotation = Course.LookEuler(velocity / speed, up);
        }
        transform.Scale = new Vector3(1f, 1f, TrailLength * pace);
        if (World.Has<VisualEffect>(Entity))
        {
            var k = Math.Clamp((speed - SparkStart) / (SparkFull - SparkStart), 0f, 1f);
            Get<VisualEffect>().Intensity = control.Grounded ? k : 0f;
        }
        if (World.Has<AudioSource>(Entity))
        {
            var k = control.Grounded ? Math.Clamp(control.Speed / SparkFull, 0f, 1f) : 0f;
            ref var audio = ref Get<AudioSource>();
            audio.Gain = 0.4f * k;
            audio.Pitch = 0.6f + 0.6f * k;
        }
    }
}

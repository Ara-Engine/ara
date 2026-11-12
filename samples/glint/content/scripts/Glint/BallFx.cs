namespace Glint;

/// <summary>
/// Presentation that rides the ball's speed: a spark trail that brightens above a walking pace,
/// and the roll sound's pitch and volume. It reads the ball and writes only its own entity.
/// </summary>
public sealed partial class BallFx : Behaviour
{
    /// <summary>Speed in m/s where sparks begin.</summary>
    [Property, Default(8f)] public partial float SparkStart { get; set; }
    /// <summary>Speed in m/s where sparks and the roll sound are at full strength.</summary>
    [Property, Default(18f)] public partial float SparkFull { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        var ball = World.Find("Ball");
        // The physics body's components appear after the first tick.
        if (ball.IsNull || !World.Has<Velocity>(ball)) return;
        ref readonly var control = ref World.Read<Ball>(ball);
        var velocity = World.Read<Velocity>(ball).Linear;
        var speed = velocity.Length();
        Get<Transform>().Position = World.Read<Transform>(ball).Position - new Vector3(0, 0.4f, 0);
        if (World.Has<VisualEffect>(Entity))
        {
            var k = Math.Clamp((speed - SparkStart) / (SparkFull - SparkStart), 0f, 1f);
            Get<VisualEffect>().Intensity = control.Grounded ? k : 0f;
        }
        if (World.Has<AudioSource>(Entity))
        {
            var k = control.Grounded ? Math.Clamp(control.Speed / SparkFull, 0f, 1f) : 0f;
            ref var audio = ref Get<AudioSource>();
            audio.Gain = 0.9f * k;
            audio.Pitch = 0.7f + 0.9f * k;
        }
    }
}

namespace Glint;

/// <summary>
/// A soft dark disc kept on the deck under the ball: a cheap contact shadow. It follows the
/// ball's position, sits on the surface below it and shrinks as the ball rises. Presentation
/// only: nothing reads it back, and it has no collider.
/// </summary>
public sealed partial class BallShadow : Behaviour
{
    /// <summary>Diameter in metres when the ball rests on the deck.</summary>
    [Property, Default(1.5f)] public partial float Spread { get; set; }
    /// <summary>Height above the deck, in metres, at which the disc has shrunk to half size.</summary>
    [Property, Default(2.5f)] public partial float Falloff { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        var ball = World.Find("Ball");
        if (ball.IsNull) return;
        var position = World.Read<Transform>(ball).Position;
        ref var transform = ref Get<Transform>();
        if (!Course.FloorBelow(position, out var top))
        {
            transform.Scale = Vector3.Zero;
            return;
        }
        var height = MathF.Max(0f, position.Y - 0.5f - top);
        var size = Spread / (1f + height / Falloff);
        transform.Position = new Vector3(position.X, top + 0.02f, position.Z);
        transform.Scale = new Vector3(size, 1f, size);
    }
}

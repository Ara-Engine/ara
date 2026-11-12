namespace Glint;

/// <summary>
/// The ball's contact shadow: a soft alpha-blended disc laid on the surface under the ball,
/// tilted to the surface, growing and fading as the ball rises. It sits under the ray-traced
/// shadow so the ball never looks like it floats.
/// </summary>
public sealed partial class BallShadow : Behaviour
{
    /// <summary>Disc diameter when the ball touches the surface, metres.</summary>
    [Property, Default(1.3f)] public partial float Spread { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        var ball = World.Find("Ball");
        if (ball.IsNull) return;
        var position = World.Read<Transform>(ball).Position;
        ref var transform = ref Get<Transform>();
        if (!Course.FloorBelow(position, out var top, out var normal))
        {
            // Over the drop: park the disc far below so it cannot be seen.
            transform.Position = new Vector3(position.X, -400f, position.Z);
            return;
        }
        var height = Math.Max(0f, position.Y - 0.5f - top);
        var fade = Math.Clamp(1f - height / 6f, 0f, 1f);
        var size = Spread * (1f + height * 0.25f);
        transform.Position = new Vector3(position.X, top + 0.02f, position.Z);
        // Euler degrees (Rx * Ry * Rz) that turn +Y onto the surface normal.
        var roll = MathF.Asin(Math.Clamp(-normal.X, -1f, 1f)) * (180f / MathF.PI);
        var pitch = MathF.Atan2(normal.Z, normal.Y) * (180f / MathF.PI);
        transform.Rotation = new Vector3(pitch, 0f, roll);
        transform.Scale = new Vector3(size * fade, 1f, size * fade);
    }
}

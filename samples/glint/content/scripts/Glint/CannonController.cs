namespace Glint;

/// <summary>
/// The cannon: a ball that rolls into its intake is held and drawn in while it loads, then shot
/// from the muzzle along the ballistic arc the course solved to reach the goal pad. After a fall
/// or a restart it is ready again.
/// </summary>
public sealed partial class CannonController : Behaviour
{
    /// <summary>Ticks the ball is held before the shot.</summary>
    [Property, Default(80)] public partial int LoadTicks { get; set; }
    /// <summary>Planar reach of the intake in metres.</summary>
    [Property, Default(2.4f)] public partial float IntakeRadius { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Span<Entity> balls = stackalloc Entity[1];
        balls = World.Query<Ball>(balls);
        if (balls.Length != 1) return;
        var ball = balls[0];
        ref var control = ref World.Get<Ball>(ball);
        ref var cannon = ref Get<Cannon>();
        var position = World.Read<PhysicsPose>(ball).Position;
        if (control.Falls != cannon.SeenFalls)
        {
            // A respawn or a restart: ready again.
            cannon.SeenFalls = control.Falls;
            cannon.State = 0;
            control.Held = false;
        }
        switch (cannon.State)
        {
            case 0:
                if (control.Grounded && Course.Planar(position, cannon.Breech) < IntakeRadius * IntakeRadius
                    && MathF.Abs(position.Y - cannon.Breech.Y) < 2f)
                {
                    cannon.State = 1;
                    cannon.Loaded = 0;
                    cannon.From = position;
                    cannon.LoadTick = tick.Frame;
                    control.Held = true;
                }
                break;
            case 1:
                cannon.Loaded++;
                var t = MathF.Min(1f, cannon.Loaded / (float)Math.Max(1, LoadTicks));
                // Drawn into the breech (with a little shake), then held there.
                var inside = cannon.Breech + new Vector3(0, 0.6f, 0);
                var shake = MathF.Sin(cannon.Loaded * 1.7f) * 0.03f * t;
                Course.Teleport(ball, Vector3.Lerp(cannon.From, inside, t * t * (3f - 2f * t)) + new Vector3(shake, 0, shake));
                if (cannon.Loaded >= LoadTicks)
                {
                    cannon.State = 2;
                    cannon.FireTick = tick.Frame;
                    control.Held = false;
                    control.Flying = true;
                    control.LastGroundY = cannon.Muzzle.Y;
                    Course.Teleport(ball, cannon.Muzzle);
                    World.Get<Velocity>(ball).Linear = cannon.Launch;
                }
                break;
            case 2:
                // In flight until the ball is back on a surface.
                if (control.Grounded) cannon.State = 0;
                break;
        }
    }
}

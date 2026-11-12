namespace Glint;

/// <summary>
/// The run's director: ready, running, finished. It starts the clock on the first input,
/// records checkpoints, ends the run at the goal, keeps the best time and restarts on E.
/// Only this behaviour changes run state.
/// </summary>
public sealed partial class GlintRun : Behaviour
{
    /// <summary>0 ready, 1 running, 2 finished.</summary>
    [Property, Replicate] public partial int State { get; set; }
    /// <summary>Whether the run has ended at the goal; drives the results panel.</summary>
    [Property, Replicate] public partial bool Finished { get; set; }
    /// <summary>Fixed ticks since the first input.</summary>
    [Property, Replicate] public partial int Ticks { get; set; }
    /// <summary>Best finished time in ticks this session; zero until a run finishes.</summary>
    [Property, Replicate] public partial int BestTicks { get; set; }
    /// <summary>Checkpoints on the course.</summary>
    [Property, Default(2), Replicate] public partial int CheckpointCount { get; set; }
    /// <summary>Highest checkpoint reached.</summary>
    [Property, Replicate] public partial int Reached { get; set; }
    /// <summary>Falls this run.</summary>
    [Property, Replicate] public partial int Falls { get; set; }
    /// <summary>Where the ball starts and restarts.</summary>
    [Property] public partial Vector3 Start { get; set; }

    /// <inheritdoc/>
    protected override void OnTick(in Tick tick)
    {
        Span<Entity> balls = stackalloc Entity[1];
        balls = World.Query<Ball>(balls);
        if (balls.Length != 1) return;
        var ball = balls[0];
        ref var control = ref World.Get<Ball>(ball);
        var player = control.Player;
        var position = World.Read<Transform>(ball).Position;
        Falls = control.Falls;
        Reached = control.Checkpoint;

        if (State == 0)
        {
            if (Input.Axis2(player, "Move") != Vector2.Zero || Input.Button(player, "Jump")) State = 1;
            else return;
        }
        if (State == 2)
        {
            var restart = Input.Button(player, "Interact");
            if (restart && !control.RestartHeld) Restart(ball);
            control.RestartHeld = restart;
            return;
        }

        Ticks++;
        Span<Entity> pads = stackalloc Entity[8];
        foreach (var pad in World.Query<Checkpoint>(pads))
        {
            ref var checkpoint = ref World.Get<Checkpoint>(pad);
            var at = World.Read<Transform>(pad).Position;
            if (Course.Planar(position, at) > checkpoint.Radius * checkpoint.Radius || MathF.Abs(position.Y - at.Y) > 3f) continue;
            if (checkpoint.Index > control.Checkpoint)
            {
                control.Checkpoint = checkpoint.Index;
                control.Spawn = at + new Vector3(0, 1.2f, 0);
                checkpoint.Reached = true;
            }
        }
        Span<Entity> goals = stackalloc Entity[1];
        foreach (var goal in World.Query<Goal>(goals))
        {
            var at = World.Read<Transform>(goal).Position;
            var reach = World.Read<Goal>(goal).Radius;
            if (Course.Planar(position, at) < reach * reach && MathF.Abs(position.Y - at.Y) < 2.5f)
            {
                State = 2;
                Finished = true;
                BestTicks = BestTicks == 0 ? Ticks : Math.Min(BestTicks, Ticks);
            }
        }
    }

    /// <summary>Back to the start line: clock, checkpoints and ball.</summary>
    void Restart(Entity ball)
    {
        State = 0;
        Finished = false;
        Ticks = 0;
        ref var control = ref World.Get<Ball>(ball);
        control.Checkpoint = 0;
        control.Falls = 0;
        control.Spawn = Start;
        Span<Entity> pads = stackalloc Entity[8];
        foreach (var pad in World.Query<Checkpoint>(pads)) World.Get<Checkpoint>(pad).Reached = false;
        Course.Teleport(ball, Start);
    }
}

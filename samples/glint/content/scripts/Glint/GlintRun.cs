namespace Glint;

/// <summary>
/// The run's director: ready, running, finished. It starts the clock on the first input,
/// collects pickups, records checkpoints and splits, handles a fall (a fade and a respawn),
/// ends the run at the goal with a medal and restarts on E. Only this behaviour changes run state.
/// </summary>
public sealed partial class GlintRun : Behaviour
{
    /// <summary>0 ready, 1 running, 2 finished.</summary>
    [Property, Replicate] public partial int State { get; set; }
    /// <summary>Whether the run has ended at the goal; drives the results screen.</summary>
    [Property, Replicate] public partial bool Finished { get; set; }
    /// <summary>Fixed ticks since the first input.</summary>
    [Property, Replicate] public partial int Ticks { get; set; }
    /// <summary>Best finished time in ticks this session; zero until a run finishes.</summary>
    [Property, Replicate] public partial int BestTicks { get; set; }
    /// <summary>Orbs collected this run.</summary>
    [Property, Replicate] public partial int Orbs { get; set; }
    /// <summary>Orbs on the course.</summary>
    [Property, Replicate] public partial int OrbTotal { get; set; }
    /// <summary>Highest checkpoint reached.</summary>
    [Property, Replicate] public partial int Reached { get; set; }
    /// <summary>Falls this run.</summary>
    [Property, Replicate] public partial int Falls { get; set; }
    /// <summary>0 none, 1 bronze, 2 silver, 3 gold, for the finished run.</summary>
    [Property, Replicate] public partial int Medal { get; set; }
    /// <summary>Gold, silver and bronze times in ticks.</summary>
    [Property, Default(3600)] public partial int GoldTicks { get; set; }
    /// <summary>Silver time in ticks.</summary>
    [Property, Default(4500)] public partial int SilverTicks { get; set; }
    /// <summary>Bronze time in ticks.</summary>
    [Property, Default(6000)] public partial int BronzeTicks { get; set; }
    /// <summary>Tick of the last fall (a counter for the whoosh).</summary>
    [Property, Replicate] public partial ulong FallTick { get; set; }
    /// <summary>Tick the run finished (a counter for the fanfare).</summary>
    [Property, Replicate] public partial ulong FinishTick { get; set; }
    /// <summary>Ticks left of the respawn fade; the three flags below are its steps.</summary>
    [Property] public partial int FadeTicks { get; set; }
    /// <summary>Fully dark.</summary>
    [Property, Replicate] public partial bool FadeFull { get; set; }
    /// <summary>Half dark.</summary>
    [Property, Replicate] public partial bool FadeHalf { get; set; }
    /// <summary>A light veil.</summary>
    [Property, Replicate] public partial bool FadeLight { get; set; }
    /// <summary>The finished run's medal flags, for the results screen.</summary>
    [Property, Replicate] public partial bool IsGold { get; set; }
    /// <summary>Silver.</summary>
    [Property, Replicate] public partial bool IsSilver { get; set; }
    /// <summary>Bronze.</summary>
    [Property, Replicate] public partial bool IsBronze { get; set; }
    /// <summary>Split times at checkpoints 1 to 3, in ticks.</summary>
    [Property, Replicate] public partial int Split1 { get; set; }
    /// <summary>Split 2.</summary>
    [Property, Replicate] public partial int Split2 { get; set; }
    /// <summary>Split 3.</summary>
    [Property, Replicate] public partial int Split3 { get; set; }
    /// <summary>The clock as digits: minutes, seconds, hundredths (the UI binds each).</summary>
    [Property, Replicate] public partial int Min { get; set; }
    /// <summary>Seconds.</summary>
    [Property, Replicate] public partial int Sec { get; set; }
    /// <summary>Hundredths.</summary>
    [Property, Replicate] public partial int Cs { get; set; }
    /// <summary>Best time digits.</summary>
    [Property, Replicate] public partial int BestMin { get; set; }
    /// <summary>Best seconds.</summary>
    [Property, Replicate] public partial int BestSec { get; set; }
    /// <summary>Best hundredths.</summary>
    [Property, Replicate] public partial int BestCs { get; set; }
    /// <summary>Split digits (minutes, seconds, hundredths) for splits 1 to 3.</summary>
    [Property, Replicate] public partial int S1Min { get; set; }
    /// <summary>Split 1 seconds.</summary>
    [Property, Replicate] public partial int S1Sec { get; set; }
    /// <summary>Split 1 hundredths.</summary>
    [Property, Replicate] public partial int S1Cs { get; set; }
    /// <summary>Split 2 minutes.</summary>
    [Property, Replicate] public partial int S2Min { get; set; }
    /// <summary>Split 2 seconds.</summary>
    [Property, Replicate] public partial int S2Sec { get; set; }
    /// <summary>Split 2 hundredths.</summary>
    [Property, Replicate] public partial int S2Cs { get; set; }
    /// <summary>Split 3 minutes.</summary>
    [Property, Replicate] public partial int S3Min { get; set; }
    /// <summary>Split 3 seconds.</summary>
    [Property, Replicate] public partial int S3Sec { get; set; }
    /// <summary>Split 3 hundredths.</summary>
    [Property, Replicate] public partial int S3Cs { get; set; }
    /// <summary>Waiting for the first input: shows the key prompts.</summary>
    [Property, Replicate] public partial bool IsReady { get; set; }
    /// <summary>A finished run has set a best time.</summary>
    [Property, Replicate] public partial bool HasBest { get; set; }
    /// <summary>Where the ball starts and restarts.</summary>
    [Property] public partial Vector3 Start { get; set; }

    static void Parts(int ticks, out int minutes, out int seconds, out int hundredths)
    {
        minutes = ticks / 3600;
        seconds = ticks / 60 % 60;
        hundredths = ticks % 60 * 100 / 60;
    }

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

        // Fall: the controller already put the ball back; this fades the screen and counts.
        if (control.Falls != Falls)
        {
            if (control.Falls > Falls) { FadeTicks = 40; FallTick = tick.Frame; }
            Falls = control.Falls;
        }
        if (FadeTicks > 0) FadeTicks--;
        FadeFull = FadeTicks > 26;
        FadeHalf = FadeTicks is > 13 and <= 26;
        FadeLight = FadeTicks is > 0 and <= 13;
        Reached = control.Checkpoint;

        IsReady = State == 0;
        HasBest = BestTicks > 0;
        var interact = Input.Button(player, "Interact");
        var pressed = interact && !control.RestartHeld;
        control.RestartHeld = interact;
        if (State == 0)
        {
            if (Input.Axis2(player, "Move") != Vector2.Zero || Input.Button(player, "Jump")) State = 1;
            else { Digits(); return; }
        }
        if (State == 2)
        {
            if (pressed) Restart(ball);
            return;
        }
        if (pressed)
        {
            // E mid-run: back to the last checkpoint.
            control.Held = false;
            control.Flying = false;
            Course.Teleport(ball, control.Spawn);
            control.Falls++;
            Falls = control.Falls;
            FadeTicks = 24;
            return;
        }

        Ticks++;
        Span<Entity> pads = stackalloc Entity[8];
        foreach (var pad in World.Query<Checkpoint>(pads))
        {
            ref var checkpoint = ref World.Get<Checkpoint>(pad);
            var at = World.Read<Transform>(pad).Position;
            if (checkpoint.Reached || Vector3.Distance(position, at + new Vector3(0, 1.0f, 0)) > checkpoint.Radius) continue;
            if (checkpoint.Index == control.Checkpoint + 1)
            {
                checkpoint.Reached = true;
                checkpoint.ReachedTick = tick.Frame;
                control.Checkpoint = checkpoint.Index;
                control.Spawn = at + new Vector3(0, 1.4f, 0);
                if (checkpoint.Index == 1) Split1 = Ticks;
                else if (checkpoint.Index == 2) Split2 = Ticks;
                else if (checkpoint.Index == 3) Split3 = Ticks;
            }
        }
        Span<Entity> orbs = stackalloc Entity[96];
        foreach (var orb in World.Query<Pickup>(orbs))
        {
            ref var pickup = ref World.Get<Pickup>(orb);
            if (pickup.Taken) continue;
            if (Vector3.Distance(position, World.Read<Transform>(orb).Position) > pickup.Radius) continue;
            pickup.Taken = true;
            pickup.TakenTick = tick.Frame;
            World.Get<Transform>(orb).Scale = Vector3.Zero;
            Orbs++;
        }
        Span<Entity> goals = stackalloc Entity[1];
        foreach (var goal in World.Query<Goal>(goals))
        {
            var at = World.Read<Transform>(goal).Position;
            var reach = World.Read<Goal>(goal).Radius;
            if (control.Grounded && Vector3.Distance(position, at + new Vector3(0, 0.5f, 0)) < reach + 4f)
            {
                State = 2;
                Finished = true;
                FinishTick = tick.Frame;
                BestTicks = BestTicks == 0 ? Ticks : Math.Min(BestTicks, Ticks);
                Medal = Ticks <= GoldTicks ? 3 : Ticks <= SilverTicks ? 2 : Ticks <= BronzeTicks ? 1 : 0;
                IsGold = Medal == 3;
                IsSilver = Medal == 2;
                IsBronze = Medal == 1;
            }
        }
        Digits();
    }

    void Digits()
    {
        Parts(Ticks, out var m, out var s, out var c);
        Min = m; Sec = s; Cs = c;
        Parts(BestTicks, out m, out s, out c);
        BestMin = m; BestSec = s; BestCs = c;
        Parts(Split1, out m, out s, out c);
        S1Min = m; S1Sec = s; S1Cs = c;
        Parts(Split2, out m, out s, out c);
        S2Min = m; S2Sec = s; S2Cs = c;
        Parts(Split3, out m, out s, out c);
        S3Min = m; S3Sec = s; S3Cs = c;
    }

    /// <summary>Back to the start line: clock, checkpoints, pickups, cannon and ball.</summary>
    void Restart(Entity ball)
    {
        State = 0;
        Finished = false;
        IsGold = IsSilver = IsBronze = false;
        Medal = 0;
        Ticks = 0;
        Orbs = 0;
        Split1 = Split2 = Split3 = 0;
        ref var control = ref World.Get<Ball>(ball);
        control.Checkpoint = 0;
        control.Falls = 0;
        control.Held = false;
        control.Flying = false;
        control.Spawn = Start;
        Falls = 0;
        Span<Entity> pads = stackalloc Entity[8];
        foreach (var pad in World.Query<Checkpoint>(pads)) World.Get<Checkpoint>(pad).Reached = false;
        Span<Entity> orbs = stackalloc Entity[96];
        foreach (var orb in World.Query<Pickup>(orbs))
        {
            ref var pickup = ref World.Get<Pickup>(orb);
            pickup.Taken = false;
            World.Get<Transform>(orb).Scale = new Vector3(pickup.Size, pickup.Size, pickup.Size);
        }
        Span<Entity> cannons = stackalloc Entity[1];
        foreach (var cannon in World.Query<Cannon>(cannons))
        {
            ref var c = ref World.Get<Cannon>(cannon);
            c.State = 0;
            c.SeenFalls = 0;
        }
        Course.Teleport(ball, Start);
        FadeTicks = 24;
        Digits();
    }
}

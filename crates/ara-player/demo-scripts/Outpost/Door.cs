namespace Outpost;

/// <summary>A door that swings open around its Y axis when <see cref="IsOpen"/> is set.</summary>
public sealed partial class Door : Behaviour
{
    /// <summary>How far the door opens.</summary>
    [Property, Unit(Units.Degrees), Range(0, 180)] public partial float OpenAngle { get; set; }

    /// <summary>Whether the door should be open.</summary>
    [Property, Replicate] public partial bool IsOpen { get; set; }

    /// <summary>Swing speed, read from tuning once (a presentation cache, not state).</summary>
    [Transient] float _degreesPerSecond;

    /// <inheritdoc/>
    protected override void OnSpawn() => _degreesPerSecond = 90f;

    /// <inheritdoc/>
    protected override void OnReload() => _degreesPerSecond = 90f;

    /// <inheritdoc/>
    protected override void OnTick(in Tick t)
    {
        ref var tr = ref Get<Transform>();
        var target = IsOpen ? OpenAngle : 0f;
        var step = _degreesPerSecond * t.Dt;
        tr.Rotation.Y = MathF.Abs(target - tr.Rotation.Y) <= step ? target : tr.Rotation.Y + MathF.Sign(target - tr.Rotation.Y) * step;
    }
}

namespace Ara;

/// <summary>
/// Declares a blittable component stored in engine ECS columns. Apply to a
/// <c>partial struct</c>; the source generator registers it in the Schema DB (name,
/// fields, units, ranges, docs), so it appears in <c>reflection.json</c> and agent tools.
/// </summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class ComponentAttribute(string? name = null) : Attribute
{
    /// <summary>Schema type name (PascalCase); defaults to the struct name.</summary>
    public string? Name { get; } = name;
}

/// <summary>Marks a component generated from the Rust Schema DB (engine-owned; do not apply by hand).</summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class EngineComponentAttribute(string name) : Attribute
{
    /// <summary>Schema type name.</summary>
    public string Name { get; } = name;
    /// <summary>Owning engine module (<c>core</c>).</summary>
    public string Module { get; init; } = "";
}

/// <summary>
/// Declares a world singleton (game rules, tuning). Systems take it as an <c>in</c> or
/// <c>ref</c> parameter; behaviours and commands use <see cref="World.Resource{T}"/>.
/// </summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class ResourceAttribute(string? name = null) : Attribute
{
    /// <summary>Schema type name; defaults to the struct name.</summary>
    public string? Name { get; } = name;
}

/// <summary>
/// Declares a presentation cue (sound, VFX trigger) that commands and systems emit with
/// <c>Cue(...)</c>. Cues are reported to the host and never change simulation state.
/// </summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class CueAttribute(string? name = null) : Attribute
{
    /// <summary>Schema type name; defaults to the struct name.</summary>
    public string? Name { get; } = name;
}

/// <summary>
/// Turns a <c>static void Run(ref A a, in B b, in Tick t)</c> method into a chunk-batched
/// system. <c>ref</c> component parameters are writes, <c>in</c> parameters reads; the
/// read/write sets go to the scheduler, so non-conflicting systems run in parallel.
/// Optional parameters: <c>Entity</c>, <c>in Tick</c>, <c>ref Rng</c>,
/// <c>ref Commands</c>, and <see cref="ResourceAttribute"/> structs.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class QueryAttribute : Attribute
{
    /// <summary>Ordering among systems whose accesses conflict (lower runs first; ties by name).</summary>
    public int Order { get; init; }
    /// <summary>System name; defaults to the type name, or <c>Type.Method</c> when the method is not <c>Run</c>.</summary>
    public string? Name { get; init; }
}

/// <summary>Restricts a <see cref="QueryAttribute"/> system to entities that also have <typeparamref name="T"/>.</summary>
[AttributeUsage(AttributeTargets.Method, AllowMultiple = true)]
public sealed class WithAttribute<T> : Attribute where T : unmanaged;

/// <summary>Restricts a <see cref="QueryAttribute"/> system to entities without <typeparamref name="T"/>.</summary>
[AttributeUsage(AttributeTargets.Method, AllowMultiple = true)]
public sealed class WithoutAttribute<T> : Attribute where T : unmanaged;

/// <summary>
/// Declares a gameplay command: a <c>static partial class</c> with
/// <c>public static CommandResult Execute(ref CommandContext ctx, Entity target, ref C component, ...)</c>.
/// Plain parameters become the params type agents and tools call it with; <c>ref</c>/<c>in</c>
/// component parameters bind to the target entity's components. Commands are
/// synchronous and capture no managed state, so they are prediction- and undo-safe.
/// </summary>
[AttributeUsage(AttributeTargets.Class)]
public sealed class CommandAttribute(string? name = null) : Attribute
{
    /// <summary>Stable command name (<c>game.UseMedkit</c>); defaults to the class name.</summary>
    public string? Name { get; } = name;
}

/// <summary>Where a networked command runs.</summary>
public enum ExecuteOn
{
    /// <summary>On the authority (server or host), after validation.</summary>
    Authority,
    /// <summary>On the owning client, predicted, then confirmed by the authority.</summary>
    Predicted,
}

/// <summary>Who may send a networked command.</summary>
public enum Caller
{
    /// <summary>Any connected client.</summary>
    Any,
    /// <summary>Only the owner of the target entity.</summary>
    Owner,
    /// <summary>Only the authority itself (tools, scripts, agents).</summary>
    Authority,
}

/// <summary>Network exposure of a <see cref="CommandAttribute"/> command (enforced by the netcode layer).</summary>
[AttributeUsage(AttributeTargets.Class)]
public sealed class ExposeAttribute(ExecuteOn executeOn, Caller caller) : Attribute
{
    /// <summary>Where it runs.</summary>
    public ExecuteOn ExecuteOn { get; } = executeOn;
    /// <summary>Who may send it.</summary>
    public Caller Caller { get; } = caller;
    /// <summary>Per-caller rate limit; 0 means unlimited.</summary>
    public double RatePerSecond { get; init; }
}

/// <summary>An example params object (JSON) for a command, shown to agents and in docs.</summary>
[AttributeUsage(AttributeTargets.Class, AllowMultiple = true)]
public sealed class ExampleAttribute(string json) : Attribute
{
    /// <summary>The params as JSON.</summary>
    public string Json { get; } = json;
}

/// <summary>
/// Backs a Behaviour's <c>partial</c> property with an ECS column: the value lives in the
/// behaviour's generated data component, so it is saved, replicated and survives reload.
/// </summary>
[AttributeUsage(AttributeTargets.Property)]
public sealed class PropertyAttribute : Attribute;

/// <summary>
/// A presentation-only cache on a Behaviour: not saved, not replicated, dropped on
/// reload. Every other Behaviour instance field is an error (ARA0101).
/// </summary>
[AttributeUsage(AttributeTargets.Field)]
public sealed class TransientAttribute : Attribute;

/// <summary>
/// Marks simulation code (a type or the whole assembly): it must be deterministic, so
/// ARA0200 rejects async, wall-clock time, <c>System.Random</c>, I/O and threads in it.
/// </summary>
[AttributeUsage(AttributeTargets.Class | AttributeTargets.Struct | AttributeTargets.Assembly)]
public sealed class SimulationAttribute : Attribute;

/// <summary>Physical unit of a numeric field, by schema symbol (see <see cref="Units"/>).</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property | AttributeTargets.Parameter)]
public sealed class UnitAttribute(string symbol) : Attribute
{
    /// <summary>The symbol (<c>m</c>, <c>deg</c>, <c>m/s</c>).</summary>
    public string Symbol { get; } = symbol;
}

/// <summary>Inclusive numeric bounds (Unity's <c>[Range]</c>, same meaning).</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property | AttributeTargets.Parameter)]
public sealed class RangeAttribute(double min, double max) : Attribute
{
    /// <summary>Smallest allowed value.</summary>
    public double Min { get; } = min;
    /// <summary>Largest allowed value.</summary>
    public double Max { get; } = max;
}

/// <summary>An inclusive lower bound.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property | AttributeTargets.Parameter)]
public sealed class MinAttribute(double min) : Attribute
{
    /// <summary>Smallest allowed value.</summary>
    public double Min { get; } = min;
}

/// <summary>An inclusive upper bound.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property | AttributeTargets.Parameter)]
public sealed class MaxAttribute(double max) : Attribute
{
    /// <summary>Largest allowed value.</summary>
    public double Max { get; } = max;
}

/// <summary>The schema default of a field: a number, bool or string, or a vector's components.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class DefaultAttribute(params object[] values) : Attribute
{
    /// <summary>The value, or a vector's components.</summary>
    public object[] Values { get; } = values;
}

/// <summary>Recipients of a replicated field.</summary>
public enum Audience
{
    /// <summary>Not replicated.</summary>
    None,
    /// <summary>Every client the entity is relevant to.</summary>
    Relevant,
    /// <summary>Only the owning client.</summary>
    Owner,
}

/// <summary>How remote views smooth a replicated field between snapshots.</summary>
public enum Interpolate
{
    /// <summary>Snap to each snapshot.</summary>
    None,
    /// <summary>Linear interpolation.</summary>
    Linear,
    /// <summary>Spherical interpolation, for rotations.</summary>
    Slerp,
}

/// <summary>Network replication of a field (codecs and interpolation stay in the engine).</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class ReplicateAttribute(Audience audience = Audience.Relevant) : Attribute
{
    /// <summary>Who receives it.</summary>
    public Audience Audience { get; } = audience;
    /// <summary>Wire precision in the field's unit; 0 sends full precision.</summary>
    public double Quantize { get; init; }
    /// <summary>Smoothing on remote views.</summary>
    public Interpolate Interpolate { get; init; }
    /// <summary>Part of the rollback (prediction) state.</summary>
    public bool Rollback { get; init; }
}

/// <summary>A former field name; files using it are renamed on read (Unity's attribute, same meaning).</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property, AllowMultiple = true)]
public sealed class FormerlySerializedAsAttribute(string oldName) : Attribute
{
    /// <summary>The old name, as written in C#.</summary>
    public string OldName { get; } = oldName;
}

/// <summary>Inspector tooltip text; used as the field doc when there is no XML doc comment.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class TooltipAttribute(string text) : Attribute
{
    /// <summary>The tooltip.</summary>
    public string Text { get; } = text;
}

/// <summary>Inspector group heading for this field and the ones after it.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class HeaderAttribute(string text) : Attribute
{
    /// <summary>The heading.</summary>
    public string Text { get; } = text;
}

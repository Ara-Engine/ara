using System.Text;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// The outcome of a command: <see cref="Ok"/>, or a rejection made with
/// <see cref="CommandContext.Reject"/>. A rejected command leaves the world unchanged.
/// </summary>
public readonly struct CommandResult
{
    internal readonly string? Reason;

    internal CommandResult(string reason) => Reason = reason;

    /// <summary>Success: everything the command wrote is kept as one undoable step.</summary>
    public static CommandResult Ok => default;

    /// <summary>True unless the command was rejected.</summary>
    public bool IsOk => Reason is null;
}

/// <summary>
/// What a <see cref="CommandAttribute"/> command sees: the tick, the caller, a
/// deterministic <see cref="Rng"/>, structural <see cref="Commands"/> and cues. A
/// <c>ref struct</c>, so commands cannot capture it in lambdas or keep it past the call.
/// </summary>
public unsafe ref struct CommandContext
{
    readonly CommandCall* _call;

    /// <summary>Deterministic random numbers for this command call.</summary>
    public Rng Rng;

    /// <summary>Created by generated command entry points.</summary>
    public CommandContext(CommandCall* call)
    {
        _call = call;
        Rng = new Rng(call->Tick->Seed ^ call->Tick->Frame, call->Sequence);
    }

    /// <summary>The current tick.</summary>
    public readonly ref readonly Tick Tick => ref *_call->Tick;

    /// <summary>The calling entity, or <see cref="Entity.Null"/> for tools and agents.</summary>
    public readonly Entity Caller => new(_call->Caller);

    /// <summary>Structural changes, applied only if the command succeeds.</summary>
    public readonly Commands Commands => new(_call->Commands);

    /// <summary>Rejects the call with a short machine-readable reason (<c>cannot_heal</c>).</summary>
    public readonly CommandResult Reject(string reason) => new(reason);

    /// <summary>Emits a presentation cue (kept only if the command succeeds).</summary>
    public readonly void Cue<T>(in T cue) where T : unmanaged, ICue<T> => Commands.Cue(cue);

    /// <summary>
    /// Used by generated entry points: writes a rejection reason into the call and maps
    /// the result to an ABI status.
    /// </summary>
    public readonly int Finish(CommandResult result)
    {
        if (result.IsOk) return AbiConst.RunOk;
        // Reasons are short ASCII codes; longer ones are cut at 64 bytes (the host decodes lossily).
        var bytes = Encoding.UTF8.GetBytes(result.Reason!);
        var n = Math.Min(bytes.Length, 64);
        bytes.AsSpan(0, n).CopyTo(new Span<byte>(_call->Reason, 64));
        _call->ReasonLen = (uint)n;
        return AbiConst.RunRejected;
    }

    /// <summary>Used by generated entry points when the target lacks a component parameter.</summary>
    public readonly int MissingComponent(string component) => Finish(Reject("missing_" + component));
}

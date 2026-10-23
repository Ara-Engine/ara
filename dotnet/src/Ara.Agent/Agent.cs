using System.Text.Json;
using System.Text.Json.Nodes;

namespace Ara.Agent;

/// <summary>The reply to one engine call.</summary>
/// <param name="Ok">True if the call succeeded.</param>
/// <param name="Output">The command or query output.</param>
/// <param name="Code">The diagnostic code on failure (<c>ARA1601</c>).</param>
/// <param name="Message">The diagnostic message on failure.</param>
public sealed record CommandReply(bool Ok, JsonNode? Output, string? Code = null, string? Message = null);

/// <summary>
/// Where a script's engine calls go. It is the only path from a script to the engine:
/// every write is a typed command, recorded as ops in one reviewable transaction. The
/// worker's sink forwards to its supervisor; an Engine Protocol client can stand in.
/// </summary>
public interface ICommandSink
{
    /// <summary>Calls a registered command or query by name with JSON params.</summary>
    Task<CommandReply> CallAsync(string command, JsonNode? parameters, CancellationToken cancel);
}

/// <summary>Thrown when an engine call fails; uncaught, it fails the script with the call's diagnostic.</summary>
public sealed class AgentCommandException(string command, string? code, string? message)
    : Exception($"`{command}` failed: {code} {message}")
{
    /// <summary>The command that failed.</summary>
    public string Command { get; } = command;
    /// <summary>Its diagnostic code.</summary>
    public string? Code { get; } = code;
}

/// <summary>
/// What a script sees as <c>ara</c>: engine calls, a log and the transaction's
/// <see cref="Why"/>. Scripts are bodies of an <c>async</c> method; their <c>return</c>
/// value is serialized to JSON for the caller.
/// </summary>
/// <example>
/// <code>
/// var crate = await ara.Call("entity.create", new { name = "Crate" });
/// ara.Why = "add a crate for the door test";
/// return new { id = crate?["id"] };
/// </code>
/// </example>
public sealed class ScriptContext(ICommandSink sink, CancellationToken cancel)
{
    readonly List<string> _log = [];

    /// <summary>Why these changes are made; shown when the transaction is reviewed.</summary>
    public string? Why { get; set; }

    /// <summary>Lines logged by the script, in order.</summary>
    public IReadOnlyList<string> Lines => _log;

    /// <summary>Number of engine calls made so far.</summary>
    public int Calls { get; private set; }

    /// <summary>Calls a command or query. <paramref name="parameters"/> is an object serialized to JSON (anonymous types work).</summary>
    /// <exception cref="AgentCommandException">The call failed.</exception>
    public async Task<JsonNode?> Call(string command, object? parameters = null)
    {
        var node = parameters switch
        {
            null => new JsonObject(),
            JsonNode n => n,
            _ => JsonSerializer.SerializeToNode(parameters, parameters.GetType(), AgentJson.Options),
        };
        Calls++;
        var reply = await sink.CallAsync(command, node, cancel).ConfigureAwait(false);
        if (!reply.Ok) throw new AgentCommandException(command, reply.Code, reply.Message);
        return reply.Output;
    }

    /// <summary>Adds a line to the script's log.</summary>
    public void Log(string message) => _log.Add(message);
}

/// <summary>JSON settings shared by the facade and the worker.</summary>
public static class AgentJson
{
    /// <summary>camelCase names, so <c>new { MaxHp = 3 }</c> becomes <c>{ "maxHp": 3 }</c>.</summary>
    public static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web);
}

using System.Text.Json;
using System.Text.Json.Serialization;
using Ara.Hosting;

namespace Ara;

/// <summary>
/// Per-user graphics options for game menus, available in exclusive Behaviour/UI callbacks.
/// These preferences are not gameplay state and must never drive simulation decisions.
/// Apply/Revert accept a configuration for the next presentation boundary; Save is explicit.
/// </summary>
public static class GraphicsSettings
{
    /// <summary>Get or replace the pending candidate without applying or saving it.</summary>
    public static GraphicsOptions Pending
    {
        get => Query().Pending;
        set { ArgumentNullException.ThrowIfNull(value); Call(new("set", value)); }
    }
    /// <summary>Last accepted preferences, including preferences requiring restart.</summary>
    public static GraphicsOptions Current => Query().Applied;
    /// <summary>Authored project defaults.</summary>
    public static GraphicsOptions Defaults => Query().Defaults;
    /// <summary>Current GPU and surface capabilities and supported display modes.</summary>
    public static GraphicsCapabilities Capabilities => Query().Capabilities;
    /// <summary>Raised after an accepted Apply/Revert, including changes made over the protocol.
    /// Subscriptions belong to the active module and are removed on L3 reload/shutdown;
    /// register again in OnReload as well as OnSpawn.</summary>
    public static event Action<GraphicsState> Applied
    {
        add { ArgumentNullException.ThrowIfNull(value); var context = Host.GraphicsContext; if (context.GraphicsRevision == 0) context.GraphicsRevision = Host.GraphicsRevision(); context.GraphicsHandlers.Add((context.GraphicsModule, value)); }
        remove
        {
            var handlers = Host.GraphicsContext.GraphicsHandlers;
            for (int index = handlers.Count - 1; index >= 0; index--)
                if (handlers[index].Handler == value) { handlers.RemoveAt(index); break; }
        }
    }
    /// <summary>Read the complete immutable service state.</summary>
    public static GraphicsState Query() => Call(new("get"));
    /// <summary>Validate/clamp and accept Pending. Throws on unavailable capabilities. Does not save.</summary>
    public static GraphicsState Apply() => Call(new("apply"));
    /// <summary>Apply the previous accepted configuration, useful for an options-menu Cancel action.</summary>
    public static GraphicsState Revert() => Call(new("revert"));
    /// <summary>Stage the project's defaults. Apply and Save remain explicit.</summary>
    public static GraphicsState ResetToDefaults() => Call(new("resetToDefaults"));
    /// <summary>Atomically save accepted preferences to this game's user config directory.</summary>
    public static GraphicsState Save() => Call(new("save"));
    /// <summary>Stage device-based Auto and clear group/scale/upscaler overrides. Never enables RT.</summary>
    public static GraphicsState AutoDetect() => Call(new("autoDetect"));

    internal static void Poll()
    {
        var context = Host.GraphicsContext;
        if (context.GraphicsHandlers.Count != 0 && Host.GraphicsRevision() != context.GraphicsRevision) Query();
    }

    internal static GraphicsState Call(GraphicsRequest request)
    {
        var response = JsonSerializer.Deserialize(Host.Graphics(JsonSerializer.SerializeToUtf8Bytes(request, GraphicsJson.Default.GraphicsRequest)), GraphicsJson.Default.GraphicsResponse)
            ?? throw new InvalidOperationException("Empty graphics settings response");
        if (response.Error is { } error) throw new InvalidOperationException(error);
        var state = response.State ?? throw new InvalidOperationException("Missing graphics settings state");
        var context = Host.GraphicsContext;
        if (context.GraphicsRevision != state.Revision)
        {
            var notify = context.GraphicsRevision != 0 || request.Operation is "apply" or "revert";
            context.GraphicsRevision = state.Revision;
            if (notify) foreach (var (module, handler) in context.GraphicsHandlers.ToArray())
            {
                var previousModule = context.GraphicsModule;
                context.GraphicsModule = module;
                try { handler(state); }
                catch (Exception exception) { Host.Log(2, $"GraphicsSettings.Applied: {exception.Message}"); }
                finally { context.GraphicsModule = previousModule; }
            }
        }
        return state;
    }
}

internal sealed record GraphicsRequest(string Operation, GraphicsOptions? Settings = null);
internal sealed record GraphicsResponse(GraphicsState? State, string? Error);
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase,
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull)]
[JsonSerializable(typeof(GraphicsRequest))]
[JsonSerializable(typeof(GraphicsResponse))]
internal sealed partial class GraphicsJson : JsonSerializerContext;

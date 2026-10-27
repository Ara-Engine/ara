using System.Text.Json.Nodes;
using Ara.Agent;

namespace Ara.Examples;

/// <summary>Agent authoring uses the same typed commands as the editor and CLI.</summary>
public static class CommandExamples
{
    sealed class TraceSink(JsonArray calls) : ICommandSink
    {
        public Task<CommandReply> CallAsync(string command, JsonNode? parameters, CancellationToken cancel)
        {
            calls.Add(new JsonObject { ["command"] = command, ["parameters"] = parameters });
            return Task.FromResult(new CommandReply(true, null));
        }
    }

    /// <summary>Runs the compiled recipes; Rust tests validate these calls against the command registry.</summary>
    public static async Task<JsonArray> TraceAsync()
    {
        var calls = new JsonArray();
        var context = new ScriptContext(new TraceSink(calls), CancellationToken.None);
        await Create(context);
        await AddLight(context, "e_actor");
        await SetIntensity(context, "e_actor");
        await RemoveLight(context, "e_actor");
        await Delete(context, "e_actor");
        return calls;
    }

    #region commands/entity.create
    /// <summary>Create an actor using a transaction-backed command.</summary>
    public static Task<JsonNode?> Create(ScriptContext ara) =>
        ara.Call("entity.create", new JsonObject { ["name"] = "Salvage crate" });
    #endregion

    #region commands/component.add
    /// <summary>Attach a light with physical intensity.</summary>
    public static Task<JsonNode?> AddLight(ScriptContext ara, string entity) =>
        ara.Call("component.add", new JsonObject { ["entity"] = entity, ["component"] = "Light", ["value"] = new JsonObject { ["intensity"] = 500 } });
    #endregion

    #region commands/property.set
    /// <summary>Change a reflected field through the command registry.</summary>
    public static Task<JsonNode?> SetIntensity(ScriptContext ara, string entity) =>
        ara.Call("property.set", new JsonObject { ["entity"] = entity, ["path"] = "Light.intensity", ["value"] = 750 });
    #endregion

    #region commands/component.remove
    /// <summary>Remove a component through an undoable command.</summary>
    public static Task<JsonNode?> RemoveLight(ScriptContext ara, string entity) =>
        ara.Call("component.remove", new JsonObject { ["entity"] = entity, ["component"] = "Light" });
    #endregion

    #region commands/entity.delete
    /// <summary>Delete an actor through the normal transaction path.</summary>
    public static Task<JsonNode?> Delete(ScriptContext ara, string entity) =>
        ara.Call("entity.delete", new JsonObject { ["entities"] = new JsonArray(entity) });
    #endregion
}

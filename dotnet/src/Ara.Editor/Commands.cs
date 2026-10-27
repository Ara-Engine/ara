using System.Text.Json;
using System.Text.Json.Serialization;

namespace Ara.Editor;

/// <summary>One typed engine command, committed with the invocation's ordinary transaction.</summary>
/// <param name="Cmd">Registered engine command name.</param>
/// <param name="Args">Immutable command arguments validated by the authority's schema.</param>
/// <param name="Symbol">Optional batch symbol for a newly created entity.</param>
public sealed record AuthoringCall(string Cmd, JsonElement Args, [property: JsonPropertyName("as")] string? Symbol = null);

/// <summary>Typed authoring operations. These queue commands; only the authority can produce ops.</summary>
public sealed class AuthoringCommands
{
    readonly List<AuthoringCall> _calls = [];
    int _symbol;
    /// <summary>The invocation's ordered command batch.</summary>
    public IReadOnlyList<AuthoringCall> Calls => _calls.AsReadOnly();
    /// <summary>Create an authored entity and return its batch-local address.</summary>
    public EntityId Create(string name, EntityId? parent = null, IReadOnlyDictionary<string, JsonElement>? components = null)
    {
        var symbol = "extension" + _symbol++;
        _calls.Add(new("entity.create", EditorJson.Arguments(new { name, parent=parent?.Value, components }), symbol));
        return new("$" + symbol);
    }
    /// <summary>Delete entities and descendants through the reversible entity command.</summary>
    public void Delete(params EntityId[] entities) => _calls.Add(new("entity.delete", EditorJson.Arguments(new { entities=entities.Select(e => e.Value) })));
    /// <summary>Reparent an authored entity; the normal entity command defines its transform semantics.</summary>
    public void Reparent(EntityId entity, EntityId? parent, uint? index = null) => _calls.Add(new("entity.reparent", EditorJson.Arguments(new { entity=entity.Value, parent=parent?.Value, index })));
    /// <summary>Set a schema-validated entity or component field.</summary>
    public void SetProperty<T>(EntityId entity, string path, T value) => _calls.Add(new("property.set", EditorJson.Arguments(new { entity=entity.Value, path, value })));
    /// <summary>Add a reflected component, optionally with field values.</summary>
    public void AddComponent<T>(EntityId entity, string component, T value) => _calls.Add(new("component.add", EditorJson.Arguments(new { entity=entity.Value, component, value })));
    /// <summary>Remove a reflected component through its typed command.</summary>
    public void RemoveComponent(EntityId entity, string component) => _calls.Add(new("component.remove", EditorJson.Arguments(new { entity=entity.Value, component })));
}

/// <summary>Selection at invocation time and an optional requested navigation change.</summary>
public sealed class EditorSelection
{
    EntityId[]? _requested;
    /// <summary>Creates an immutable snapshot of selected entity addresses.</summary>
    public EditorSelection(IEnumerable<EntityId> entities) => Entities = Array.AsReadOnly(entities.ToArray());
    /// <summary>Selected addresses, primary first.</summary>
    public IReadOnlyList<EntityId> Entities { get; }
    /// <summary>Requested selection after a successful authority transaction, or null if unchanged.</summary>
    public IReadOnlyList<EntityId>? Requested => _requested is null ? null : Array.AsReadOnly(_requested);
    /// <summary>Request selection through the editor's ordinary reversible navigation action.</summary>
    public void Set(IEnumerable<EntityId> entities) => _requested = entities.Distinct().ToArray();
}

/// <summary>One command invocation: immutable queries, selection and typed authoring commands.</summary>
public sealed class EditorContext(EditorScene scene, EditorSelection selection)
{
    /// <summary>Scene snapshot captured at the authority revision.</summary>
    public EditorScene Scene { get; } = scene;
    /// <summary>Selection query and deferred navigation request.</summary>
    public EditorSelection Selection { get; } = selection;
    /// <summary>Typed commands applied atomically by the authority under the initiating session.</summary>
    public AuthoringCommands Commands { get; } = new();
}

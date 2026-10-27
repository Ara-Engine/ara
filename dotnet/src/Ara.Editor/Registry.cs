namespace Ara.Editor;

/// <summary>Implemented by editor-only extensions to contribute to the one editor command registry.</summary>
public interface IEditorExtension
{
    /// <summary>Register commands and menu placements; called on each compiled editor generation.</summary>
    void Register(EditorRegistry registry);
}

/// <summary>Metadata for a command in the native editor's shared registry.</summary>
/// <param name="Id">Stable command id.</param>
/// <param name="Title">Human title for menus and the palette.</param>
/// <param name="Summary">Command purpose for tool search.</param>
/// <param name="Enabled">False when a callback fault disabled the command until reload.</param>
/// <param name="DisabledReason">The callback fault shown in menus, the palette and Problems.</param>
public sealed record EditorCommand(string Id, string Title, string Summary, bool Enabled = true, string? DisabledReason = null);

/// <summary>A menu placement that refers to a registered command.</summary>
/// <param name="Path">Slash-separated menu path.</param>
/// <param name="Command">Existing registry command id.</param>
public sealed record EditorMenu(string Path, string Command);

/// <summary>Contributes command definitions and menu placements to the native registry.</summary>
public sealed class EditorRegistry
{
    readonly Dictionary<string, (EditorCommand Metadata, Action<EditorContext> Run)> _commands = new(StringComparer.Ordinal);
    readonly List<EditorMenu> _menus = [];
    readonly HashSet<string> _reserved;
    readonly Dictionary<string, string> _disabled = new(StringComparer.Ordinal);
    /// <summary>Create a registration scope with the native registry's reserved ids.</summary>
    public EditorRegistry(IEnumerable<string>? reserved = null) => _reserved = new(reserved ?? [], StringComparer.Ordinal);
    /// <summary>Registered commands, in stable id order.</summary>
    public IReadOnlyList<EditorCommand> Commands => Array.AsReadOnly(_commands.Values.Select(c => _disabled.TryGetValue(c.Metadata.Id, out var why)
        ? c.Metadata with { Enabled = false, DisabledReason = why } : c.Metadata).OrderBy(c => c.Id, StringComparer.Ordinal).ToArray());
    /// <summary>Menu placements, ordered by path then command id.</summary>
    public IReadOnlyList<EditorMenu> Menus => Array.AsReadOnly(_menus.OrderBy(m => m.Path, StringComparer.Ordinal).ThenBy(m => m.Command, StringComparer.Ordinal).ToArray());
    /// <summary>Register one command; duplicate native or extension ids are rejected.</summary>
    public void RegisterCommand(string id, string title, Action<EditorContext> execute, string summary = "")
    {
        if (string.IsNullOrWhiteSpace(id) || id.Any(c => !char.IsAsciiLetterOrDigit(c) && c is not ('.' or '_' or '-'))) throw new ArgumentException("command id must contain letters, digits, dots, underscores or hyphens", nameof(id));
        ArgumentException.ThrowIfNullOrWhiteSpace(title);
        ArgumentNullException.ThrowIfNull(execute);
        if (_reserved.Contains(id) || !_commands.TryAdd(id, (new(id, title, summary), execute))) throw new ArgumentException($"command `{id}` is already registered", nameof(id));
    }
    /// <summary>Place a registered command in a menu; no separate action or mutation path is created.</summary>
    public void AddMenu(string path, string command)
    {
        if (!_commands.ContainsKey(command) && !_reserved.Contains(command)) throw new ArgumentException($"unknown registry command `{command}`", nameof(command));
        if (string.IsNullOrWhiteSpace(path) || path.Split('/').Any(string.IsNullOrWhiteSpace)) throw new ArgumentException("menu path must contain nonempty segments", nameof(path));
        if (_menus.Any(m => m.Path == path)) throw new ArgumentException($"menu path `{path}` is already registered", nameof(path));
        _menus.Add(new(path, command));
    }
    /// <summary>Invoke a registered extension callback; the returned context only contains queued commands.</summary>
    public void Invoke(string command, EditorContext context)
    {
        if (_disabled.ContainsKey(command)) throw new InvalidOperationException($"editor command `{command}` is disabled until its next successful reload");
        if (!_commands.TryGetValue(command, out var entry)) throw new ArgumentException($"unknown extension command `{command}`", nameof(command));
        entry.Run(context);
    }
    internal void Disable(string command, string reason) => _disabled.TryAdd(command, reason);
}

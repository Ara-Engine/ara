namespace Ara.Mod;

/// <summary>Mod entry point, found by name by <c>Exports.LoadMod</c>.</summary>
public static class Entry
{
    /// <summary>Returns a greeting proving the mod's code ran.</summary>
    public static string Run() => $"hello from {typeof(Entry).Assembly.GetName().Name}, loaded at runtime";
}

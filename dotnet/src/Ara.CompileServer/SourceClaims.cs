using System.Security.Cryptography;
using System.Text.Json;

namespace Ara.CompileServer;

/// <summary>Private source.replace receipt, identified by its job and exact byte revision.</summary>
internal sealed record SourceClaim(string Edit, string Hash, string Phase);

/// <summary>Pending versions survive worker latency; write echoes match bytes, never a timer.</summary>
internal sealed class SourceClaims
{
    sealed class Entry
    {
        internal readonly Dictionary<string, string> Pending = new(StringComparer.Ordinal);
        internal string? Echo;
    }
    readonly Dictionary<string, Entry> _paths = new(ModuleState.Paths);

    internal static SourceClaim Parse(string json)
    {
        var claim = JsonSerializer.Deserialize<SourceClaim>(json, new JsonSerializerOptions { PropertyNameCaseInsensitive = true })
            ?? throw new ArgumentException("Missing source claim");
        if (string.IsNullOrEmpty(claim.Edit) || claim.Edit.Length > 256 || claim.Hash is null || claim.Hash.Length != 64 || !claim.Hash.All(Uri.IsHexDigit)
            || claim.Phase is not ("preparing" or "materialized" or "cancelled"))
            throw new ArgumentException("Invalid source claim edit, SHA-256 or phase");
        return claim with { Hash = claim.Hash.ToLowerInvariant() };
    }

    /// <summary>Reads exact bytes on the compiler/worker side, allowing atomic file replacement.</summary>
    internal static string? Hash(string path)
    {
        try
        {
            using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            return Convert.ToHexStringLower(SHA256.HashData(file));
        }
        catch (IOException) { return null; }
        catch (UnauthorizedAccessException) { return null; }
    }

    /// <summary>Called under the coordinator lock. Returns true only for a verified completion.</summary>
    internal bool Update(string path, SourceClaim claim, string? current)
    {
        if (!_paths.TryGetValue(path, out var entry))
        {
            if (claim.Phase != "preparing") return false;
            _paths[path] = entry = new();
        }
        if (claim.Phase == "preparing") entry.Pending[claim.Edit] = claim.Hash;
        else if (entry.Pending.Remove(claim.Edit, out var expected) && expected == claim.Hash
            && claim.Phase == "materialized" && current == expected)
        {
            entry.Echo = expected;
            return true;
        }
        if (entry.Echo is null && entry.Pending.Count == 0) _paths.Remove(path);
        return false;
    }

    /// <summary>Immediate legacy compile requests claim only their current byte revision.</summary>
    internal void Current(string path, string? hash)
    {
        if (hash is null) return;
        if (!_paths.TryGetValue(path, out var entry)) _paths[path] = entry = new();
        entry.Echo = hash;
    }

    /// <summary>Human bytes win, including mixed batches; duplicate write echoes stay attributed.</summary>
    internal bool Matches(string path, string? hash)
    {
        if (!_paths.TryGetValue(path, out var entry)) return false;
        if (hash is not null && (entry.Echo == hash || entry.Pending.ContainsValue(hash)))
        {
            entry.Echo = hash;
            return true;
        }
        entry.Echo = null;
        if (entry.Pending.Count == 0) _paths.Remove(path);
        return false;
    }
}

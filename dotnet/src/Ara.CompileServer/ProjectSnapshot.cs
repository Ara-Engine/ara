using System.Text.Json;
using Ara.CompileServer;

namespace Ara.Hosting;

// CompileServer only; Host continues linking the unchanged PublishedSnapshot.cs.
internal sealed partial record PublishedSnapshot
{
    /// <summary>Serializes the hashes derived from the same request's actual source reads.</summary>
    internal static byte[] ProjectManifestBytes(string name, bool hot, string profile, string[] references,
        string fingerprint, byte[] pe, byte[] pdb, ProjectSourceCapture capture,
        IReadOnlyDictionary<string, string>? dependencies = null, string? graph = null) =>
        JsonSerializer.SerializeToUtf8Bytes(new Manifest(name, hot, profile, references, fingerprint,
            Hash(pe), Hash(pdb), capture.Sources.Select(source => new Input(FullPath(source.Path), source.Hash)).ToArray(),
            dependencies?.ToDictionary(pair => pair.Key, pair => pair.Value) ?? new Dictionary<string, string>(), graph));
}

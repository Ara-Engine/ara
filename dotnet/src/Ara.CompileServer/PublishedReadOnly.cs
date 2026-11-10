using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;

namespace Ara.Hosting;

internal sealed partial record PublishedSnapshot
{
    /// <summary>Preserves an explicit unwatched image without inventing committed source bodies.</summary>
    internal static PublishedSnapshot CaptureReadOnly(string directory, string assembly, string name,
        bool hot, IReadOnlyCollection<string> references, string profile, Func<string, string?> dependencyFingerprint)
    {
        var pe = File.ReadAllBytes(assembly);
        var pdbPath = Path.ChangeExtension(assembly, ".pdb");
        var pdb = File.Exists(pdbPath) ? File.ReadAllBytes(pdbPath) : [];
        using (var image = new PEReader(new MemoryStream(pe)))
            if (!image.HasMetadata || !image.GetMetadataReader().IsAssembly) throw new BadImageFormatException("the explicit prebuilt image is not a managed assembly");
        if (pdb.Length > 0 && !SymbolsMatch(pe, pdb)) throw new InvalidDataException("the explicit prebuilt symbols do not match the supplied assembly");
        var opaque = new PublishedSnapshot(name, hot, 0, "opaque:" + Hash(pe), Hash(pe), Hash(pdb), pe, pdb, []) { HasCommittedSources = false };
        try
        {
            var manifest = JsonSerializer.Deserialize<Manifest>(File.ReadAllBytes(Path.Combine(directory, ManifestFile)), Json);
            if (manifest is null || manifest.Name != name || manifest.Hot != hot || manifest.Profile != profile
                || manifest.References is null || !manifest.References.SequenceEqual(references)
                || string.IsNullOrEmpty(manifest.Fingerprint)
                || manifest.Sources is null || manifest.Sources.Any(source => source is null || !Path.IsPathFullyQualified(source.Path))
                || manifest.Sources.Select(source => source.Path).Distinct(StringComparer.OrdinalIgnoreCase).Count() != manifest.Sources.Length
                || string.IsNullOrEmpty(manifest.Graph) || !CacheMatches(directory, manifest.Fingerprint, pe, pdb)) return opaque;
            var publication = File.ReadAllLines(Path.Combine(directory, "published.sha256"));
            if (publication.Length != 3 || publication[0] != manifest.Fingerprint || publication[2] != opaque.PeHash
                || File.ReadAllText(Path.Combine(directory, "input.sha256")).Trim() != manifest.Fingerprint) return opaque;
            using var inputs = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(directory, "module.inputs.json")));
            var project = inputs.RootElement.GetProperty("project").GetString()!;
            var listed = inputs.RootElement.GetProperty("sources");
            var owned = listed.EnumerateArray().Select(source => FullPath(Path.Combine(project,
                source.GetProperty("storagePath").GetString()!))).ToHashSet(StringComparer.OrdinalIgnoreCase);
            if (inputs.RootElement.GetProperty("name").GetString() != name || owned.Count != listed.GetArrayLength()
                || owned.Count != manifest.Sources.Length || manifest.Sources.Any(source => !owned.Contains(source.Path))) return opaque;
            // These are image publication/ownership facts only. Saved source bytes
            // are deliberately never read, let alone claimed as committed bodies.
            var paths = manifest.Sources.Select(source => source.Path).ToArray();
            var dependenciesMatch = manifest.Dependencies is not null && manifest.Dependencies.Count == references.Count
                && references.All(reference => manifest.Dependencies.TryGetValue(reference, out var fingerprint)
                    && !string.IsNullOrEmpty(fingerprint) && fingerprint == dependencyFingerprint(reference));
            return dependenciesMatch
                ? opaque with { Fingerprint = manifest.Fingerprint, Graph = manifest.Graph, OwnedPaths = paths, Dependencies = manifest.Dependencies! }
                : opaque with { Graph = manifest.Graph, OwnedPaths = paths };
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException
            or ArgumentException or InvalidOperationException or KeyNotFoundException) { return opaque; }
    }
}

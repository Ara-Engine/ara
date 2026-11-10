using System.Security.Cryptography;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>The exact published bytes and owned source texts captured before loading an image.</summary>
internal sealed partial record PublishedSnapshot(string Name, bool Hot, uint Generation, string Fingerprint,
    string PeHash, string PdbHash, byte[] Pe, byte[] Pdb, PublishedSource[] Sources)
{
    internal IReadOnlyDictionary<string, string> Dependencies { get; init; } = new Dictionary<string, string>();
    internal string? Graph { get; init; }
    internal bool HasCommittedSources { get; init; } = true;
    internal string[]? OwnedPaths { get; init; }
    internal const string ManifestFile = "source.snapshot.json";
    static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };
    sealed record Input(string Path, string Hash);
    sealed record Manifest(string Name, bool Hot, string Profile, string[] References,
        string Fingerprint, string PeHash, string PdbHash, Input[] Sources, Dictionary<string, string> Dependencies, string? Graph);

    internal static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));
    internal static string TextHash(string text) => Hash(Encoding.UTF8.GetBytes(text));

    /// <summary>Only reuse symbols whose complete bytes were trusted when the publication was emitted.</summary>
    internal static bool CacheMatches(string directory, string fingerprint, byte[] pe, byte[] pdb)
    {
        try
        {
            var prior = JsonSerializer.Deserialize<Manifest>(File.ReadAllBytes(Path.Combine(directory, ManifestFile)), Json);
            return prior is not null && prior.Fingerprint == fingerprint && prior.PeHash == Hash(pe)
                && prior.PdbHash == Hash(pdb) && SymbolsMatch(pe, pdb);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException)
        {
            return false;
        }
    }

    /// <summary>Portable symbols must identify this PE, not another publication.</summary>
    internal static bool SymbolsMatch(byte[] pe, byte[] pdb)
    {
        if (pdb.Length == 0) return false;
        try
        {
            using var reader = new PEReader(new MemoryStream(pe));
            var entry = reader.ReadDebugDirectory().FirstOrDefault(entry => entry.Type == DebugDirectoryEntryType.CodeView);
            if (entry.Type != DebugDirectoryEntryType.CodeView) return false;
            var codeView = reader.ReadCodeViewDebugDirectoryData(entry);
            using var symbols = MetadataReaderProvider.FromPortablePdbStream(new MemoryStream(pdb));
            var header = symbols.GetMetadataReader().DebugMetadataHeader;
            if (header is null) return false;
            var id = new BlobContentId(header.Id);
            return codeView.Guid == id.Guid && entry.Stamp == id.Stamp;
        }
        catch (Exception error) when (error is BadImageFormatException or IOException or ArgumentException or InvalidOperationException) { return false; }
    }

    /// <summary>Describes the actual read texts and emitted or validated cached image, never later rereads.</summary>
    internal static byte[] ManifestBytes(string name, bool hot, string profile, string[] references,
        string fingerprint, byte[] pe, byte[] pdb, IEnumerable<(string Path, string Text)> sources,
        IReadOnlyDictionary<string, string>? dependencies = null, string? graph = null) =>
        JsonSerializer.SerializeToUtf8Bytes(new Manifest(name, hot, profile, references, fingerprint,
            Hash(pe), Hash(pdb), sources.Select(source => new Input(FullPath(source.Path), TextHash(source.Text))).ToArray(),
            dependencies?.ToDictionary(pair => pair.Key, pair => pair.Value) ?? new Dictionary<string, string>(), graph));

    /// <summary>Invalid or obsolete publication metadata takes the ordinary initial compilation path.</summary>
    internal static PublishedSnapshot? TryCapture(string directory, string assembly, string name,
        bool hot, IReadOnlyCollection<string> references, string profile, Func<string, string?>? dependencyFingerprint = null)
    {
        try
        {
            var manifest = JsonSerializer.Deserialize<Manifest>(File.ReadAllBytes(Path.Combine(directory, ManifestFile)), Json);
            if (manifest is null || manifest.Name != name || manifest.Hot != hot || manifest.Profile != profile
                || manifest.References is null || !manifest.References.SequenceEqual(references)
                || manifest.Sources is null || string.IsNullOrEmpty(manifest.Fingerprint) || string.IsNullOrEmpty(manifest.Graph)
                || manifest.Sources.Any(source => source is null || string.IsNullOrEmpty(source.Path) || string.IsNullOrEmpty(source.Hash))
                || manifest.Dependencies is null || manifest.Dependencies.Count != references.Count
                || references.Any(reference => !manifest.Dependencies.TryGetValue(reference, out var fingerprint)
                    || string.IsNullOrEmpty(fingerprint) || fingerprint != dependencyFingerprint?.Invoke(reference))) return null;
            var stampPath = Path.Combine(directory, "published.sha256");
            var stamp = File.ReadAllText(stampPath);
            var publication = stamp.Split('\n', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
            if (publication.Length != 3 || publication[0] != manifest.Fingerprint || publication[2] != manifest.PeHash
                || File.ReadAllText(Path.Combine(directory, "input.sha256")).Trim() != manifest.Fingerprint) return null;
            var inputPath = Path.Combine(directory, "module.inputs.json");
            var inputBytes = File.ReadAllBytes(inputPath);
            using var inputs = JsonDocument.Parse(inputBytes);
            var project = inputs.RootElement.GetProperty("project").GetString()!;
            if (inputs.RootElement.GetProperty("name").GetString() != name) return null;
            var paths = inputs.RootElement.GetProperty("sources").EnumerateArray()
                .Select(source => FullPath(Path.Combine(project, source.GetProperty("storagePath").GetString()!))).ToHashSet(StringComparer.OrdinalIgnoreCase);
            if (paths.Count != inputs.RootElement.GetProperty("sources").GetArrayLength()
                || paths.Count != manifest.Sources.Length || manifest.Sources.Select(source => source.Path).Distinct(StringComparer.OrdinalIgnoreCase).Count() != paths.Count
                || manifest.Sources.Any(source => !paths.Contains(source.Path))) return null;
            // This PE read supplies both Stage and Watch. Reopening a mutable path later
            // could give the server a different generation from the one actually loaded.
            var pe = File.ReadAllBytes(assembly);
            var pdbPath = Path.ChangeExtension(assembly, ".pdb");
            var pdb = File.Exists(pdbPath) ? File.ReadAllBytes(pdbPath) : [];
            if (Hash(pe) != manifest.PeHash || Hash(pdb) != manifest.PdbHash || !SymbolsMatch(pe, pdb)) return null;
            var sources = manifest.Sources.Select(source => new PublishedSource(source.Path, ReadText(source.Path), source.Hash)).ToArray();
            if (sources.Any(source => TextHash(source.Text) != source.Hash)
                || File.ReadAllText(stampPath) != stamp || !File.ReadAllBytes(inputPath).AsSpan().SequenceEqual(inputBytes)) return null;
            return new(name, hot, 0, manifest.Fingerprint, manifest.PeHash, manifest.PdbHash, pe, pdb, sources) { Dependencies = manifest.Dependencies, Graph = manifest.Graph };
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException
            or KeyNotFoundException or InvalidOperationException or ArgumentException)
        {
            return null;
        }
    }

    internal bool Valid(string name, bool hot, uint generation) => Name == name && Hot == hot && Generation == generation
        && !string.IsNullOrEmpty(Fingerprint) && Pe.Length > 0 && Hash(Pe) == PeHash && Hash(Pdb) == PdbHash
        && (HasCommittedSources ? SymbolsMatch(Pe, Pdb) : Pdb.Length == 0 || SymbolsMatch(Pe, Pdb))
        && (HasCommittedSources || Sources.Length == 0)
        && (OwnedPaths is null || OwnedPaths.Distinct(StringComparer.OrdinalIgnoreCase).Count() == OwnedPaths.Length && OwnedPaths.All(Path.IsPathFullyQualified))
        && Sources.Select(source => source.Path).Distinct(StringComparer.OrdinalIgnoreCase).Count() == Sources.Length
        && Sources.All(source => Path.IsPathFullyQualified(source.Path) && TextHash(source.Text) == source.Hash);

    internal void Write(Protocol.Writer writer)
    {
        writer.Str(Name).Bool(Hot).U32(Generation).Str(Fingerprint).Str(PeHash).Str(PdbHash).Bytes(Pe).Bytes(Pdb).U32((uint)Sources.Length);
        foreach (var source in Sources) writer.Str(source.Path).Str(source.Text).Str(source.Hash);
        writer.U32((uint)Dependencies.Count);
        foreach (var (name, fingerprint) in Dependencies) writer.Str(name).Str(fingerprint);
        writer.Str(Graph ?? "");
        writer.Bool(HasCommittedSources).Bool(OwnedPaths is not null);
        if (OwnedPaths is not null) writer.Strs(OwnedPaths);
    }

    internal static PublishedSnapshot Read(Protocol.Reader reader)
    {
        var (name, hot, generation, fingerprint, peHash, pdbHash, pe, pdb) =
            (reader.Str(), reader.Bool(), reader.U32(), reader.Str(), reader.Str(), reader.Str(), reader.Bytes(), reader.Bytes());
        var sources = new PublishedSource[reader.U32()];
        for (var index = 0; index < sources.Length; index++) sources[index] = new(reader.Str(), reader.Str(), reader.Str());
        var dependencies = new Dictionary<string, string>(StringComparer.Ordinal);
        var count = reader.U32();
        for (var index = 0; index < count; index++) dependencies.Add(reader.Str(), reader.Str());
        var graph = reader.Str();
        var hasSources = reader.Bool();
        var ownedPaths = reader.Bool() ? reader.Strs() : null;
        return new(name, hot, generation, fingerprint, peHash, pdbHash, pe, pdb, sources)
        { Dependencies = dependencies, Graph = graph, HasCommittedSources = hasSources, OwnedPaths = ownedPaths };
    }

    static string FullPath(string path) => Path.GetFullPath(path.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)
        ? @"\\" + path[8..] : path.StartsWith(@"\\?\", StringComparison.Ordinal) ? path[4..] : path);

    static string ReadText(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }
}

/// <summary>An owned source's exact decoded text and publication hash, including component-only files.</summary>
internal sealed record PublishedSource(string Path, string Text, string Hash);

using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer;

/// <summary>A loaded publication without Roslyn compilations or an invented EnC baseline.</summary>
internal sealed class PublishedLoaded(PublishedSnapshot snapshot, ImmutableDictionary<string, MetadataReference> references)
{
    internal readonly MetadataReference Reference = MetadataReference.CreateFromImage(snapshot.Pe);
    internal readonly ImmutableDictionary<string, string>? Texts = snapshot.HasCommittedSources
        ? snapshot.Sources.ToImmutableDictionary(source => source.Path, source => source.Text, ModuleState.Paths) : null;
    internal readonly IEnumerable<string>? Paths = snapshot.HasCommittedSources
        ? snapshot.Sources.Select(source => source.Path).ToImmutableArray()
        : snapshot.OwnedPaths is { } paths ? paths.ToImmutableArray() : null;
    internal readonly bool HasCommittedSources = snapshot.HasCommittedSources;
    internal readonly bool Hot = snapshot.Hot;
    internal readonly uint Generation = snapshot.Generation;
    internal readonly string Fingerprint = snapshot.Fingerprint;
    internal readonly ImmutableDictionary<string, string> Dependencies = snapshot.Dependencies.ToImmutableDictionary();
    internal readonly ImmutableDictionary<string, MetadataReference> DepRefs = references;
}

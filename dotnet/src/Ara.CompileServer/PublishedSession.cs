using System.Collections.Immutable;
using Ara.Hosting;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Text;

namespace Ara.CompileServer;

// Published images stay distinct from compilations until actual work emits a new image.
internal sealed partial class ModuleSession
{
    PublishedLoaded? _published;
    internal bool Loaded => _state is not null || _published is not null;
    internal bool? LoadedHot => _state?.Hot ?? _published?.Hot;
    internal MetadataReference? LoadedReference => _state?.Reference ?? _published?.Reference;
    internal ImmutableDictionary<string, string>? LoadedTexts => _state?.Texts ?? _published?.Texts;
    internal IEnumerable<string>? LoadedPaths => _state?.Texts.Keys ?? _published?.Paths;
    internal bool ReadOnlyPublished => _published is { HasCommittedSources: false };
    internal string? PublicationFingerprint => _published?.Fingerprint;
    internal bool ImportPublished(PublishedSnapshot snapshot, ICompileContext? context = null)
    {
        if (!snapshot.Valid(Name, _startHot, InitialGeneration)
            || snapshot.HasCommittedSources && (_project is not null && string.IsNullOrEmpty(snapshot.Graph)
                || snapshot.Dependencies.Count != References.Count || References.Any(name => !snapshot.Dependencies.TryGetValue(name, out var fingerprint) || string.IsNullOrEmpty(fingerprint)))
            || snapshot.Dependencies.Any(pair => !References.Contains(pair.Key) || string.IsNullOrEmpty(pair.Value))) return false;
        var refs = ImmutableDictionary.CreateBuilder<string, MetadataReference>();
        if (context is not null)
            foreach (var (name, fingerprint) in snapshot.Dependencies)
                if (context.PublishedFingerprint(name) == fingerprint && context.Reference(name) is { } reference) refs[name] = reference;
        try
        {
            _published = new(snapshot, refs.ToImmutable());
            if (_project is not null) _graph = snapshot.Graph;
            return true;
        }
        catch (Exception error) when (error is BadImageFormatException or ArgumentException) { return false; }
    }
    internal bool PublishedReferencesDrift(ICompileContext context) => _published is { } published && !PublishedReferencesMatch(published, context);
    static bool PublishedReferencesMatch(PublishedLoaded published, ICompileContext context) => published.Dependencies.All(pair =>
        context.PublishedFingerprint(pair.Key) == pair.Value || published.DepRefs.TryGetValue(pair.Key, out var reference) && ReferenceEquals(reference, context.Reference(pair.Key)));
    internal bool PublishedSourcesDrift => _published is { HasCommittedSources: true, Texts: { } texts }
        && (_project is { } project && Graph(project.Scan()) != _graph
            || !texts.Keys.ToHashSet(ModuleState.Paths).SetEquals(SourceFiles())
            || texts.Any(source => ReadWithRetry(source.Key) != source.Value));

    Outcome ReadOnlyWork(Phases timing) => new(this, OutcomeKind.Errors)
    {
        Timing = timing,
        Intent = CompileIntent.CommittedCode,
        ValidatedInputs = false,
        Extra = [(AraCodes.LoadFailed, "error", $"module `{Name}` is an explicit prebuilt dependency without trusted committed source bodies; republish and reload it before recompiling, changing its heat, or rebinding its dependencies. Its loaded image was retained.")],
    };

    bool MatchesPublishedInputs(PublishedLoaded published, ICompileContext context)
    {
        var current = _project?.Scan();
        if (current is not null && (current.Diagnostics.Length != 0 || Graph(current) != _graph || OpaqueInventoryChanged(current, context) || OwnershipChanged(current, context))) return false;
        if (current is not null) ObserveUnloadedDomains(current, context);
        return published.Texts is { } texts && SourcesMatch(texts, SourceFiles(current)) && PublishedReferencesMatch(published, context);
    }

    // Construct real source compilations only for actual work. They are preparation
    // state, never committed facts about the loaded PE; only a successful new emit
    // may create the first EnC baseline and replace PublishedLoaded.
    Outcome BuildPublished(ICompileContext ctx, Phases timing, string[]? currentFiles,
        CompileIntent intent, Escalation escalation, bool? heat, CancellationToken ct, bool ownEdit = false)
    {
        var published = _published ?? throw new InvalidOperationException("a loaded publication is required");
        if (!published.HasCommittedSources) return ReadOnlyWork(timing);
        var committed = published.Texts ?? throw new InvalidOperationException("trusted publications retain committed source texts");
        var texts = currentFiles is null ? committed : currentFiles.ToImmutableDictionary(path => path,
            path => ReadWithRetry(path) ?? throw new FileNotFoundException("module source disappeared while capturing inputs", path), ModuleState.Paths);
        var changed = texts.Count != committed.Count || texts.Any(pair => !committed.TryGetValue(pair.Key, out var text) || text != pair.Value);
        // Watchers can fan out a save to unchanged modules. This check must precede
        // every Roslyn tree/compilation allocation, including the first watched save.
        if (ownEdit && !changed && PublishedReferencesMatch(published, ctx))
            return new(this, OutcomeKind.None) { ValidatedInputs = true, Intent = intent, Timing = timing };
        if (ownEdit && !changed) escalation = Escalation.Dependency;
        _disk.Clear();
        foreach (var (path, text) in texts) _disk[path] = text;
        var trees = texts.ToImmutableDictionary(pair => pair.Key, pair => ModuleCompiler.Parse(pair.Key, pair.Value), ModuleState.Paths);
        var (refs, surfaces) = Dependencies(ctx);
        var hot = (heat ?? (changed || published.Hot)) && ctx.Enc;
        var user = _compiler.Create([.. trees.Values, Usings], hot, refs.Values);
        var candidate = new ModuleState(user, user, ImmutableDictionary<string, SourceText>.Empty,
            trees, texts, null, hot, published.Generation, null, surfaces, refs);
        timing.Parse = timing.Mark();
        var outcome = Build(candidate.User, candidate, AbiConst.ReloadL3, escalation, timing,
            [.. candidate.Trees.Values], null, ctx, true, ct);
        outcome.ValidatedInputs = intent == CompileIntent.CurrentSources || MatchesCurrentInputs(candidate, ctx);
        outcome.Intent = intent;
        return outcome;
    }
}

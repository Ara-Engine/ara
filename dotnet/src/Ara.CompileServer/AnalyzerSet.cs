using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Diagnostics.CodeAnalysis;
using Ara.Analyzers;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Runs the Ara analyzers (ARA0001-ARA0506) over the files an edit touched, so a reload
/// with an analyzer error is refused like a compile error (the last good code keeps running).
/// The module's profile (<c>gameplay</c> or <c>editor</c>) selects the API allowlist.
/// </summary>
internal sealed class AnalyzerSet : IAnalysis
{
    readonly ImmutableArray<DiagnosticAnalyzer> _analyzers =
        [new UnityAnalyzer(), new StateAnalyzer(), new DeterminismAnalyzer(), new AllocationAnalyzer(), new AotAnalyzer(), new ApiAccessAnalyzer()];

    readonly ConcurrentDictionary<string, AnalyzerOptions> _options = new();

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile)
    {
        var analyzerOptions = _options.GetOrAdd(profile, p => new AnalyzerOptions([], new ProfileOptions(p)));
        var options = new CompilationWithAnalyzersOptions(analyzerOptions, onAnalyzerException: null,
            concurrentAnalysis: true, logAnalyzerExecutionTime: false);
        var cwa = compilation.WithAnalyzers(_analyzers, options);
        var all = ImmutableArray.CreateBuilder<Diagnostic>();
        foreach (var tree in trees)
        {
            all.AddRange(cwa.GetAnalyzerSyntaxDiagnosticsAsync(tree, CancellationToken.None).GetAwaiter().GetResult());
            var model = compilation.GetSemanticModel(tree);
            all.AddRange(cwa.GetAnalyzerSemanticDiagnosticsAsync(model, null, CancellationToken.None).GetAwaiter().GetResult());
        }
        return all.ToImmutable();
    }

    /// <summary>The MSBuild property an IDE build would pass: <c>build_property.AraModuleProfile</c>.</summary>
    sealed class ProfileOptions(string profile) : AnalyzerConfigOptionsProvider
    {
        readonly Options _global = new(profile);

        public override AnalyzerConfigOptions GlobalOptions => _global;
        public override AnalyzerConfigOptions GetOptions(SyntaxTree tree) => Options.None;
        public override AnalyzerConfigOptions GetOptions(AdditionalText textFile) => Options.None;

        sealed class Options(string? profile) : AnalyzerConfigOptions
        {
            internal static readonly Options None = new(null);

            public override bool TryGetValue(string key, [NotNullWhen(true)] out string? value)
            {
                value = profile is not null && key == ApiAccessAnalyzer.ProfileOption ? profile : null;
                return value is not null;
            }
        }
    }
}

using System.Collections.Immutable;
using Ara.Analyzers;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Runs the Ara analyzers (ARA0001-ARA0400) over the files an edit touched, so a reload
/// with an analyzer error is refused like a compile error (the last good code keeps running).
/// </summary>
internal sealed class AnalyzerSet : IAnalysis
{
    readonly ImmutableArray<DiagnosticAnalyzer> _analyzers =
        [new UnityAnalyzer(), new StateAnalyzer(), new DeterminismAnalyzer(), new AllocationAnalyzer(), new AotAnalyzer()];

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile)
    {
        var options = new CompilationWithAnalyzersOptions(new AnalyzerOptions([]), onAnalyzerException: null,
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
}

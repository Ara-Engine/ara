using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Diagnostics.CodeAnalysis;
using System.Globalization;
using Ara.Analyzers;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.CompileServer;

/// <summary>
/// Runs the Ara analyzers (ARA0001-ARA0506) over eligible methods from a proven clean
/// baseline, full touched trees otherwise, or the full module for declaration/reference
/// changes. Analyzer errors retain the last good code.
/// The module's profile (<c>gameplay</c> or <c>editor</c>) selects the API allowlist.
/// </summary>
internal sealed class AnalyzerSet : IAnalysis
{
    readonly ImmutableArray<DiagnosticAnalyzer> _analyzers =
        [new UnityAnalyzer(), new StateAnalyzer(), new DeterminismAnalyzer(), new AllocationAnalyzer(), new AotAnalyzer(), new ApiAccessAnalyzer()];

    readonly ConcurrentDictionary<string, AnalyzerOptions> _options = new();

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile) =>
        Analyze(compilation, trees, profile, CancellationToken.None);

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> Analyze(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        var failures = new ConcurrentQueue<Diagnostic>();
        var cwa = CreateDriver(compilation, profile, (_, _, diagnostic) => failures.Enqueue(Failure(diagnostic)));
        var all = ImmutableArray.CreateBuilder<Diagnostic>();
        foreach (var tree in trees)
        {
            all.AddRange(cwa.GetAnalyzerSyntaxDiagnosticsAsync(tree, ct).GetAwaiter().GetResult());
            var model = compilation.GetSemanticModel(tree);
            all.AddRange(cwa.GetAnalyzerSemanticDiagnosticsAsync(model, null, ct).GetAwaiter().GetResult());
        }
        all.AddRange(failures);
        ct.ThrowIfCancellationRequested();
        return all.ToImmutable();
    }

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> AnalyzeBodies(Compilation compilation, IReadOnlyCollection<MethodDeclarationSyntax> methods, string profile) =>
        AnalyzeBodies(compilation, methods, profile, CancellationToken.None);

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> AnalyzeBodies(Compilation compilation, IReadOnlyCollection<MethodDeclarationSyntax> methods, string profile, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        // Tree/span requests omit Roslyn's nonlocal failure diagnostics. Capture
        // the callback too, so a failed analyzer cannot establish clean trust.
        var failures = new ConcurrentQueue<Diagnostic>();
        var cwa = CreateDriver(compilation, profile, (_, _, diagnostic) => failures.Enqueue(Failure(diagnostic)));
        var all = ImmutableArray.CreateBuilder<Diagnostic>();
        foreach (var group in methods.GroupBy(method => method.SyntaxTree))
        {
            all.AddRange(cwa.GetAnalyzerSyntaxDiagnosticsAsync(group.Key, ct).GetAwaiter().GetResult());
            var model = compilation.GetSemanticModel(group.Key);
            foreach (var method in group)
            {
                var diagnostics = cwa.GetAnalyzerSemanticDiagnosticsAsync(model, method.Span, ct).GetAwaiter().GetResult();
                // Successive Roslyn requests can include earlier cached diagnostics.
                // Each disjoint ordinary method owns only its own source diagnostics.
                all.AddRange(diagnostics.Where(diagnostic => Owns(method, diagnostic)));
            }
        }
        all.AddRange(failures);
        ct.ThrowIfCancellationRequested();
        return all.ToImmutable();
    }

    /// <summary>Owns this method's source diagnostics; failures without source locations must never disappear.</summary>
    internal static bool Owns(MethodDeclarationSyntax method, Diagnostic diagnostic) =>
        !diagnostic.Location.IsInSource || (diagnostic.Location.SourceTree == method.SyntaxTree
            && method.Span.Contains(diagnostic.Location.SourceSpan));

    /// <summary>Analyzes a full project in one Roslyn request, allowing work across files to run concurrently.</summary>
    internal ImmutableArray<Diagnostic> AnalyzeProject(Compilation compilation, string profile, CancellationToken ct = default)
    {
        ct.ThrowIfCancellationRequested();
        var failures = new ConcurrentQueue<Diagnostic>();
        var cwa = CreateDriver(compilation, profile, (_, _, diagnostic) => failures.Enqueue(Failure(diagnostic)));
        var diagnostics = cwa.GetAnalyzerDiagnosticsAsync(ct).GetAwaiter().GetResult();
        ct.ThrowIfCancellationRequested();
        return [.. diagnostics, .. failures];
    }

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> AnalyzeModule(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile) =>
        AnalyzeProject(compilation, profile);

    /// <inheritdoc/>
    public ImmutableArray<Diagnostic> AnalyzeModule(Compilation compilation, IReadOnlyCollection<SyntaxTree> trees, string profile, CancellationToken ct) =>
        AnalyzeProject(compilation, profile, ct);

    // Roslyn reports analyzer exceptions as warnings. An unchecked candidate must
    // retain the running code; its external AD diagnostic maps to existing ARA0900.
    static Diagnostic Failure(Diagnostic diagnostic) => Diagnostic.Create(
        new DiagnosticDescriptor(diagnostic.Id, diagnostic.Descriptor.Title, "{0}", diagnostic.Descriptor.Category,
            DiagnosticSeverity.Error, isEnabledByDefault: true), diagnostic.Location,
        additionalLocations: diagnostic.AdditionalLocations, properties: diagnostic.Properties,
        messageArgs: [diagnostic.GetMessage(CultureInfo.InvariantCulture)]);

    CompilationWithAnalyzers CreateDriver(Compilation compilation, string profile, Action<Exception, DiagnosticAnalyzer, Diagnostic>? onException)
    {
        var analyzerOptions = _options.GetOrAdd(profile, p => new AnalyzerOptions([], new ProfileOptions(p)));
        var options = new CompilationWithAnalyzersOptions(analyzerOptions, onAnalyzerException: onException,
            concurrentAnalysis: true, logAnalyzerExecutionTime: false);
        return compilation.WithAnalyzers(_analyzers, options);
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

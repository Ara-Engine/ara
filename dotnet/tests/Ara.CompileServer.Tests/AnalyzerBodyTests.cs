using Ara.Hosting;
using System.Collections.Immutable;
using System.Reflection;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;
using Microsoft.CodeAnalysis.Operations;

namespace Ara.CompileServer.Tests;

public sealed class AnalyzerBodyTests
{
    [Fact]
    public void A_throwing_analyzer_returns_a_nonempty_raw_result_for_scoped_requests()
    {
        var tree = ModuleCompiler.Parse("Failure.cs", "static class C { static int M() => System.Math.Abs(-2); }");
        var compiler = new ModuleCompiler("Failure", [typeof(Behaviour).Assembly.Location]);
        var compilation = compiler.Create([tree], true, []);
        var method = Assert.Single(tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>());
        var set = new AnalyzerSet();
        // Inject a failure without exposing custom analyzer configuration in the product API.
        typeof(AnalyzerSet).GetField("_analyzers", BindingFlags.Instance | BindingFlags.NonPublic)!
            .SetValue(set, ImmutableArray.Create<DiagnosticAnalyzer>(new FailingAnalyzer()));
        var diagnostics = set.AnalyzeBodies(compilation, [method], "gameplay");
        Assert.Contains(diagnostics, diagnostic => diagnostic.Id == "AD0001");
    }

    [Theory]
    [InlineData("p.Value += DateTime.UtcNow.Second;", "ARA0200")]
    [InlineData("p.Value += new int[2].Length;", "ARA0300")]
    [InlineData("p.Value += typeof(Probe).GetMethods().Length;", "ARA0400")]
    [InlineData("p.Value += System.Environment.NewLine.Length;", "ARA0506")]
    [InlineData("p.Value += GetComponent<Probe>().Value;", "ARA0001")]
    [InlineData("p.Value += Math.Abs(-2);", null)]
    public void A_body_edit_preserves_the_full_tree_diagnostics(string body, string? expected)
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", body, StringComparison.Ordinal));
        var outcome = fixture.Compile(0);
        Assert.Equal(expected is "ARA0200" or "ARA0400" or "ARA0506" or "ARA0001" ? OutcomeKind.Errors : OutcomeKind.Delta, outcome.Kind);
        if (expected is null) Assert.Empty(outcome.Problems);
        else Assert.Equal(expected, Assert.Single(outcome.Problems).Id);
        Assert.True(fixture.Module.State!.AnalyzerClean);
        if (outcome.Next is { } next)
        {
            var tree = next.Trees[fixture.Paths[0]];
            var methods = tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>()
                .Where(method => method.Identifier.ValueText == "First").ToArray();
            var analyzer = new AnalyzerSet();
            Assert.Equal(Exact(analyzer.Analyze(next.Final, [tree], "gameplay")),
                Exact(analyzer.AnalyzeBodies(next.Final, methods, "gameplay")));
            fixture.Module.Commit(outcome);
            Assert.Equal(expected is null, fixture.Module.State!.AnalyzerClean);
        }
    }

    [Fact]
    public void Disjoint_methods_in_two_files_own_each_diagnostic_once_in_either_order()
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += new int[2].Length;", StringComparison.Ordinal)
            .Replace("return 1;", "return new int[3].Length;", StringComparison.Ordinal));
        fixture.Write(1, AnalysisModule.Second.Replace("p.Value += 2;", "p.Value += new int[4].Length;", StringComparison.Ordinal));
        var outcome = fixture.Compile(0, 1);
        Assert.Equal(OutcomeKind.Delta, outcome.Kind);
        Assert.Equal(2, outcome.Problems.Count(diagnostic => diagnostic.Id == "ARA0300"));
        var next = outcome.Next!;
        var methods = next.Trees.Values.SelectMany(tree => tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>())
            .Where(method => method.Identifier.ValueText is "First" or "Last" or "Generic").ToArray();
        var analyzer = new AnalyzerSet();
        var expected = Exact(analyzer.Analyze(next.Final, next.Trees.Values.ToArray(), "gameplay"));
        Assert.Equal(expected, Exact(analyzer.AnalyzeBodies(next.Final, methods, "gameplay")));
        Assert.Equal(expected, Exact(analyzer.AnalyzeBodies(next.Final, methods.Reverse().ToArray(), "gameplay")));
        Assert.False(next.AnalyzerClean);
    }

    internal static string[] Exact(IEnumerable<Diagnostic> diagnostics) => diagnostics.Select(diagnostic =>
        $"{diagnostic.Id}|{diagnostic.Severity}|{diagnostic.Location.SourceTree?.FilePath}|{diagnostic.Location.SourceSpan}|{diagnostic.GetMessage()}")
        .Order(StringComparer.Ordinal).ToArray();

    // Instantiated directly by the test; never discovered or shipped as a compiler extension.
#pragma warning disable RS1001
    sealed class FailingAnalyzer : DiagnosticAnalyzer
    {
        public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => new Ara.Analyzers.AllocationAnalyzer().SupportedDiagnostics;
        public override void Initialize(AnalysisContext context)
        {
            context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
            context.EnableConcurrentExecution();
            context.RegisterOperationAction(_ => throw new InvalidOperationException("injected analyzer failure"), OperationKind.Invocation);
        }
    }
#pragma warning restore RS1001
}

/// <summary>Direct session fixture so candidate proof and committed proof can be checked separately.</summary>
sealed class AnalysisModule : IDisposable, ICompileContext
{
    internal const string First = """
        namespace AnalysisCases;
        /// <summary>Probe data.</summary>
        [Component] public partial struct Probe { /// <summary>Value.</summary>
            public int Value;
        }
        [Simulation] public static partial class Logic {
            [Query] static void First(ref Probe p) { p.Value += 1; }
            [Query] static void Sibling(ref Probe p) { p.Value += 3; }
            static int Generic<T>() where T : struct { return 1; }
            static T GetComponent<T>() where T : struct => default;
        }
        """;
    internal const string Second = """
        namespace AnalysisCases;
        public static partial class Logic { [Query] static void Last(ref Probe p) { p.Value += 2; } }
        """;
    readonly string _directory = InstalledTools.Scratch("analyzer-bodies");
    internal readonly ModuleSession Module;
    internal readonly string[] Paths;

    internal AnalysisModule(IAnalysis? analysis = null)
    {
        Paths = [Path.Combine(_directory, "First.cs"), Path.Combine(_directory, "Second.cs")];
        Write(0, First);
        Write(1, Second);
        Module = new ModuleSession(0, "Game", _directory, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", analysis ?? new AnalyzerSet());
    }

    internal void Write(int file, string source) => File.WriteAllText(Paths[file], source);
    internal Outcome Compile(params int[] files) => Module.Compile(files.Select(index => Paths[index]), 0, this);
    internal void Start()
    {
        var initial = Module.Initial(this);
        Assert.Equal(OutcomeKind.Image, initial.Kind);
        Module.Commit(initial);
    }
    public bool Enc => true;
    public EncCapabilities Caps => EncCapabilities.BodiesOnly;
    public ModuleState? Dependency(string name) => null;
    public bool HasDependents(ModuleSession module) => false;
    public void Dispose() => Directory.Delete(_directory, recursive: true);
}

using Ara.Hosting;
using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Reflection;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.CompileServer.Tests;

/// <summary>An additive (L2) edit analyzes the touched files and the files that spell its new names, not the module.</summary>
public sealed class AdditiveAnalysisScopeTests
{
    const string A = """
        namespace Scope;
        /// <summary>Probe data.</summary>
        [Component] public partial struct Probe { /// <summary>Value.</summary>
            public int Value;
        }
        [Simulation] public static partial class Logic {
            [Query] static void Run(ref Probe p) { p.Value += 1; }
            // @insert
        }
        """;
    const string B = """
        namespace Scope;
        public static partial class Logic {
            [Query] static void Other(ref Probe p) { p.Value += Shared(); }
            static int Shared() => 2;
        }
        """;
    const string C = """
        namespace Scope;
        static class Unrelated { static int Free() => 3; }
        """;

    [Theory]
    [InlineData("static int Added() => 4;", "A")]
    [InlineData("static int Shared(long x) => 5;", "AB")]
    [InlineData("static int Free;", "AC")]
    [InlineData("const int Shared2 = 6;", "A")]
    // Names the language binds on its own, and types, are not found by spelling: the whole module.
    [InlineData("static int Count => 7;", "ABC")]
    [InlineData("static int GetEnumerator() => 8;", "ABC")]
    [InlineData("sealed class Extra { }", "ABC")]
    public void An_added_member_analyzes_its_file_and_the_files_that_spell_its_name(string insert, string expected)
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(A.Replace("// @insert", insert, StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Delta, outcome.Kind);
        Assert.Equal(AbiConst.ReloadL2, outcome.ReloadKind);
        Assert.Equal(expected, fixture.Visited());
        fixture.Module.Commit(outcome);
        Assert.True(fixture.Module.State!.AnalyzerClean);
    }

    [Fact]
    public void A_module_with_any_analyzer_diagnostic_is_analyzed_whole()
    {
        using var fixture = new ScopeModule(report: true);
        Assert.False(fixture.Module.State!.AnalyzerClean);
        var outcome = fixture.Compile(fixture.Edit(A.Replace("// @insert", "static int Added() => 4;", StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Delta, outcome.Kind);
        Assert.Equal("ABC", fixture.Visited());
    }

    [Fact]
    public void A_violation_in_an_added_member_is_reported_and_keeps_the_committed_state()
    {
        using var fixture = new ScopeModule(report: false, real: true);
        var good = fixture.Module.State;
        var outcome = fixture.Compile(fixture.Edit(A.Replace("// @insert", "static int Bad() => System.DateTime.UtcNow.Second;", StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Errors, outcome.Kind);
        Assert.Equal("ARA0200", Assert.Single(outcome.Problems).Id);
        Assert.Same(good, fixture.Module.State);

        var repaired = fixture.Compile(fixture.Edit(A.Replace("// @insert", "static int Good() => 9;", StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Delta, repaired.Kind);
        Assert.Empty(repaired.Problems);
    }

    /// <summary>Three files in one hot module; the probe analyzer records which files the analysis visited.</summary>
    sealed class ScopeModule : IDisposable, ICompileContext
    {
        readonly string _directory = InstalledTools.Scratch("additive-scope");
        readonly string[] _paths;
        readonly Probe? _probe;
        internal readonly ModuleSession Module;

        internal ScopeModule(bool report, bool real = false)
        {
            _paths = new[] { "A.cs", "B.cs", "C.cs" }.Select(name => Path.Combine(_directory, name)).ToArray();
            File.WriteAllText(_paths[0], A);
            File.WriteAllText(_paths[1], B);
            File.WriteAllText(_paths[2], C);
            var analysis = new AnalyzerSet();
            if (!real)
            {
                _probe = new Probe(report);
                // Inject the probe without exposing custom analyzer configuration in the product API.
                typeof(AnalyzerSet).GetField("_analyzers", BindingFlags.Instance | BindingFlags.NonPublic)!
                    .SetValue(analysis, ImmutableArray.Create<DiagnosticAnalyzer>(_probe));
            }
            Module = new ModuleSession(0, "Game", _directory, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", analysis);
            var initial = Module.Initial(this);
            Assert.Equal(OutcomeKind.Image, initial.Kind);
            Module.Commit(initial);
        }

        /// <summary>Writes the new text of file A and returns its path.</summary>
        internal string Edit(string text)
        {
            File.WriteAllText(_paths[0], text);
            return _paths[0];
        }

        internal Outcome Compile(string path)
        {
            _probe?.Seen.Clear();
            return Module.Compile(new[] { path }, 0, this);
        }

        /// <summary>The visited files as letters (A, B, C), in order.</summary>
        internal string Visited() => string.Concat(_probe!.Seen.Select(path => Path.GetFileNameWithoutExtension(path))
            .Where(name => name is "A" or "B" or "C").Order(StringComparer.Ordinal));

        public bool Enc => true;
        public EncCapabilities Caps => EncCapabilities.Net10;
        public ModuleState? Dependency(string name) => null;
        public bool HasDependents(ModuleSession module) => false;
        public void Dispose() => Directory.Delete(_directory, recursive: true);
    }

    // Instantiated directly by the test; never discovered or shipped as a compiler extension.
#pragma warning disable RS1001, RS2008
    sealed class Probe(bool report) : DiagnosticAnalyzer
    {
        static readonly DiagnosticDescriptor Note = new("ARA9001", "probe", "probe", "Test", DiagnosticSeverity.Info, true);
        internal readonly ConcurrentBag<string> Seen = [];
        public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics => [Note];
        public override void Initialize(AnalysisContext context)
        {
            context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
            context.EnableConcurrentExecution();
            context.RegisterSyntaxTreeAction(c =>
            {
                Seen.Add(c.Tree.FilePath);
                if (report) c.ReportDiagnostic(Diagnostic.Create(Note, Location.None));
            });
        }
    }
#pragma warning restore RS1001, RS2008
}

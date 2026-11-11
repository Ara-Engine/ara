using Ara.Hosting;
using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Reflection;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.CompileServer.Tests;

/// <summary>An edit that changes declarations (L2, or an L3 rebuild) analyzes the touched files and the files that spell a changed name, not the module.</summary>
public sealed class AnalysisScopeTests
{
    const string A = """
        namespace Scope;
        /// <summary>Probe data.</summary>
        [Component] public partial struct Probe { /// <summary>Value.</summary>
            public int Value;
        }
        [Simulation] public static partial class Logic {
            [Query] static void Run(ref Probe p) { p.Value += Shared(); }
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
    [InlineData("sealed class Extra { }", "A")]
    [InlineData("sealed class Shared3 { }", "A")]
    [InlineData("sealed class Free { }", "AC")]
    // Names the language binds on its own are not found by spelling: the whole module.
    [InlineData("static int Count => 7;", "ABC")]
    [InlineData("static int GetEnumerator() => 8;", "ABC")]
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

    [Fact]
    public void A_signature_change_analyzes_its_file_and_the_files_that_spell_the_name()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(1, B.Replace("static int Shared() => 2;", "static int Shared(int x = 0) => 2;", StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Image, outcome.Kind);
        Assert.Equal(AbiConst.ReloadL3, outcome.ReloadKind);
        Assert.Equal("AB", fixture.Visited());
        fixture.Module.Commit(outcome);
        Assert.True(fixture.Module.State!.AnalyzerClean);
    }

    [Fact]
    public void An_edit_that_needs_a_full_rebuild_only_for_a_closure_analyzes_just_its_file()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(2, C.Replace("static int Free() => 3;", "static int Free() { System.Func<int> f = () => 3; return f(); }", StringComparison.Ordinal)));
        Assert.Equal(AbiConst.ReloadL3, outcome.ReloadKind);
        Assert.Equal(Escalation.Closure, outcome.Escalation);
        Assert.Equal("C", fixture.Visited());
    }

    [Fact]
    public void A_new_struct_field_analyzes_only_the_files_that_spell_it()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(A.Replace("public int Value;", "public int Value;\n    /// <summary>Extra.</summary>\n    public int Extra;", StringComparison.Ordinal)));
        Assert.Equal(AbiConst.ReloadL3, outcome.ReloadKind);
        Assert.Equal("A", fixture.Visited());
    }

    [Fact]
    public void A_type_attribute_change_reaches_the_other_parts_of_the_type()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(A.Replace("[Simulation] public static partial class Logic", "public static partial class Logic", StringComparison.Ordinal)));
        Assert.Equal(AbiConst.ReloadL3, outcome.ReloadKind);
        Assert.Equal("AB", fixture.Visited());
    }

    [Fact]
    public void A_base_list_change_is_not_found_by_a_spelled_name_and_analyzes_the_whole_module()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(2, C.Replace("static class Unrelated { static int Free() => 3; }", "sealed class Unrelated : System.IDisposable { public void Dispose() { } }", StringComparison.Ordinal)));
        Assert.Equal(AbiConst.ReloadL3, outcome.ReloadKind);
        Assert.Equal("ABC", fixture.Visited());
    }

    [Fact]
    public void A_new_file_analyzes_itself_and_the_files_that_spell_its_types()
    {
        using var fixture = new ScopeModule(report: false);
        var outcome = fixture.Compile(fixture.Edit(3, "namespace Scope;\nstatic class Fresh { static int One() => 1; }"));
        Assert.Equal(AbiConst.ReloadL2, outcome.ReloadKind);
        Assert.Equal("D", fixture.Visited());
    }

    /// <summary>Declarations that no spelled name finds take the whole module (null), however small the edit.</summary>
    [Theory]
    [InlineData("class K { }", "class K { public static K operator +(K a, K b) => a; }", false)]
    [InlineData("class K { }", "class K { public int this[int i] => i; }", false)]
    [InlineData("class K { }", "class K { ~K() { } }", false)]
    [InlineData("class K { }", "class K : System.IDisposable { public void Dispose() { } }", false)]
    [InlineData("class K { }", "static class K { }", false)]
    [InlineData("class K<T> { }", "class K<T, U> { }", true)] // a different type: found where K is spelled
    [InlineData("class K<T> { }", "class K<T> where T : class { }", false)]
    [InlineData("class K { void M() { } }", "class K { void M() { var x = 1; } }", true)]
    [InlineData("class K { void M() { } }", "class K { void M(int x) { } }", true)]
    [InlineData("class K { }", "[System.Obsolete] class K { }", true)]
    [InlineData("class K { }", "class K { K(int x) { } }", true)]
    [InlineData("using System;\nclass K { }", "using System.Linq;\nclass K { }", true)]
    [InlineData("class K { }", "global using System.Text;\nclass K { }", false)]
    [InlineData("class K { }", "[assembly: System.Reflection.AssemblyTitle(\"x\")]\nclass K { }", false)]
    [InlineData("class K { void M() { } }", "class K { void GetEnumerator() { } }", false)]
    public void Scoping_by_spelled_name_gives_way_to_the_whole_module_when_a_name_cannot_find_the_change(string before, string after, bool scoped)
    {
        var old = ModuleCompiler.Parse("K.cs", before);
        var edited = ModuleCompiler.Parse("K.cs", after);
        var other = ModuleCompiler.Parse("O.cs", "class O { }");
        var trees = AnalysisScope.Trees([new FileChange("K.cs", old, edited)], [edited], [edited, other]);
        Assert.Equal(scoped, trees is not null);
        if (trees is not null) Assert.Equal([edited], trees);
    }

    /// <summary>
    /// Adding [Simulation] to a type makes the other parts' code subject to ARA0200. The scoped analysis must
    /// report exactly what a fresh full build of the same sources reports.
    /// </summary>
    [Fact]
    public void A_policy_change_in_one_part_finds_the_violation_in_another_exactly_as_a_fresh_build_does()
    {
        const string plain = "namespace Scope;\n/// <summary>Probe data.</summary>\n[Component] public partial struct Probe { /// <summary>Value.</summary>\n    public int Value; }\npublic static partial class Logic {\n    [Query] static void Run(ref Probe p) { p.Value += 1; }\n}";
        const string other = "namespace Scope;\npublic static partial class Logic { [Query] static void Other(ref Probe p) { p.Value += System.DateTime.UtcNow.Second; } }";
        using var fixture = new ScopeModule(report: false, real: true, a: plain, b: other);
        var live = fixture.Compile(fixture.Edit(plain.Replace("public static partial class Logic", "[Simulation] public static partial class Logic", StringComparison.Ordinal)));
        Assert.Equal(OutcomeKind.Errors, live.Kind);
        var fresh = fixture.Fresh();
        Assert.Equal(OutcomeKind.Errors, fresh.Kind);
        Assert.Contains(live.Problems, d => d.Id == "ARA0200" && d.Location.SourceTree!.FilePath.EndsWith("B.cs", StringComparison.Ordinal));
        Assert.Equal(AnalyzerBodyTests.Exact(fresh.Problems), AnalyzerBodyTests.Exact(live.Problems));
    }

    /// <summary>Three files in one hot module; the probe analyzer records which files the analysis visited.</summary>
    sealed class ScopeModule : IDisposable, ICompileContext
    {
        readonly string _directory = InstalledTools.Scratch("additive-scope");
        readonly string[] _paths;
        readonly Probe? _probe;
        internal readonly ModuleSession Module;

        internal ScopeModule(bool report, bool real = false, string a = A, string b = B)
        {
            _paths = new[] { "A.cs", "B.cs", "C.cs" }.Select(name => Path.Combine(_directory, name)).ToArray();
            File.WriteAllText(_paths[0], a);
            File.WriteAllText(_paths[1], b);
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
        internal string Edit(string text) => Edit(0, text);

        /// <summary>Writes the new text of file <paramref name="file"/> (0 = A, 1 = B, 2 = C, 3 = a new file D) and returns its path.</summary>
        internal string Edit(int file, string text)
        {
            var path = file < 3 ? _paths[file] : Path.Combine(_directory, "D.cs");
            File.WriteAllText(path, text);
            return path;
        }

        /// <summary>A fresh session over the current sources, analyzed in full: what the live compile must agree with.</summary>
        internal Outcome Fresh()
        {
            var module = new ModuleSession(1, "Game", _directory, true, [typeof(Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
            return module.Initial(this);
        }

        internal Outcome Compile(string path)
        {
            _probe?.Seen.Clear();
            return Module.Compile(new[] { path }, 0, this);
        }

        /// <summary>The visited files as letters (A, B, C), in order.</summary>
        internal string Visited() => string.Concat(_probe!.Seen.Select(path => Path.GetFileNameWithoutExtension(path))
            .Where(name => name is "A" or "B" or "C" or "D").Order(StringComparer.Ordinal));

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

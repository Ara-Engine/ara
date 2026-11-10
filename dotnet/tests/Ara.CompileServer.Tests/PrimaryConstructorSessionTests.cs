namespace Ara.CompileServer.Tests;

public sealed class PrimaryConstructorSessionTests
{
    [Theory]
    [InlineData("float", "seed", "ARA0703")]
    [InlineData("string", "seed.Length", "ARA0704")]
    public void Capturing_a_primary_parameter_from_another_partial_part_cannot_take_l1(string type, string value, string code)
    {
        using var fixture = new AnalysisModule();
        var (declaration, method) = Sources(type, value);
        fixture.Write(0, declaration);
        fixture.Write(1, method);
        fixture.Start();
        var committed = fixture.Module.State;
        fixture.Write(1, method.Replace("=> 1", "=> " + value, StringComparison.Ordinal));
        var changed = fixture.Compile(1);
        Assert.Equal(OutcomeKind.Errors, changed.Kind);
        Assert.Contains(changed.Problems, diagnostic => diagnostic.Id == code);
        Assert.Null(changed.Delta);
        Assert.Same(committed, fixture.Module.State);
        fixture.Write(1, method);
        var repaired = fixture.Compile(1);
        Assert.Equal(OutcomeKind.None, repaired.Kind);
        Assert.Empty(repaired.Problems);
    }

    [Fact]
    public void A_primary_constructor_elsewhere_does_not_block_an_unrelated_types_l1()
    {
        using var fixture = new AnalysisModule();
        var (declaration, method) = Sources("string", "seed.Length");
        const string logic = "[Simulation] public static partial class Logic { [Query] static void Run(ref State value) { value.Amount += 1; } }";
        fixture.Write(0, declaration);
        fixture.Write(1, method + "\n" + logic);
        fixture.Start();
        fixture.Write(1, method + "\n" + logic.Replace("+= 1", "+= 2", StringComparison.Ordinal));
        var changed = fixture.Compile(1);
        Assert.Equal(OutcomeKind.Delta, changed.Kind);
        Assert.Empty(changed.Problems);
    }

    [Theory]
    [InlineData("float", "seed")]
    [InlineData("string", "seed.Length")]
    public void An_added_method_capturing_a_primary_parameter_rebuilds_the_constructor(string type, string value)
    {
        using var fixture = new AnalysisModule();
        fixture.Write(0, $"namespace Primary; public partial class Config({type} seed) {{ public readonly float Amount = {value}; }}");
        const string method = "namespace Primary; public partial class Config { public float Read() => 1; }";
        fixture.Write(1, method);
        fixture.Start();
        fixture.Write(1, method.Replace("}", "public float ReadSeed() => " + value + "; }", StringComparison.Ordinal));
        var changed = fixture.Module.Compile([fixture.Paths[1]], 0, new AdditiveContext());
        Assert.Equal(OutcomeKind.Image, changed.Kind);
        Assert.Equal(Escalation.Constructor, changed.Escalation);
        Assert.DoesNotContain(changed.Problems, diagnostic => diagnostic.Severity == Microsoft.CodeAnalysis.DiagnosticSeverity.Error);
    }

    sealed class AdditiveContext : ICompileContext
    {
        public bool Enc => true;
        public EncCapabilities Caps => EncCapabilities.Net10;
        public ModuleState? Dependency(string name) => null;
        public bool HasDependents(ModuleSession module) => false;
    }

    [Theory]
    [InlineData("public const int Extra = 12;")]
    [InlineData("public static readonly int Extra;")]
    public void Ordinary_classes_keep_permitted_l2_fields(string declaration)
    {
        using var fixture = new AnalysisModule();
        fixture.Write(0, "namespace Primary; public partial class Config { }");
        const string method = "namespace Primary; public partial class Config { public float Read() => 1; }";
        fixture.Write(1, method);
        fixture.Start();
        fixture.Write(1, method.Replace("}", declaration + " }", StringComparison.Ordinal));
        var changed = fixture.Module.Compile([fixture.Paths[1]], 0, new AdditiveContext());
        Assert.Equal(OutcomeKind.Delta, changed.Kind);
        Assert.Equal(Ara.Hosting.AbiConst.ReloadL2, changed.ReloadKind);
        Assert.Empty(changed.Problems);
    }

    internal static (string Declaration, string Method) Sources(string type, string value) => ($$"""
        namespace Primary;
        /// <summary>State.</summary>
        [Component] public partial struct State({{type}} seed) {
            /// <summary>Amount.</summary>
            public float Amount = {{value}};
        }
        """, "namespace Primary; public partial struct State { public float Read() => 1; }");
}

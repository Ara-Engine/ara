using System.Collections.Immutable;
using Ara.Generators;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.Generators.Tests;

public class GeneratorTests
{
    const string Usings = "global using System;\nglobal using System.Numerics;\nglobal using Ara;\n";

    static readonly ImmutableArray<MetadataReference> References =
    [
        .. ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!).Split(Path.PathSeparator)
            .Where(p => Path.GetFileName(p).StartsWith("System.", StringComparison.Ordinal) || Path.GetFileName(p) is "netstandard.dll")
            .Select(p => (MetadataReference)MetadataReference.CreateFromFile(p)),
        MetadataReference.CreateFromFile(typeof(Ara.Behaviour).Assembly.Location),
    ];

    static readonly CSharpParseOptions Parse = new(LanguageVersion.Latest, DocumentationMode.Parse);

    sealed class Options(bool native) : AnalyzerConfigOptionsProvider
    {
        public override AnalyzerConfigOptions GlobalOptions { get; } = new Global(native);
        public override AnalyzerConfigOptions GetOptions(SyntaxTree tree) => GlobalOptions;
        public override AnalyzerConfigOptions GetOptions(AdditionalText textFile) => GlobalOptions;

        sealed class Global(bool native) : AnalyzerConfigOptions
        {
            public override bool TryGetValue(string key, out string value)
            {
                value = native ? "true" : "";
                return native && key == "build_property.AraNativeEntry";
            }
        }
    }

    /// <summary>Runs the generator; returns the output compilation, generator diagnostics and generated sources by hint.</summary>
    static (Compilation Out, ImmutableArray<Diagnostic> Diags, Dictionary<string, string> Sources) Run(string source, bool native = false)
    {
        var compilation = CSharpCompilation.Create("Game.Module",
            [CSharpSyntaxTree.ParseText(source, Parse, "Game.cs"), CSharpSyntaxTree.ParseText(Usings, Parse, "Usings.cs")],
            References, new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
        GeneratorDriver driver = CSharpGeneratorDriver.Create([new ModuleGenerator().AsSourceGenerator()], parseOptions: Parse, optionsProvider: new Options(native));
        driver = driver.RunGeneratorsAndUpdateCompilation(compilation, out var output, out var diags);
        var sources = driver.GetRunResult().Results.SelectMany(r => r.GeneratedSources).ToDictionary(s => s.HintName, s => s.SourceText.ToString());
        return (output, diags, sources);
    }

    const string Module = """
        namespace Game;

        /// <summary>Hit points.</summary>
        [Component]
        public partial struct Health
        {
            /// <summary>Current value.</summary>
            [Range(0, 100), Unit(Units.Percent), Replicate(Audience.Owner, Rollback = true)] public float Value;
            /// <summary>Who last hit it.</summary>
            public Entity LastHit;
        }

        /// <summary>Global tuning.</summary>
        [Resource] public partial struct Tuning { /// <summary>Regen per second.</summary>
            [Default(2)] public float Regen; }

        public static partial class Regen
        {
            [Query(Order = 3)]
            static void Run(ref Health h, in Tuning t, in Tick tick, Entity self, ref Rng rng) => h.Value += t.Regen * tick.Dt + rng.NextFloat() * 0;
        }

        /// <summary>A light that can be switched.</summary>
        public sealed partial class Lamp : Behaviour
        {
            /// <summary>On or off.</summary>
            [Property] public partial bool On { get; set; }
        }

        /// <summary>Switches a lamp.</summary>
        [Command("game.Toggle")]
        public static partial class Toggle
        {
            /// <summary>Flips it.</summary>
            /// <param name="ctx">Context.</param>
            /// <param name="lamp">The lamp to flip.</param>
            /// <param name="h">Its health.</param>
            public static CommandResult Execute(ref CommandContext ctx, Entity lamp, in Health h) => CommandResult.Ok;
        }
        """;

    [Fact]
    public void Generates_registration_that_compiles()
    {
        var (output, diags, sources) = Run(Module);
        Assert.Empty(diags);
        Assert.Empty(output.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error));
        var module = sources["AraModule.g.cs"];
        Assert.Contains("[assembly: global::Ara.Hosting.AraModule(\"Game.Module\", typeof(global::Ara.Generated.AraModule_Game_Module))]", module);
        Assert.Contains("""b.Type("Health", "{\"kind\":\"component\",\"doc\":\"Hit points.\",\"fields\":{\"value\":{\"type\":\"f32\",\"doc\":\"Current value.\",\"unit\":\"%\",\"replicate\":{\"audience\":\"owner\",\"rollback\":true},\"range\":[0,100]},\"lastHit\":{\"type\":\"entity\",\"doc\":\"Who last hit it.\"}}}");""", module);
        Assert.Contains("b.System(\"Regen\", &global::Game.Regen.__AraRun_Run, 3, [new global::Ara.Hosting.ColumnAccess(\"Health\", true)], [new global::Ara.Hosting.ColumnAccess(\"Tuning\", false)], [], [], false);", module);
        Assert.Contains("b.Behaviour(\"Lamp\", \"Lamp\", sizeof(global::Game.Lamp.Data), static () => new global::Game.Lamp());", module);
        Assert.Contains("b.Command(\"game.Toggle\", \"Switches a lamp.\", \"ToggleParams\", global::Game.Toggle.__AraParamsLayout()", module);
        Assert.Contains("[\"Health\"], [],", module); // reads Health, writes nothing
        Assert.Contains("public partial bool On { get => AraData<Data>().On; set => AraData<Data>().On = value; }", sources["Game.Lamp.behaviour.g.cs"]);
    }

    [Fact]
    public void Body_edits_leave_generated_code_unchanged()
    {
        // The compile server keeps an edit on the L1 path only if generated code is identical.
        var (_, _, before) = Run(Module);
        var (_, _, after) = Run(Module.Replace("rng.NextFloat() * 0", "rng.NextFloat() * 1"));
        Assert.Equal(before, after);
    }

    [Fact]
    public void Nested_defaults_keep_their_schema_json()
    {
        const string source = """
            namespace Game;
            /// <summary>A nested value.</summary>
            public struct Pair {
                /// <summary>The horizontal value.</summary>
                public int X;
            }
            /// <summary>A component with a nested default.</summary>
            [Component] public partial struct Data {
                /// <summary>Initial nested state.</summary>
                [Default(Json = "{\"x\":2}")] public Pair Nested;
            }
            """;
        var (output, diags, sources) = Run(source);
        Assert.Empty(diags.Where(d => d.Severity == DiagnosticSeverity.Error));
        Assert.Empty(output.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error));
        Assert.Contains("""\"default\":{\"x\":2}""", sources["AraModule.g.cs"]);
    }

    [Fact]
    public void Native_entry_is_emitted_on_request()
    {
        var (output, _, sources) = Run(Module, native: true);
        Assert.Contains("UnmanagedCallersOnly(EntryPoint = \"ara_script_init\")", sources["AraModule.g.cs"]);
        Assert.Contains(sources.Values, source => source.Contains("Host.TypeIndex("));
        Assert.Empty(output.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error));
    }

    [Theory]
    [InlineData("ARA0701", "/// <summary>X.</summary>\n[Component] public struct NotPartial { /// <summary>V.</summary>\n public int V; }")]
    [InlineData("ARA0703", "[Component] public partial struct Undocumented { public int V; }")]
    [InlineData("ARA0704", "/// <summary>X.</summary>\n[Component] public partial struct Named { /// <summary>N.</summary>\n public string N; }")]
    [InlineData("ARA0702", "/// <summary>X.</summary>\n[Component] public partial struct C { /// <summary>V.</summary>\n public int V; }\npublic static partial class S { [Query] static void Run(ref C c, string s) { } }")]
    [InlineData("ARA0702", "/// <summary>X.</summary>\n[Component] public partial struct T { }\n/// <summary>X.</summary>\n[Component] public partial struct C { /// <summary>V.</summary>\n public int V; }\npublic static partial class S { [Query] static void Run(in T t, ref C c) { } }")]
    [InlineData("ARA0705", "/// <summary>X.</summary>\n[Command] public static partial class Bad { public static void Execute() { } }")]
    [InlineData("ARA0706", "/// <summary>X.</summary>\npublic sealed partial class B : Behaviour { /// <summary>P.</summary>\n [Property] public bool P { get; set; } }")]
    public void Reports_invalid_declarations(string id, string source)
    {
        var (_, diags, sources) = Run(source);
        Assert.Contains(diags, d => d.Id == id);
        Assert.False(sources.ContainsKey("AraModule.g.cs"), "no registration for a module with errors");
    }
}

using Microsoft.CodeAnalysis;

namespace Ara.Generators.Tests;

public partial class GeneratorTests
{
    const string BatchData = """
        namespace Game;
        /// <summary>Authoritative values.</summary>
        [Component] public partial struct Count { /// <summary>Current value.</summary>
            public float Value; }
        /// <summary>A selection tag.</summary>
        [Component] public partial struct Selected { }
        /// <summary>Shared tuning.</summary>
        [Resource] public partial struct Tuning { /// <summary>Scale.</summary>
            public float Scale; }
        """;

    const string BatchSystem = """
        public static partial class Batch
        {
            [Query(Order = 4), With<Selected>]
            static void Run(Span<Count> values, ReadOnlySpan<Velocity> velocities,
                ReadOnlySpan<Entity> entities, ref Tuning tuning, in Tick tick, ref Commands commands)
            {
                for (var i = 0; i < values.Length; i++)
                    values[i].Value += velocities[i].Linear.X * tick.Dt;
                tuning.Scale += values.Length;
            }
        }
        """;

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Batch_query_calls_the_method_once_and_preserves_declared_access(bool native)
    {
        var (output, diagnostics, sources) = Run(BatchData + BatchSystem, native);
        Assert.Empty(diagnostics);
        Assert.Empty(output.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error));
        var wrapper = sources["Game.Batch.system.Run.g.cs"];
        Assert.DoesNotContain("for (int i", wrapper);
        Assert.DoesNotContain("Unsafe.Add", wrapper);
        Assert.Contains("new global::System.Span<global::Game.Count>(call->Columns[0], n)", wrapper);
        Assert.Contains("new global::System.ReadOnlySpan<global::Ara.Velocity>(call->Columns[1], n)", wrapper);
        Assert.Contains("new global::System.ReadOnlySpan<global::Ara.Entity>(call->Entities, n)", wrapper);
        Assert.Contains("ref r0, in *call->Tick, ref cmds", wrapper);
        Assert.Contains("[new global::Ara.Hosting.ColumnAccess(\"Count\", true), new global::Ara.Hosting.ColumnAccess(\"Velocity\", false)]", sources["AraModule.g.cs"]);
        Assert.Contains("[new global::Ara.Hosting.ColumnAccess(\"Tuning\", true)], [\"Selected\"], [], true)", sources["AraModule.g.cs"]);
    }

    [Fact]
    public void Batch_body_edits_leave_generated_output_identical()
    {
        var before = Run(BatchData + BatchSystem);
        var after = Run(BatchData + BatchSystem.Replace("velocities[i].Linear.X * tick.Dt", "velocities[i].Linear.X * tick.Dt + 1"));
        Assert.Empty(before.Diags);
        Assert.Empty(after.Diags);
        Assert.Equal(before.Sources, after.Sources);
    }

    [Fact]
    public void Readonly_batch_views_and_value_tick_compile_without_managed_allocations_in_the_wrapper()
    {
        var (output, diagnostics, sources) = Run(BatchData + """
            public static partial class Reading
            {
                [Query] static void Run(ReadOnlySpan<Count> values, Tick tick) { }
            }
            """);
        Assert.Empty(diagnostics);
        Assert.Empty(output.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error));
        Assert.Contains("Run(new global::System.ReadOnlySpan<global::Game.Count>(call->Columns[0], n), *call->Tick);", sources["Game.Reading.system.Run.g.cs"]);
        Assert.Contains("ColumnAccess(\"Count\", false)", sources["AraModule.g.cs"]);
    }

    [Theory]
    [InlineData("Span<int> values")]
    [InlineData("ref Span<Count> values")]
    [InlineData("in ReadOnlySpan<Count> values")]
    [InlineData("Span<Entity> entities, Span<Count> values")]
    [InlineData("Span<Count> values, ref Count scalar")]
    [InlineData("Span<Count> values, Entity scalar")]
    [InlineData("Span<Count> values, ref Rng rng")]
    [InlineData("Span<Selected> values")]
    [InlineData("Span<Count> values, ReadOnlySpan<Count> duplicate")]
    [InlineData("Span<Count> values, ReadOnlySpan<Tuning> resources")]
    [InlineData("Span<AudioSource> values")]
    public void Invalid_batch_shapes_report_a_generator_error(string parameters)
    {
        var (_, diagnostics, sources) = Run(BatchData + $$"""
            public static partial class Invalid
            {
                [Query] static void Run({{parameters}}) { }
            }
            """);
        Assert.Contains(diagnostics, d => d.Id == "ARA0702");
        Assert.False(sources.ContainsKey("AraModule.g.cs"));
    }
}

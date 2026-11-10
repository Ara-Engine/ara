namespace Ara.CompileServer.Tests;

public sealed class DocumentationOwnershipTests
{
    const string FieldDoc = "/// <summary>Value.</summary>";
    const string Helper = "static int Helper() => 7;";

    [Fact]
    public void Body_length_changes_do_not_move_a_later_documentations_syntax_attachment()
    {
        const string before = """
            public class Probe {
                static int Helper() { return 7; }
                /// <summary>Value.</summary>
                public int Value;
            }
            """;
        var after = before.Replace("return 7;", "var amount = 12345; return amount;", StringComparison.Ordinal);
        Assert.False(EditClassifier.DocsChanged([new("Probe.cs", ModuleCompiler.Parse("Probe.cs", before), ModuleCompiler.Parse("Probe.cs", after))]));
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Moving_a_field_doc_to_a_helper_cannot_bypass_required_documentation(bool bodyEdit)
    {
        using var fixture = new AnalysisModule();
        var before = AnalysisModule.First.Replace(FieldDoc, Helper + "\n" + FieldDoc, StringComparison.Ordinal);
        fixture.Write(0, before);
        fixture.Start();
        var committed = fixture.Module.State;
        var after = before.Replace(Helper + "\n" + FieldDoc, FieldDoc + "\n" + Helper, StringComparison.Ordinal);
        if (bodyEdit) after = after.Replace("p.Value += 1;", "p.Value += 2;", StringComparison.Ordinal);
        fixture.Write(0, after);
        var rejected = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Errors, rejected.Kind);
        Assert.Contains(rejected.Problems, diagnostic => diagnostic.Id == "ARA0703");
        Assert.Same(committed, fixture.Module.State);
        fixture.Write(0, before.Replace("p.Value += 1;", "p.Value += 3;", StringComparison.Ordinal));
        var repaired = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Delta, repaired.Kind);
        Assert.Empty(repaired.Problems);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Moving_helper_docs_onto_a_field_rebuilds_its_changed_schema(bool bodyEdit)
    {
        using var fixture = new AnalysisModule();
        const string helperDoc = "/// <summary>Helper.</summary>";
        var before = AnalysisModule.First.Replace(FieldDoc, helperDoc + "\n" + Helper + "\n" + FieldDoc, StringComparison.Ordinal);
        fixture.Write(0, before);
        fixture.Start();
        var old = fixture.Module.State!;
        var after = before.Replace(helperDoc + "\n" + Helper, Helper + "\n" + helperDoc, StringComparison.Ordinal);
        if (bodyEdit) after = after.Replace("p.Value += 1;", "p.Value += 2;", StringComparison.Ordinal);
        fixture.Write(0, after);
        var changed = fixture.Compile(0);
        Assert.Equal(OutcomeKind.Image, changed.Kind);
        Assert.Equal(Escalation.Generated, changed.Escalation);
        Assert.Empty(changed.Problems);
        var generated = string.Join("\n", changed.Next!.Generated.Values);
        Assert.Contains("Helper.", generated);
        Assert.DoesNotContain("Helper.", string.Join("\n", old.Generated.Values));
        fixture.Module.Commit(changed);
        Assert.Equal(old.Generation + 1, fixture.Module.State!.Generation);
    }
}

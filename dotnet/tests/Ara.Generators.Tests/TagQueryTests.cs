using Microsoft.CodeAnalysis;

namespace Ara.Generators.Tests;

public partial class GeneratorTests
{
    [Theory]
    [InlineData("", true)]
    [InlineData("public static readonly int Cached = 7;", true)]
    [InlineData("public const int Version = 1;", true)]
    [InlineData("/// <summary>Amount.</summary>\npublic int Amount;", false)]
    [InlineData("/// <summary>Amount.</summary>\nprivate int amount;", false)]
    public void Scalar_and_batch_tag_detection_uses_the_same_instance_field_rules(string members, bool tag)
    {
        foreach (var parameter in new[] { "ref Probe value", "System.Span<Probe> value" })
        {
            var result = Run($$"""
                namespace Game;
                /// <summary>Tag or data fixture.</summary>
                [Component] public partial struct Probe { {{members}} }
                public static partial class Update {
                    [Query] static void Run({{parameter}}) { }
                }
                """);
            Assert.Equal(tag, result.Diags.Any(diagnostic => diagnostic.Id == "ARA0702"));
            if (!tag)
            {
                Assert.Empty(result.Diags.Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error));
                Assert.Empty(result.Out.GetDiagnostics().Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error));
            }
        }
    }

    [Fact]
    public void Presence_check_keeps_implicit_fields_and_does_not_bypass_required_documentation()
    {
        foreach (var member in new[] { "public int Amount;", "public int Amount { get; set; }" })
        {
            var result = Run($$"""
                namespace Game;
                /// <summary>A data fixture.</summary>
                [Component] public partial struct Probe { {{member}} }
                public static partial class Update {
                    [Query] static void Run(ref Probe value) { }
                }
                """);
            Assert.DoesNotContain(result.Diags, diagnostic => diagnostic.Id == "ARA0702");
            Assert.Contains(result.Diags, diagnostic => diagnostic.Id == "ARA0703");
        }
    }
}

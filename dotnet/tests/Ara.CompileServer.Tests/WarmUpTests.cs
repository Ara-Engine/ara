namespace Ara.CompileServer.Tests;

public sealed class WarmUpTests
{
    /// <summary>The start-up warm-up must really exercise the generator and a debug-IL emit, or it warms nothing.</summary>
    /// <summary>A reference read that fails once (a scanner holding a file) must not fail every later compile in the process.</summary>
    [Fact]
    public void A_framework_reference_failure_is_not_cached()
    {
        var calls = 0;
        var shared = ModuleCompiler.FrameworkOnce(() =>
        {
            if (Interlocked.Increment(ref calls) == 1) throw new IOException("the reference file is locked (injected)");
            return [Microsoft.CodeAnalysis.MetadataReference.CreateFromFile(typeof(object).Assembly.Location)];
        });
        Assert.Throws<IOException>(() => shared.Value);
        Assert.Single(shared.Value);   // the next caller retries and succeeds
        Assert.Equal(2, calls);
        Assert.Single(shared.Value);   // and the success is kept
        Assert.Equal(2, calls);
    }

    [Theory]
    [InlineData(1, false)]
    [InlineData(2, false)]
    [InlineData(4, false)]
    [InlineData(7, false)]
    [InlineData(8, true)]
    [InlineData(16, true)]
    public void The_warm_up_only_runs_where_it_does_not_compete_with_the_first_compile(int cores, bool expected) =>
        Assert.Equal(expected, ModuleCompiler.ShouldWarmUp(cores));

    /// <summary>ARA_WARMUP=0/1 overrides the core rule in either direction; other values leave it alone.</summary>
    [Theory]
    [InlineData(16, "0", false)]
    [InlineData(2, "1", true)]
    [InlineData(16, "", true)]
    [InlineData(2, "yes", false)]
    [InlineData(16, null, true)]
    public void The_warm_up_setting_overrides_the_core_rule(int cores, string? setting, bool expected) =>
        Assert.Equal(expected, ModuleCompiler.ShouldWarmUp(cores, setting));

    [Fact]
    public void The_warm_up_compile_generates_and_emits()
    {
        Assert.Null(ModuleCompiler.WarmUp());
    }
}

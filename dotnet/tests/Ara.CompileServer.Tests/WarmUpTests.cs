namespace Ara.CompileServer.Tests;

public sealed class WarmUpTests
{
    /// <summary>The start-up warm-up must really exercise the generator and a debug-IL emit, or it warms nothing.</summary>
    [Fact]
    public void The_warm_up_compile_generates_and_emits()
    {
        Assert.True(ModuleCompiler.WarmUp());
    }
}

namespace Ara.CompileServer.Tests;

public sealed class StartupTraceTests
{
    /// <summary>The trace is off unless asked for, and then costs nothing; the counters it subtracts only grow.</summary>
    [Fact]
    public void The_trace_is_off_by_default_and_its_counters_only_grow()
    {
        Assert.False(StartupTrace.Enabled);
        var before = StartupTrace.Counters.Now();
        GC.Collect();
        var after = StartupTrace.Counters.Now();
        Assert.True(after.Gen0 > before.Gen0);
        Assert.True(after.Jit >= before.Jit);
        Assert.True(after.PauseMs >= before.PauseMs);
        StartupTrace.Mark("ignored while disabled"); // writes nothing
    }
}

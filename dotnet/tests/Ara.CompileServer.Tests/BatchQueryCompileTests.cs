using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class BatchQueryCompileTests
{
    [Fact]
    public void Batch_method_body_edits_keep_l1_and_preserve_the_last_good_code_after_errors()
    {
        using var modules = new Modules();
        modules.Add(0, "Game", """
            /// <summary>Authoritative values.</summary>
            [Component] public partial struct Count { /// <summary>Current value.</summary>
                public int Value; }
            public static partial class Counting
            {
                [Query] static void Run(Span<Count> counts)
                {
                    for (var i = 0; i < counts.Length; i++) counts[i].Value += 1;
                }
            }
            """);
        modules.Idle();
        Assert.Equal(AbiConst.ReloadInitial, modules.Sink.Next().Reload().Single().Kind);
        modules.Edit(0, "Game", "+= 1", "+= 2");
        Assert.Equal(AbiConst.ReloadL1, modules.Sink.Next().Delta().Kind);
        modules.Edit(0, "Game", "+= 2", "+= ;");
        Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
        modules.Edit(0, "Game", "+= ;", "+= 3");
        Assert.Equal(AbiConst.ReloadL1, modules.Sink.Next().Delta().Kind);
        Assert.True(modules.Sink.Empty);
    }
}

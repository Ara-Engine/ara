using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class RecoveryTests
{
    const string Core = "namespace Core; public static class Values { public static int Number() => 1; }";
    const string Game = "namespace Game; public static class Read { public static int Number() => Core.Values.Number(); }";

    [Fact]
    public void Recovery_rebuilds_current_disk_inputs_as_one_graph_or_retains_all_last_good_modules()
    {
        using var modules = new Modules();
        modules.Add(0, "Core", Core);
        modules.Idle();
        modules.Sink.Next().Reload();
        modules.Add(1, "Game", Game, references: ["Core"]);
        modules.Idle();
        modules.Sink.Next().Reload();
        // No watcher notification: recovery must read the current files, not cached syntax.
        File.WriteAllText(modules.File("Core", "Code.cs"), Core.Replace("=> 1", "=> 7"));
        modules.Server.Recover(71);
        modules.Idle();
        var sent = modules.Sink.Next();
        var batch = sent.Reload();
        Assert.Equal(new uint[] { 0, 1 }, batch.Select(e => e.Slot));
        Assert.All(batch, e => Assert.NotEmpty(e.Pe));
        Assert.Equal(71ul, BitConverter.ToUInt64(sent.Payload.AsSpan(sent.Payload.Length - 8)));
        var generation = batch[0].Generation;

        File.WriteAllText(modules.File("Game", "Code.cs"), Game.Replace("Core.Values.Number()", ";"));
        modules.Server.Recover(72);
        modules.Idle();
        Assert.True(modules.Sink.Next().Diagnostics().Errors > 0);
        var failure = modules.Sink.Next();
        while (failure.Kind == MessageKind.Diagnostics) {
            Assert.True(failure.Diagnostics().Errors > 0);
            failure = modules.Sink.Next();
        }
        Assert.Equal(MessageKind.RecoveryFinished, failure.Kind);
        var reader = new Protocol.Reader(failure.Payload);
        Assert.Equal(72ul, reader.U64());
        Assert.Equal("failed", reader.Str());
        Assert.True(modules.Sink.Empty);

        File.WriteAllText(modules.File("Game", "Code.cs"), Game);
        modules.Server.Recover(73);
        modules.Idle();
        var fixedBatch = modules.Sink.Next().Reload();
        Assert.Equal(generation + 1, fixedBatch[0].Generation);
    }
}

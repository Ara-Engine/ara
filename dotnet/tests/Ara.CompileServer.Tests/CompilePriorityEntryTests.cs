using System.Buffers.Binary;
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class CompilePriorityEntryTests
{
    [Fact]
    public async Task The_real_pipe_entry_preserves_inherited_nice_through_startup_human_and_agent_compiles()
    {
        if (!OperatingSystem.IsMacOS()) return;
        var root = InstalledTools.Scratch("priority-entry");
        var source = Path.Combine(root, "Rules.cs");
        File.WriteAllText(source, "public static class Rules { public static int Value() => 1; }");
        var dll = Path.Combine(InstalledTools.Repo, "dotnet", "artifacts", "bin", "Ara.CompileServer", "release", "Ara.CompileServer.dll");
        var name = "ara-p-" + Guid.NewGuid().ToString("N")[..16];
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        using var pipe = new NamedPipeServerStream(name, PipeDirection.InOut, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
        var start = new ProcessStartInfo("dotnet") { RedirectStandardError = true, RedirectStandardOutput = true, UseShellExecute = false };
        foreach (var arg in new[] { "exec", dll, "--pipe", name, "--parent", System.Environment.ProcessId.ToString() }) start.ArgumentList.Add(arg);
        var inherited = Nice(0);
        using var process = Process.Start(start) ?? throw new InvalidOperationException("compile server did not start");
        var stderr = process.StandardError.ReadToEndAsync(deadline.Token);
        var stdout = process.StandardOutput.ReadToEndAsync(deadline.Token);
        try
        {
            await pipe.WaitForConnectionAsync(deadline.Token);
            // Startup has already applied its idle policy before connecting.
            Assert.Equal(inherited, Nice(process.Id));
            Protocol.Send(pipe, MessageKind.Hello, new Protocol.Writer().U32(Protocol.Version).U32((uint)System.Environment.ProcessId)
                .Bool(true).Str("Baseline").ToArray());
            Protocol.Send(pipe, MessageKind.Watch, new Protocol.Writer().U32(0).Str("Game").Str(root).Bool(true)
                .Strs([typeof(Behaviour).Assembly.Location]).Strs([]).Str("gameplay").Bool(false).ToArray());
            await Complete(MessageKind.Reload);
            Assert.Equal(inherited, Nice(process.Id));
            foreach (var (value, origin) in new[] { (2, AbiConst.OriginHuman), (3, AbiConst.OriginAgent), (4, AbiConst.OriginHuman) })
            {
                File.WriteAllText(source, $"public static class Rules {{ public static int Value() => {value}; }}");
                Protocol.Send(pipe, MessageKind.Change, new Protocol.Writer().U32(0).Strs([source]).U32(origin).ToArray());
                await Complete(MessageKind.Delta);
                Assert.Equal(inherited, Nice(process.Id));
            }
            Protocol.Send(pipe, MessageKind.Shutdown, []);
            await process.WaitForExitAsync(deadline.Token);
            Assert.Equal(0, process.ExitCode);
            Assert.Empty(await stdout);
            Assert.DoesNotContain("priority hint failed", await stderr, StringComparison.Ordinal);
            Assert.Equal(inherited, Nice(0));
        }
        finally
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
                await process.WaitForExitAsync();
            }
        }

        async Task Complete(MessageKind expected)
        {
            var received = false;
            while (true)
            {
                var header = new byte[5];
                await pipe.ReadExactlyAsync(header, deadline.Token);
                var payload = new byte[checked((int)BinaryPrimitives.ReadUInt32LittleEndian(header) - 1)];
                await pipe.ReadExactlyAsync(payload, deadline.Token);
                var kind = (MessageKind)header[4];
                if (kind == MessageKind.Diagnostics)
                {
                    var reader = new Protocol.Reader(payload);
                    reader.U32();
                    Assert.True(reader.U32() == 0, reader.Str());
                }
                if (kind == MessageKind.Log)
                {
                    var reader = new Protocol.Reader(payload);
                    reader.U32();
                    Assert.DoesNotContain("priority hint failed", reader.Str(), StringComparison.Ordinal);
                }
                received |= kind == expected;
                if (received && kind == MessageKind.CompileState && !new Protocol.Reader(payload).Bool()) return;
            }
        }
    }

    static int Nice(int pid)
    {
        Marshal.SetLastPInvokeError(0);
        var priority = Native.GetPriority(0, pid);
        Assert.True(priority != -1 || Marshal.GetLastPInvokeError() == 0, "cannot read child priority");
        return priority;
    }

    static class Native
    {
        [DllImport("/usr/lib/libSystem.B.dylib", EntryPoint = "getpriority", SetLastError = true)]
        internal static extern int GetPriority(int which, int who);
    }
}

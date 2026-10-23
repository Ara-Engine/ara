using System.Diagnostics;
using System.IO.Pipes;
using System.Reflection.Metadata;
using System.Runtime.InteropServices;

namespace Ara.Hosting;

/// <summary>
/// The game process's side of the compile server: starts <c>Ara.CompileServer</c> as a
/// child process, serves it one duplex named pipe, and turns its messages into staged
/// updates on a background thread. Roslyn (and its GC pauses) never run in this process
/// (decision A13).
/// </summary>
internal sealed class CompileClient : IDisposable
{
    readonly NamedPipeServerStream _pipe;
    readonly Process _server;
    readonly Thread _reader;
    readonly DevModuleLoader _loader;
    volatile bool _closing;

    /// <summary>Starts the server at <paramref name="serverDll"/> and connects to it.</summary>
    internal CompileClient(string serverDll, DevModuleLoader loader)
    {
        _loader = loader;
        var name = $"ara-compile-{Environment.ProcessId}-{Guid.NewGuid():N}";
        _pipe = new NamedPipeServerStream(name, PipeDirection.InOut, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
        var appHost = Path.ChangeExtension(serverDll, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : null);
        var psi = File.Exists(appHost)
            ? new ProcessStartInfo(appHost)
            : new ProcessStartInfo("dotnet") { ArgumentList = { "exec", serverDll } };
        foreach (var arg in new[] { "--pipe", name, "--parent", Environment.ProcessId.ToString() }) psi.ArgumentList.Add(arg);
        psi.UseShellExecute = false;
        psi.RedirectStandardOutput = false;
        psi.RedirectStandardError = false;
        // The server must not inherit EnC mode: it never applies deltas to itself.
        psi.Environment.Remove("DOTNET_MODIFIABLE_ASSEMBLIES");
        _server = Process.Start(psi) ?? throw new InvalidOperationException($"cannot start `{serverDll}`");
        var connect = _pipe.WaitForConnectionAsync();
        if (!connect.Wait(TimeSpan.FromSeconds(30)))
            throw new TimeoutException("the compile server did not connect within 30 s");
        Protocol.Send(_pipe, MessageKind.Hello, new Protocol.Writer()
            .U32(Protocol.Version).U32((uint)Environment.ProcessId).Bool(MetadataUpdater.IsSupported).ToArray());
        _reader = new Thread(ReadLoop) { Name = "ara-compile-client", IsBackground = true };
        _reader.Start();
    }

    /// <summary>Asks the server to compile and watch a module folder.</summary>
    internal void Watch(uint slot, string name, string dir, bool hot) =>
        Protocol.Send(_pipe, MessageKind.Watch, new Protocol.Writer()
            .U32(slot).Str(name).Str(dir).Bool(hot)
            .Strs([typeof(ScriptRuntime).Assembly.Location])
            .ToArray());

    /// <summary>Asks for a hot (Debug IL, EnC) or cold (optimized) rebuild.</summary>
    internal void SetHeat(uint slot, bool hot) =>
        Protocol.Send(_pipe, MessageKind.SetHeat, new Protocol.Writer().U32(slot).Bool(hot).ToArray());

    /// <summary>Asks for a full image (a delta could not be applied).</summary>
    internal void Rebuild(uint slot, uint escalation)
    {
        try { Protocol.Send(_pipe, MessageKind.Rebuild, new Protocol.Writer().U32(slot).U32(escalation).ToArray()); }
        catch (IOException) { /* the server is gone; ReadLoop reports it */ }
    }

    void ReadLoop()
    {
        try
        {
            while (Protocol.Receive(_pipe) is { } msg)
            {
                var r = new Protocol.Reader(msg.Payload);
                switch (msg.Kind)
                {
                    case MessageKind.CompileState:
                        Host.CompileState(r.Bool());
                        break;
                    case MessageKind.Image:
                        _loader.OnImage(r.U32(), r.U32(), r.Bool(), r.U32(), r.U32(), r.U64(), r.Bytes(), r.Bytes());
                        break;
                    case MessageKind.Delta:
                        _loader.OnDelta(r.U32(), r.U32(), r.U32(), r.U64(), r.Bytes(), r.Bytes(), r.Bytes());
                        break;
                    case MessageKind.Diagnostics:
                        _loader.OnDiagnostics(r.U32(), r.U32(), r.Str(), r.U64());
                        break;
                    case MessageKind.Log:
                        Host.Log(r.U32(), "[compile server] " + r.Str());
                        break;
                }
            }
        }
        catch (Exception e) when (!_closing)
        {
            Diag.Report(AraCodes.CompileServer, "error", $"compile server connection failed: {e.Message}");
        }
        finally
        {
            Host.CompileState(false);
        }
        if (!_closing) Diag.Report(AraCodes.CompileServer, "error", "the compile server exited; hot reload is off until the host restarts");
    }

    /// <summary>Stops the server and the reader thread.</summary>
    public void Dispose()
    {
        _closing = true;
        try { Protocol.Send(_pipe, MessageKind.Shutdown, []); } catch (IOException) { }
        if (!_server.WaitForExit(2000))
        {
            try { _server.Kill(entireProcessTree: true); } catch (InvalidOperationException) { }
        }
        _pipe.Dispose();
        _reader.Join(2000);
        _server.Dispose();
    }
}

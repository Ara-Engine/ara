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

    /// <summary>Opaque lifetime for this loader-owned producer connection.</summary>
    internal readonly string DiagnosticInstance = Guid.NewGuid().ToString("N");
    /// <summary>Ordered connection replacement within the owning loader.</summary>
    internal readonly ulong DiagnosticBinding;
    internal bool Connected => !_closing && !_server.HasExited;

    /// <summary>Starts the server at <paramref name="serverDll"/> and connects to it.</summary>
    internal CompileClient(string serverDll, DevModuleLoader loader, ulong diagnosticBinding)
    {
        DiagnosticBinding = diagnosticBinding;
        _loader = loader;
        // Unix pipes are domain sockets, by default under $TMPDIR; macOS's long $TMPDIR
        // exceeds the 104-byte socket path limit, so other platforms use a short rooted
        // path, which .NET takes verbatim on both ends.
        var name = OperatingSystem.IsWindows()
            ? $"ara-compile-{System.Environment.ProcessId}-{Guid.NewGuid():N}"
            : $"/tmp/ara-{System.Environment.ProcessId}-{Guid.NewGuid():N}";
        _pipe = new NamedPipeServerStream(name, PipeDirection.InOut, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
        var appHost = Path.ChangeExtension(serverDll, RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".exe" : null);
        var psi = File.Exists(appHost)
            ? new ProcessStartInfo(appHost)
            : new ProcessStartInfo("dotnet") { ArgumentList = { "exec", serverDll } };
        foreach (var arg in new[] { "--pipe", name, "--parent", System.Environment.ProcessId.ToString() }) psi.ArgumentList.Add(arg);
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
            .U32(Protocol.Version).U32((uint)System.Environment.ProcessId).Bool(MetadataUpdater.IsSupported).Str(Capabilities()).ToArray());
        _reader = new Thread(ReadLoop) { Name = "ara-compile-client", IsBackground = true };
        _reader.Start();
    }

    /// <summary>Asks the server to compile and watch a module folder (after the modules it references).</summary>
    internal void Watch(uint slot, string name, string dir, bool hot, IReadOnlyCollection<string> references, string profile, bool watch,
        bool initial = true, uint generation = 0) =>
        Protocol.Send(_pipe, MessageKind.Watch, new Protocol.Writer()
            .U32(slot).Str(name).Str(dir).Bool(hot)
            .Strs([typeof(ScriptRuntime).Assembly.Location])
            .Strs(references).Str(profile).Bool(watch).Bool(initial).U32(generation)
            .ToArray());

    internal void Recover(ulong ticket) =>
        Protocol.Send(_pipe, MessageKind.Recover, new Protocol.Writer().U64(ticket).ToArray());

    /// <summary>Asks for a compile of changed files; agent edits are coalesced and run behind human ones.</summary>
    internal void Change(uint slot, IReadOnlyCollection<string> paths, uint origin, string? claim = null)
    {
        var payload = new Protocol.Writer().U32(slot).Strs(paths).U32(origin);
        if (claim is not null) payload.Str(claim);
        try { Protocol.Send(_pipe, MessageKind.Change, payload.ToArray()); }
        catch (IOException) { /* the server is gone; ReadLoop reports it */ }
    }

    /// <summary>Asks for a hot (Debug IL, EnC) or cold (optimized) rebuild.</summary>
    internal void SetHeat(uint slot, bool hot) =>
        Protocol.Send(_pipe, MessageKind.SetHeat, new Protocol.Writer().U32(slot).Bool(hot).ToArray());

    /// <summary>Asks for a full image (a delta could not be applied).</summary>
    internal void Rebuild(uint slot, uint escalation)
    {
        try { Protocol.Send(_pipe, MessageKind.Rebuild, new Protocol.Writer().U32(slot).U32(escalation).ToArray()); }
        catch (IOException) { /* the server is gone; ReadLoop reports it */ }
    }

    /// <summary>
    /// The runtime's EnC capabilities (what <c>dotnet watch</c> asks for): they decide which
    /// additive edits can take L2. Empty when EnC is off.
    /// </summary>
    static string Capabilities()
    {
        if (!MetadataUpdater.IsSupported) return "";
        try
        {
            return typeof(MetadataUpdater).GetMethod("GetCapabilities", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static)
                ?.Invoke(null, null) as string ?? "";
        }
        catch (Exception) { return ""; }
    }

    void ReadLoop()
    {
        using var binding = _loader.Activate();
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
                    case MessageKind.Reload:
                        _loader.OnReload(r);
                        break;
                    case MessageKind.Delta:
                        _loader.OnDelta(r);
                        break;
                    case MessageKind.Diagnostics:
                        _loader.OnDiagnostics(this, r);
                        break;
                    case MessageKind.RecoveryFinished:
                        _loader.CompleteRecovery(r.U64(), r.Str(), r.Strs());
                        break;
                    case MessageKind.Restart:
                        _loader.OnRestart(r);
                        break;
                    case MessageKind.Log:
                        Host.Log(r.U32(), "[compile server] " + r.Str());
                        break;
                }
            }
        }
        catch (Exception e)
        {
            if (!_closing) Diag.ReportException(AraCodes.CompileServer, "error", $"compile server connection failed: {e.Message}", e);
        }
        finally
        {
            Host.CompileState(false);
        }
        if (!_closing)
        {
            Diag.Report(AraCodes.CompileServer, "error", "the compile server exited; hot reload is off until the host restarts");
            _loader.RecoveryDisconnected(this);
        }
    }

    /// <summary>Stops the server and the reader thread.</summary>
    public void Dispose()
    {
        _closing = true;
        try { Protocol.Send(_pipe, MessageKind.Shutdown, []); } catch (IOException) { }
        if (!_server.WaitForExit(2000))
        {
            try { _server.Kill(); } catch (InvalidOperationException) { }
        }
        _pipe.Dispose();
        _reader.Join(2000);
        _server.Dispose();
    }
}

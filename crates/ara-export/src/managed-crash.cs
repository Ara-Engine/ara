// Compiled into exported gameplay modules; no gameplay state or host ABI changes.
using System;
using System.IO;
using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;

namespace Ara.Shipping;

internal static class CrashCapture
{
    // A module initializer also runs for NativeAOT shared libraries.
    [ModuleInitializer]
    [System.Diagnostics.CodeAnalysis.SuppressMessage("Usage", "CA2255", Justification = "Export-generated application infrastructure must attach before any gameplay entry point, including NativeAOT shared-library loading.")]
    internal static void Install()
    {
        AppDomain.CurrentDomain.UnhandledException += OnUnhandled;
        if (Array.IndexOf(Environment.GetCommandLineArgs(), "--crash-probe") >= 0 &&
            Array.IndexOf(Environment.GetCommandLineArgs(), "managed") >= 0)
            new System.Threading.Thread(ThrowProbe) { IsBackground = true }.Start();
    }

    private static void ThrowProbe() => throw new InvalidOperationException("Ara managed unhandled exception probe");

    private static void OnUnhandled(object sender, UnhandledExceptionEventArgs args)
    {
        try
        {
            var executable = Environment.ProcessPath ?? throw new IOException("Process path unavailable");
            using var config = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(Path.GetDirectoryName(executable)!, "ara-crash.json")));
            var buildId = config.RootElement.GetProperty("buildId").GetString() ?? "unknown";
            if (buildId.Length != 64 || Array.Exists(buildId.ToCharArray(), c => !Uri.IsHexDigit(c))) return;
            var state = OperatingSystem.IsWindows()
                ? Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData)
                : Environment.GetEnvironmentVariable("XDG_STATE_HOME") ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".local", "state");
            var directory = Environment.GetEnvironmentVariable("ARA_CRASH_DIR") ?? Path.Combine(state, "Ara", "crashes", buildId);
            Directory.CreateDirectory(directory);
            var exception = args.ExceptionObject as Exception;
            var message = exception?.Message ?? args.ExceptionObject.ToString() ?? "Unhandled managed exception";
            var stack = exception?.ToString() ?? message;
            var path = Path.Combine(directory, $"{Environment.ProcessId}-{Guid.NewGuid():N}.managed");
            using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read);
            using var writer = new BinaryWriter(stream, Encoding.UTF8, leaveOpen: true);
            writer.Write(Encoding.ASCII.GetBytes("ARAMCS1\0"));
            // Fixed little-endian lengths; Rust applies the canonical JSON writer
            // during symbolization, so this boundary never invents JSON ordering.
            foreach (var text in new[] { buildId, message, stack })
            {
                var bytes = Encoding.UTF8.GetBytes(text);
                if (bytes.Length > 1024 * 1024) return;
                writer.Write((uint)bytes.Length);
                writer.Write(bytes);
            }
            writer.Flush();
            stream.Flush(flushToDisk: true);
        }
        catch { /* A failure while recording must not replace the original exception. */ }
    }
}

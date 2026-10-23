using System.Diagnostics;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>
/// Builds structured diagnostics in the <c>ara-diag</c> JSON shape
/// (<c>{ code, severity, message, location: { file, line, column } }</c>) and sends them
/// to the host. Codes come from <see cref="AraCodes"/>.
/// </summary>
public static class Diag
{
    /// <summary>Reports one diagnostic. Never throws.</summary>
    public static void Report(string code, string severity, string message, string? file = null, int line = 0, int column = 0)
    {
        try { Host.Diagnostic(ToJson(code, severity, message, file, line, column)); }
        catch { /* reporting must never throw across the boundary */ }
    }

    /// <summary>Renders one diagnostic as UTF-8 JSON.</summary>
    public static byte[] ToJson(string code, string severity, string message, string? file = null, int line = 0, int column = 0)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms))
        {
            w.WriteStartObject();
            w.WriteString("code", code);
            w.WriteString("severity", severity);
            w.WriteString("message", message);
            if (file is not null || line > 0)
            {
                w.WriteStartObject("location");
                if (file is not null) w.WriteString("file", file);
                if (line > 0) w.WriteNumber("line", line);
                if (column > 0) w.WriteNumber("column", column);
                w.WriteEndObject();
            }
            w.WriteEndObject();
        }
        return ms.ToArray();
    }

    /// <summary>
    /// Reports a fault in user code: the exception type, message and the first source
    /// location of the stack (when PDBs are loaded), with the "disabled until next reload" note.
    /// </summary>
    internal static void Fault(string code, string what, Exception e)
    {
        string? file = null;
        int line = 0, column = 0;
        try
        {
            foreach (var frame in new StackTrace(e, fNeedFileInfo: true).GetFrames())
            {
                if (frame.GetFileName() is { } f)
                {
                    (file, line, column) = (f, frame.GetFileLineNumber(), frame.GetFileColumnNumber());
                    break;
                }
            }
        }
        catch { /* best effort */ }
        Report(code, "error", $"{what} threw {e.GetType().Name}: {e.Message} (disabled until the next reload)", file, line, column);
    }
}

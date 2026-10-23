using System.Text.Json;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer;

/// <summary>
/// Maps Roslyn diagnostics to the <c>ara-diag</c> JSON shape. Ara analyzer and generator
/// diagnostics keep their own <c>ARA0001</c>-<c>ARA0899</c> ids; C# compiler errors become
/// <c>ARA0900</c> with the <c>CS</c> id in the message. Locations are absolute file paths
/// with 1-based line and column.
/// </summary>
internal static class Diagnostics
{
    /// <summary><c>ARA0900</c>: a C# compile error.</summary>
    internal const string CompileError = "ARA0900";
    /// <summary><c>ARA0901</c>: a C# compile warning.</summary>
    internal const string CompileWarning = "ARA0901";
    /// <summary><c>ARA0902</c>: a module past its soft size cap.</summary>
    internal const string ModuleLarge = "ARA0902";
    /// <summary><c>ARA0903</c>: a module past 8k lines.</summary>
    internal const string ModuleTooLarge = "ARA0903";

    /// <summary>True for diagnostics worth sending: errors, and Ara-coded warnings and infos.</summary>
    internal static bool Reportable(Diagnostic d) =>
        d.Severity == DiagnosticSeverity.Error
        || (d.Id.StartsWith("ARA", StringComparison.Ordinal) && d.Severity != DiagnosticSeverity.Hidden);

    /// <summary>Writes one Roslyn diagnostic.</summary>
    internal static void Write(Utf8JsonWriter w, Diagnostic d)
    {
        var ara = d.Id.StartsWith("ARA", StringComparison.Ordinal);
        var code = ara ? d.Id : d.Severity == DiagnosticSeverity.Error ? CompileError : CompileWarning;
        var message = ara ? d.GetMessage() : $"{d.Id}: {d.GetMessage()}";
        var span = d.Location.GetMappedLineSpan();
        Write(w, code, Severity(d.Severity), message, span.IsValid ? span.Path : null,
            span.IsValid ? span.StartLinePosition.Line + 1 : 0, span.IsValid ? span.StartLinePosition.Character + 1 : 0);
    }

    /// <summary>Writes one diagnostic from parts.</summary>
    internal static void Write(Utf8JsonWriter w, string code, string severity, string message, string? file = null, int line = 0, int column = 0)
    {
        w.WriteStartObject();
        w.WriteString("code", code);
        w.WriteString("severity", severity);
        w.WriteString("message", message);
        if (file is not null)
        {
            w.WriteStartObject("location");
            w.WriteString("file", file.Replace('\\', '/'));
            if (line > 0) w.WriteNumber("line", line);
            if (column > 0) w.WriteNumber("column", column);
            w.WriteEndObject();
        }
        w.WriteEndObject();
    }

    static string Severity(DiagnosticSeverity s) => s switch
    {
        DiagnosticSeverity.Error => "error",
        DiagnosticSeverity.Warning => "warning",
        _ => "info",
    };

    /// <summary>A JSON array of diagnostics.</summary>
    internal static string Array(Action<Utf8JsonWriter> body)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms))
        {
            w.WriteStartArray();
            body(w);
            w.WriteEndArray();
        }
        return System.Text.Encoding.UTF8.GetString(ms.ToArray());
    }
}

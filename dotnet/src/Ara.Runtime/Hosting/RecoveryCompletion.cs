using System.Text.Json;

namespace Ara.Hosting;

/// <summary>Private completion marker, observed only after the owning tick boundary applies.</summary>
internal sealed class RecoveryCompletion(ulong ticket, string outcome, string[] modules) : PendingUpdate
{
    internal void Report()
    {
        using var bytes = new MemoryStream();
        using (var writer = new Utf8JsonWriter(bytes))
        {
            writer.WriteStartObject();
            writer.WriteString("code", "ARA0935");
            writer.WriteString("severity", outcome == "failed" ? "error" : "info");
            writer.WriteString("message", $"Script recovery {outcome}");
            writer.WriteStartObject("data");
            writer.WriteStartObject("recovery");
            writer.WriteNumber("schema", 1);
            writer.WriteNumber("ticket", ticket);
            writer.WriteString("outcome", outcome);
            writer.WriteStartArray("modules");
            foreach (var module in modules) writer.WriteStringValue(module);
            writer.WriteEndArray();
            writer.WriteEndObject();
            writer.WriteEndObject();
            writer.WriteEndObject();
        }
        Host.Diagnostic(bytes.ToArray());
    }
}

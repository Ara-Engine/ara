using System.Diagnostics;
using System.Diagnostics.CodeAnalysis;
using System.Text;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>Bounded fault metadata on the original diagnostic, without compilation, transport waits or user ToString calls.</summary>
internal static class ExceptionDetails
{
    const int MaxFrames = 64, MaxDepth = 8, MaxNodes = 16, MaxStringBytes = 8192;

    sealed class Budget { internal int Frames = MaxFrames, Nodes = MaxNodes; }
    sealed record Frame(string Method, string? File, int Line, int Column);
    sealed class Node
    {
        internal string Type = "", Message = "";
        internal readonly List<Frame> Frames = [];
        internal readonly List<Node> Inners = [];
        internal bool Truncated;

        internal void Write(Utf8JsonWriter writer)
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WriteString("type", Type);
            writer.WriteString("message", Message);
            writer.WriteStartArray("frames");
            foreach (var frame in Frames)
            {
                writer.WriteStartObject();
                writer.WriteString("method", frame.Method);
                writer.WriteString("file", frame.File);
                if (frame.Line > 0) writer.WriteNumber("line", frame.Line); else writer.WriteNull("line");
                if (frame.Column > 0) writer.WriteNumber("column", frame.Column); else writer.WriteNull("column");
                writer.WriteEndObject();
            }
            writer.WriteEndArray();
            writer.WriteStartArray("inners");
            foreach (var inner in Inners) inner.Write(writer);
            writer.WriteEndArray();
            writer.WriteBoolean("truncated", Truncated);
            writer.WriteEndObject();
        }
    }

    /// <summary>Adds the payload without replacing existing scope, subjects, message or location.</summary>
    internal static byte[] Attach(ReadOnlySpan<byte> diagnostic, Exception error, string? module = null)
    {
        var node = Capture(error, new(), 0);
        using var original = JsonDocument.Parse(diagnostic.ToArray());
        using var bytes = new MemoryStream();
        using (var writer = new Utf8JsonWriter(bytes))
        {
            writer.WriteStartObject();
            foreach (var property in original.RootElement.EnumerateObject())
                if (property.Name != "data") property.WriteTo(writer);
            writer.WriteStartObject("data");
            if (original.RootElement.TryGetProperty("data", out var data) && data.ValueKind == JsonValueKind.Object)
            {
                foreach (var property in data.EnumerateObject())
                    if (property.Name != "exception" && (module is null || property.Name != "module")) property.WriteTo(writer);
            }
            else if (data.ValueKind is not (JsonValueKind.Undefined or JsonValueKind.Null))
            {
                writer.WritePropertyName("original");
                data.WriteTo(writer);
            }
            if (module is not null) writer.WriteString("module", module);
            writer.WritePropertyName("exception");
            node.Write(writer);
            writer.WriteEndObject();
            if (!original.RootElement.TryGetProperty("location", out _) && node.Frames.FirstOrDefault(f => f.File is not null) is { } location)
            {
                writer.WriteStartObject("location");
                writer.WriteString("file", location.File);
                if (location.Line > 0) writer.WriteNumber("line", location.Line);
                if (location.Column > 0) writer.WriteNumber("column", location.Column);
                writer.WriteEndObject();
            }
            writer.WriteEndObject();
        }
        return bytes.ToArray();
    }

    static Node Capture(Exception error, Budget budget, int depth)
    {
        budget.Nodes--;
        var node = new Node();
        node.Type = Text(error.GetType().FullName ?? error.GetType().Name, node);
        node.Message = Text(error.Message, node);
        try
        {
            var trace = new StackTrace(error, fNeedFileInfo: true);
            var count = Math.Min(trace.FrameCount, budget.Frames);
            for (var index = 0; index < count; index++)
            {
                if (trace.GetFrame(index) is not { } frame) { node.Truncated = true; continue; }
                var file = frame.GetFileName();
                node.Frames.Add(new(Text(Method(frame), node), file is null ? null : Text(file, node),
                    frame.GetFileLineNumber(), frame.GetFileColumnNumber()));
            }
            budget.Frames -= count;
            node.Truncated |= count < trace.FrameCount;
        }
        catch { node.Truncated = true; } // missing/trimmed symbols must not escape a fault handler
        var inners = error is AggregateException aggregate ? aggregate.InnerExceptions :
            error.InnerException is { } cause ? (IReadOnlyList<Exception>)[cause] : [];
        foreach (var inner in inners)
        {
            if (depth >= MaxDepth || budget.Nodes == 0) { node.Truncated = true; break; }
            var child = Capture(inner, budget, depth + 1);
            node.Inners.Add(child);
            node.Truncated |= child.Truncated;
        }
        return node;
    }

    [UnconditionalSuppressMessage("Trimming", "IL2026", Justification = "Best-effort metadata for already executing stack frames; trimmed metadata is handled as unavailable.")]
    static string Method(StackFrame frame)
    {
        var method = frame.GetMethod();
        return method is null ? "<unknown>" : $"{method.DeclaringType?.FullName ?? "<global>"}.{method.Name}";
    }

    static string Text(string value, Node node)
    {
        var result = new StringBuilder(Math.Min(value.Length, MaxStringBytes));
        var bytes = 0;
        foreach (var rune in value.EnumerateRunes())
        {
            if (bytes + rune.Utf8SequenceLength > MaxStringBytes) { node.Truncated = true; break; }
            result.Append(rune.ToString());
            bytes += rune.Utf8SequenceLength;
        }
        return result.ToString();
    }
}

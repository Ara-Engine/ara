using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Ara.Hosting;

namespace Ara.Runtime.Tests;

public sealed unsafe class ExceptionDiagnosticsTests
{
    [Fact]
    public void Fault_keeps_the_full_managed_chain_and_physical_source_locations_on_one_row()
    {
        var error = CaptureFailure();
        var row = Report(error);
        Assert.Equal("ARA0911", row.GetProperty("code").GetString());
        Assert.Equal("error", row.GetProperty("severity").GetString());
        Assert.Contains("disabled until the next reload", row.GetProperty("message").GetString());
        var root = row.GetProperty("data").GetProperty("exception");
        Assert.Equal(1, root.GetProperty("schema").GetInt32());
        Assert.Equal(typeof(InvalidOperationException).FullName, root.GetProperty("type").GetString());
        Assert.Equal("outer failure", root.GetProperty("message").GetString());
        var frames = root.GetProperty("frames").EnumerateArray().ToArray();
        Assert.Contains(frames, f => f.GetProperty("method").GetString()!.Contains(nameof(ThrowOuter)));
        Assert.Contains(frames, f => f.GetProperty("method").GetString()!.Contains(nameof(Middle)));
        Assert.Contains(frames, f => f.GetProperty("method").GetString()!.Contains(nameof(CaptureFailure)));
        var located = frames.First(f => f.GetProperty("file").ValueKind == JsonValueKind.String);
        Assert.EndsWith("ExceptionDiagnosticsTests.cs", located.GetProperty("file").GetString());
        Assert.True(located.GetProperty("line").GetUInt32() > 0);
        Assert.Equal(located.GetProperty("file").GetString(), row.GetProperty("location").GetProperty("file").GetString());
        var inner = Assert.Single(root.GetProperty("inners").EnumerateArray());
        Assert.Equal(typeof(ArgumentException).FullName, inner.GetProperty("type").GetString());
        Assert.Contains(inner.GetProperty("frames").EnumerateArray(), f => f.GetProperty("method").GetString()!.Contains(nameof(ThrowInner)));
        Assert.False(root.GetProperty("truncated").GetBoolean());
    }

    [Fact]
    public void Aggregate_order_and_payload_limits_are_explicit_and_deterministic()
    {
        var errors = Enumerable.Range(0, 20).Select(i => new InvalidOperationException($"inner {i}")).ToArray();
        var error = new AggregateException(new string('\u00e9', 9000), errors);
        var first = Report(error).GetProperty("data").GetProperty("exception");
        var second = Report(error).GetProperty("data").GetProperty("exception");
        Assert.Equal(first.GetRawText(), second.GetRawText());
        Assert.True(first.GetProperty("truncated").GetBoolean());
        Assert.True(Encoding.UTF8.GetByteCount(first.GetProperty("message").GetString()!) <= 8192);
        var inners = first.GetProperty("inners").EnumerateArray().ToArray();
        Assert.Equal(15, inners.Length); // root plus fifteen children reaches the node budget
        Assert.Equal(Enumerable.Range(0, 15).Select(i => $"inner {i}"), inners.Select(i => i.GetProperty("message").GetString()));
        foreach (var node in inners) Assert.Equal(1, node.GetProperty("schema").GetInt32());
    }

    [Fact]
    public void Stack_and_inner_depth_are_bounded_without_inventing_locations()
    {
        var stack = Report(DeepFailure(100)).GetProperty("data").GetProperty("exception");
        Assert.Equal(64, stack.GetProperty("frames").GetArrayLength());
        Assert.True(stack.GetProperty("truncated").GetBoolean());
        Exception nested = new InvalidOperationException("leaf");
        for (var i = 0; i < 20; i++) nested = new InvalidOperationException($"level {i}", nested);
        var root = Report(nested).GetProperty("data").GetProperty("exception");
        Assert.True(root.GetProperty("truncated").GetBoolean());
        var node = root;
        var depth = 0;
        while (node.GetProperty("inners").GetArrayLength() != 0)
        {
            node = node.GetProperty("inners")[0];
            depth++;
        }
        Assert.InRange(depth, 1, 8);
        Assert.True(node.GetProperty("truncated").GetBoolean());
        Assert.Empty(root.GetProperty("frames").EnumerateArray());
    }

    [Fact]
    public void Existing_scope_subjects_message_and_location_survive_exception_enrichment()
    {
        const string original = """{"code":"ARA0912","severity":"error","message":"keep this summary","location":{"file":"display/Script.cs","line":7},"subjects":[{"kind":"system","id":"original"}],"data":{"compile":{"binding":3},"play":{"source":"player-a","binding":8},"custom":42}}""";
        using var before = JsonDocument.Parse(original);
        using var after = JsonDocument.Parse(ExceptionDetails.Attach(Encoding.UTF8.GetBytes(original), CaptureFailure()));
        foreach (var field in new[] { "code", "severity", "message", "location", "subjects" })
            Assert.Equal(before.RootElement.GetProperty(field).GetRawText(), after.RootElement.GetProperty(field).GetRawText());
        foreach (var field in new[] { "compile", "play", "custom" })
            Assert.Equal(before.RootElement.GetProperty("data").GetProperty(field).GetRawText(), after.RootElement.GetProperty("data").GetProperty(field).GetRawText());
        Assert.True(after.RootElement.GetProperty("data").TryGetProperty("exception", out _));
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static Exception CaptureFailure()
    {
        try { Middle(); }
        catch (Exception error) { return error; }
        throw new InvalidOperationException("test did not throw");
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static void Middle() => ThrowOuter();

    [MethodImpl(MethodImplOptions.NoInlining)]
    static void ThrowOuter()
    {
        try { ThrowInner(); }
        catch (Exception inner) { throw new InvalidOperationException("outer failure", inner); }
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static void ThrowInner() => throw new ArgumentException("inner failure");

    [MethodImpl(MethodImplOptions.NoInlining)]
    static Exception DeepFailure(int depth)
    {
        try { Recurse(depth); }
        catch (Exception error) { return error; }
        throw new InvalidOperationException("test did not throw");
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static void Recurse(int depth)
    {
        if (depth == 0) throw new InvalidOperationException("deep stack");
        Recurse(depth - 1);
        GC.KeepAlive(depth);
    }

    static JsonElement Report(Exception error)
    {
        var rows = new List<byte[]>();
        var handle = GCHandle.Alloc(rows);
        try
        {
            var init = new HostInit { Ctx = (void*)GCHandle.ToIntPtr(handle), Api = new HostApi { Log = &Log, Diagnostic = &Diagnostic } };
            using var binding = Host.Bind(new HostContext(init));
            Diag.Fault("ARA0911", "behaviour `Broken`", error);
            using var json = JsonDocument.Parse(Assert.Single(rows));
            return json.RootElement.Clone();
        }
        finally { handle.Free(); }
    }

    [UnmanagedCallersOnly]
    static void Log(void* context, uint level, byte* text, uint length) { }

    [UnmanagedCallersOnly]
    static void Diagnostic(void* context, byte* json, uint length) =>
        ((List<byte[]>)GCHandle.FromIntPtr((nint)context).Target!).Add(new ReadOnlySpan<byte>(json, checked((int)length)).ToArray());
}

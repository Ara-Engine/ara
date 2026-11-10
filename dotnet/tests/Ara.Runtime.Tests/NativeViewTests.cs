using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using Ara.Hosting;
using Xunit.Abstractions;

namespace Ara.Runtime.Tests;

/// <summary>Exercises checked native access and the portable codec fallback for the same generated data type.</summary>
public sealed unsafe class NativeViewTests(ITestOutputHelper output)
{
    static Velocity* s_values;
    static int s_count;
    static int s_reads;
    static readonly byte[] ZeroVelocity = Encoding.UTF8.GetBytes("{\"linear\":[0,0,0],\"angular\":[0,0,0]}");

    [UnmanagedCallersOnly]
    static byte* ComponentPtr(void* context, ulong entity, uint type, uint write)
    {
        try { return type == Velocity.TypeIndex && entity > 0 && entity <= (ulong)s_count ? (byte*)(s_values + (int)entity - 1) : null; }
        catch { return null; }
    }

    [UnmanagedCallersOnly]
    static int ComponentJson(void* context, ulong entity, uint type, uint operation, byte* data, uint length, byte* destination, uint capacity)
    {
        try
        {
            if (operation == AbiConst.ComponentHas) return 1;
            if (operation == AbiConst.ComponentWrite) return 1;
            if (capacity < ZeroVelocity.Length) return -ZeroVelocity.Length;
            ZeroVelocity.CopyTo(new Span<byte>(destination, (int)capacity));
            s_reads++;
            return ZeroVelocity.Length;
        }
        catch { return 0; }
    }

    [Fact]
    public void Native_physics_access_updates_the_pointer_without_codec_calls_or_allocation()
    {
        var values = new Velocity[1024];
        fixed (Velocity* data = values)
        {
            s_values = data; s_count = values.Length; s_reads = 0;
            using var binding = Host.Bind(new HostContext(new HostInit { Api = new HostApi { ComponentPtr = &ComponentPtr, ComponentJson = &ComponentJson } }));
            var previous = Host.EnterExclusive();
            try
            {
                for (var warm = 0; warm < 100; warm++) World.Get<Velocity>(new(1)).Linear.X += 1;
                var allocated = GC.GetAllocatedBytesForCurrentThread();
                for (var i = 0; i < values.Length; i++) World.Get<Velocity>(new((ulong)i + 1)).Linear.Y = i;
                Assert.Equal(0, GC.GetAllocatedBytesForCurrentThread() - allocated);
                Assert.Equal(0, s_reads);
                Assert.Equal(100, values[0].Linear.X);
                for (var i = 0; i < values.Length; i++) Assert.Equal(i, values[i].Linear.Y);
                EngineAccess.Flush();
            }
            finally { Host.ExitExclusive(previous); s_values = null; s_count = 0; }
        }
        Probe();
    }

    void Probe()
    {
        var path = System.Environment.GetEnvironmentVariable("ARA_NATIVE_VIEW_BENCH_OUTPUT");
        if (path is null) return;
        const int count = 8192;
        var values = new Velocity[count];
        var cases = new List<object>();
        fixed (Velocity* data = values)
        {
            s_values = data; s_count = count;
            using var binding = Host.Bind(new HostContext(new HostInit { Api = new HostApi { ComponentPtr = &ComponentPtr, ComponentJson = &ComponentJson } }));
            var previous = Host.EnterExclusive();
            try
            {
                foreach (var native in new[] { false, true })
                {
                    void Run()
                    {
                        for (var i = 0; i < count; i++)
                        {
                            var entity = new Entity((ulong)i + 1);
                            if (native) World.Get<Velocity>(entity).Linear.X += 1;
                            else EngineAccess.Get(entity, Velocity.TypeIndex, true, EngineValueCodecs.ReadVelocity, EngineValueCodecs.WriteVelocity).Linear.X += 1;
                        }
                        EngineAccess.Flush();
                    }
                    for (var warm = 0; warm < 12; warm++) Run();
                    var samples = new double[32];
                    var allocated = GC.GetAllocatedBytesForCurrentThread();
                    for (var i = 0; i < samples.Length; i++)
                    {
                        var start = Stopwatch.GetTimestamp();
                        Run();
                        samples[i] = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
                    }
                    allocated = GC.GetAllocatedBytesForCurrentThread() - allocated;
                    var ordered = samples.Order().ToArray();
                    cases.Add(new { native, entities = count, p50_ms = ordered[ordered.Length / 2], p95_ms = ordered[(int)(ordered.Length * 0.95)], bytes_per_batch = allocated / samples.Length, samples_ms = samples });
                }
            }
            finally { Host.ExitExclusive(previous); s_values = null; s_count = 0; }
        }
        var json = System.Text.Json.JsonSerializer.Serialize(cases, new System.Text.Json.JsonSerializerOptions { WriteIndented = true });
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);
        File.WriteAllText(path, json);
        output.WriteLine(json);
    }
}

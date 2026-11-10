using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Ara.Hosting;
using Xunit.Abstractions;

namespace Ara.Runtime.Tests;

/// <summary>Exercises the real generated accessor across one exclusive callback batch.</summary>
public sealed unsafe class BridgeScaleTests(ITestOutputHelper output)
{
    static readonly byte[] VelocityJson = Encoding.UTF8.GetBytes("{\"linear\":[0,0,0],\"angular\":[0,0,0]}");
    static int s_reads;
    static int s_writes;

    [UnmanagedCallersOnly]
    static int ComponentJson(void* context, ulong entity, uint index, uint operation, byte* data, uint length, byte* destination, uint capacity)
    {
        try
        {
            if (operation == AbiConst.ComponentWrite) { s_writes++; return 1; }
            if (operation == AbiConst.ComponentHas) return 1;
            if (capacity < VelocityJson.Length) return -VelocityJson.Length;
            VelocityJson.CopyTo(new Span<byte>(destination, (int)capacity));
            s_reads++;
            return VelocityJson.Length;
        }
        catch { return 0; }
    }

    [Fact]
    public void Accessor_batches_keep_distinct_entities_and_flush_each_write_once()
    {
        var reportPath = System.Environment.GetEnvironmentVariable("ARA_BRIDGE_BENCH_OUTPUT");
        int[] sizes = reportPath is null ? [64] : [64, 1024, 8192];
        var results = new List<object>();
        var context = new HostContext(new HostInit { Api = new HostApi { ComponentJson = &ComponentJson } });
        foreach (var count in sizes)
        {
            void Run()
            {
                using var binding = Host.Bind(context);
                var previous = Host.EnterExclusive();
                try
                {
                    for (var i = 0; i < count; i++)
                    {
                        var entity = new Entity((ulong)i + 1);
                        ref var value = ref World.Get<Velocity>(entity);
                        Assert.Equal(0, value.Linear.X);
                        value.Linear.X += 1;
                        Assert.Equal(1, World.Read<Velocity>(entity).Linear.X);
                    }
                    EngineAccess.Flush();
                }
                finally { Host.ExitExclusive(previous); }
            }
            for (var warm = 0; warm < 8; warm++) Run();
            s_reads = 0;
            s_writes = 0;
            var samples = new double[reportPath is null ? 1 : 12];
            var allocated = GC.GetAllocatedBytesForCurrentThread();
            for (var i = 0; i < samples.Length; i++)
            {
                var start = Stopwatch.GetTimestamp();
                Run();
                samples[i] = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
            }
            allocated = GC.GetAllocatedBytesForCurrentThread() - allocated;
            Assert.Equal(count * samples.Length, s_reads);
            Assert.Equal(count * samples.Length, s_writes);
            var ordered = samples.Order().ToArray();
            results.Add(new { entities = count, p50_ms = ordered[ordered.Length / 2], bytes_per_batch = allocated / samples.Length, samples_ms = samples });
        }
        var json = JsonSerializer.Serialize(results, new JsonSerializerOptions { WriteIndented = true });
        output.WriteLine(json);
        if (reportPath is not null)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(reportPath))!);
            File.WriteAllText(reportPath, json);
        }
    }
}

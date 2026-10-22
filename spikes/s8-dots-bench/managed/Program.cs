using System.Diagnostics;
using System.Runtime;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;
using S8;

if (args.Length != 7) throw new ArgumentException("WORKLOAD COUNT THREADS WARMUP FRAMES RESULT_JSON DUMP_OR_DASH");
string workload = args[0];
int count = int.Parse(args[1]), threads = int.Parse(args[2]), warmup = int.Parse(args[3]), frames = int.Parse(args[4]);
ThreadPool.SetMinThreads(threads, threads);
GCSettings.LatencyMode = GCLatencyMode.SustainedLowLatency;
nint handle = Host.Allocate(count, workload == "w4");
try { Run(); }
finally { Host.Free(handle); }

unsafe void Run()
{
    HostView view = Host.View(handle);
    var runner = new Runner(handle, view, workload, threads);
    for (int frame = 0; frame < warmup; frame++) runner.Step((uint)frame);
    var times = new double[frames];
    var gcStart = new[] { GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2) };
    TimeSpan pauseStart = GC.GetTotalPauseDuration();
    long allocationStart = GC.GetTotalAllocatedBytes(true);
    for (int frame = 0; frame < frames; frame++)
    {
        long start = Stopwatch.GetTimestamp();
        runner.Step((uint)(warmup + frame));
        times[frame] = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
    }
    long allocationBytes = GC.GetTotalAllocatedBytes(true) - allocationStart;
    int[] collections = [GC.CollectionCount(0) - gcStart[0], GC.CollectionCount(1) - gcStart[1], GC.CollectionCount(2) - gcStart[2]];
    double pauseMs = (GC.GetTotalPauseDuration() - pauseStart).TotalMilliseconds;
    if (workload == "w4") Host.Collect(handle);
    double[] sums = new double[12];
    float[][] samples = new float[64][];
    using var dump = args[6] != "-" ? new BinaryWriter(File.Create(args[6])) : null;
    Span<float> r = stackalloc float[12];
    for (int i = 0; i < count; i++)
    {
        F4 p = view.p[i], v = view.v[i], q = view.q[i];
        MemoryMarshal.Cast<float, F4>(r)[0] = p;
        MemoryMarshal.Cast<float, F4>(r)[1] = v;
        MemoryMarshal.Cast<float, F4>(r)[2] = q;
        for (int j = 0; j < 12; j++) sums[j] += r[j];
        if (dump != null) dump.Write(MemoryMarshal.AsBytes(r));
    }
    for (int i = 0; i < 64; i++)
    {
        int id = i * (count - 1) / 63;
        F4 p = view.p[id], v = view.v[id], q = view.q[id];
        samples[i] = [p.x, p.y, p.z, p.w, v.x, v.y, v.z, v.w, q.x, q.y, q.z, q.w];
    }
    var sorted = times.Order().ToArray();
    var result = new ResultRow { setup = RuntimeFeature.IsDynamicCodeSupported ? "jit" : "aot", workload = workload, count = count, threads = threads,
        warmup = warmup, frames = frames, p50_ms = sorted[(int)Math.Ceiling(frames * 0.50) - 1], p95_ms = sorted[(int)Math.Ceiling(frames * 0.95) - 1],
        ms = times, sums = sums, samples = samples, allocated_bytes = allocationBytes, gc_collections = collections, gc_pause_ms = pauseMs };
    File.WriteAllText(args[5], JsonSerializer.Serialize(result, ResultJson.Default.ResultRow));
    Console.WriteLine($"{result.setup} {workload} n={count} threads={threads} p50={result.p50_ms:F3} p95={result.p95_ms:F3} ms");
}

/// Cache the range callback so the kernel does not allocate a closure per chunk.
sealed unsafe class Runner
{
    readonly nint handle;
    readonly HostView h;
    readonly string workload;
    readonly int threads;
    readonly ParallelOptions options;
    readonly Action<int> runChunk;
    uint frame;
    readonly int chunkSize;
    public Runner(nint handle, HostView h, string workload, int threads)
    {
        this.handle = handle; this.h = h; this.workload = workload; this.threads = threads;
        options = new ParallelOptions { MaxDegreeOfParallelism = threads };
        chunkSize = workload is "w2" or "w4" ? 256 : 4096;
        runChunk = RunChunk;
    }
    public void Step(uint frame)
    {
        this.frame = frame;
        if (workload == "w2") Kernels.BuildGrid(h);
        int n = workload == "w4" ? 10000 : h.count;
        if (threads > 1) Parallel.For(0, (n + chunkSize - 1) / chunkSize, options, runChunk);
        else RunRange(0, n);
        if (workload == "w4") Host.Playback(handle);
    }
    void RunChunk(int chunk)
    {
        int n = workload == "w4" ? 10000 : h.count;
        RunRange(chunk * chunkSize, Math.Min((chunk + 1) * chunkSize, n));
    }
    void RunRange(int start, int end)
    {
        switch (workload)
        {
            case "w1": Kernels.Transform(new Span<F4>(h.p + start, end - start), new ReadOnlySpan<F4>(h.v + start, end - start), new Span<F4>(h.q + start, end - start)); break;
            case "w2": Kernels.Boids(h, start, end); break;
            case "w3": Kernels.Particles(new Span<F4>(h.p + start, end - start), new Span<F4>(h.v + start, end - start)); break;
            case "w3simd": Kernels.ParticlesSimd(h.p + start, h.v + start, end - start); break;
            case "w4": Kernels.Churn(h, start, end, frame); break;
            default: throw new ArgumentException(workload);
        }
    }
}

sealed class ResultRow
{
    public required string setup { get; init; }
    public required string workload { get; init; }
    public int count { get; init; }
    public int threads { get; init; }
    public int warmup { get; init; }
    public int frames { get; init; }
    public double p50_ms { get; init; }
    public double p95_ms { get; init; }
    public required double[] ms { get; init; }
    public required double[] sums { get; init; }
    public required float[][] samples { get; init; }
    public long allocated_bytes { get; init; }
    public required int[] gc_collections { get; init; }
    public double gc_pause_ms { get; init; }
}

[JsonSerializable(typeof(ResultRow))]
sealed partial class ResultJson : JsonSerializerContext;

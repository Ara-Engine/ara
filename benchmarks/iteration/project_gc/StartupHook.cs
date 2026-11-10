using System.Text;
using System.Text.Json;

/// <summary>Diagnostic-only reply-boundary counters; never loaded by a gameplay world.</summary>
internal static class StartupHook
{
    public static void Initialize()
    {
        var path = System.Environment.GetEnvironmentVariable("ARA_GC_LOG") ?? throw new InvalidOperationException("missing private log");
        var count = int.Parse(System.Environment.GetEnvironmentVariable("ARA_GC_REQUESTS")!);
        var log = new StreamWriter(path, append: false) { AutoFlush = true };
        AppDomain.CurrentDomain.ProcessExit += (_, _) => log.Dispose();
        Console.SetOut(new Replies(Console.Out, log, count));
    }

    sealed class Replies(TextWriter inner, TextWriter log, int finalCount) : TextWriter
    {
        int index;
        long allocated = GC.GetTotalAllocatedBytes(precise: true);
        TimeSpan pause = GC.GetTotalPauseDuration();
        int[] collections = [GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2)];
        public override Encoding Encoding => inner.Encoding;
        public override void Flush() => inner.Flush();
        public override Task FlushAsync() => inner.FlushAsync();
        public override void WriteLine(string? value) { Capture(); inner.WriteLine(value); }
        public override Task WriteLineAsync(string? value) { Capture(); return inner.WriteLineAsync(value); }

        void Capture()
        {
            var currentAllocated = GC.GetTotalAllocatedBytes(precise: true);
            var currentPause = GC.GetTotalPauseDuration();
            var currentCollections = new[] { GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2) };
            var info = GC.GetGCMemoryInfo();
            long? afterFullGc = null;
            if (++index == finalCount)
            {
                // Only after all compile work. This deliberately invalidates final-request timing.
                GC.Collect(); GC.WaitForPendingFinalizers(); GC.Collect();
                afterFullGc = GC.GetTotalMemory(forceFullCollection: false);
            }
            log.WriteLine(JsonSerializer.Serialize(new { request = index - 1,
                allocationBytes = currentAllocated - allocated,
                collections = Enumerable.Range(0, 3).Select(i => currentCollections[i] - collections[i]).ToArray(),
                pauseMilliseconds = (currentPause - pause).TotalMilliseconds,
                info.HeapSizeBytes, info.FragmentedBytes, finalLiveHeapBytes = afterFullGc }));
            allocated = GC.GetTotalAllocatedBytes(precise: true);
            pause = GC.GetTotalPauseDuration();
            collections = [GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2)];
        }
    }
}

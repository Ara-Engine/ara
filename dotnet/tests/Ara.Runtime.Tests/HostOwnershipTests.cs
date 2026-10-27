using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using Ara.Hosting;
using Xunit.Abstractions;

namespace Ara.Runtime.Tests;

public sealed unsafe class HostOwnershipTests(ITestOutputHelper output)
{
    static readonly ConcurrentQueue<nint> Logs = new();
    static readonly ConcurrentQueue<nint> Reports = new();
    static readonly ConcurrentQueue<string> Diagnostics = new();

    sealed class Loader : IModuleLoader
    {
        public ScriptRuntime Runtime { get; private set; } = null!;
        public int Stops { get; private set; }
        public void Attach(ScriptRuntime runtime) => Runtime = runtime;
        public int Load(string json)
        {
            Host.SetTypeIndex("Game.Probe", (uint)(nuint)Runtime.Context.Init.Ctx);
            Runtime.Enqueue(new ErrorUpdate { Slot = 0, Report = new ReloadReport { Kind = AbiConst.ReloadL4 } });
            Log.Info("load");
            return 0;
        }
        public bool CanReload => true;
        public int SetHeat(uint slot, bool hot) => 0;
        public int RequestCompile(string json) => 0;
        public int AlcsAlive => 0;
        public int AlcsCreated => 0;
        public int Shutdown() { Stops++; return 0; }
    }

    [UnmanagedCallersOnly]
    static void LogMessage(void* ctx, uint level, byte* message, uint len)
    {
        try { Logs.Enqueue((nint)ctx); } catch { }
    }

    [UnmanagedCallersOnly]
    static void Reload(void* ctx, ReloadReport* report)
    {
        try { Reports.Enqueue((nint)ctx); } catch { }
    }

    [UnmanagedCallersOnly]
    static ulong Now(void* ctx) { try { return 0; } catch { return 0; } }

    [UnmanagedCallersOnly]
    static void Diagnostic(void* ctx, byte* message, uint len)
    {
        try { Diagnostics.Enqueue(Encoding.UTF8.GetString(message, (int)len)); } catch { }
    }

    static int LoadModules(ScriptApi api, nint handle)
    {
        ReadOnlySpan<byte> config = "{}"u8;
        fixed (byte* json = config) return api.LoadModules((void*)handle, json, (uint)config.Length);
    }

    static ScriptApi Start(nint handle, Loader loader)
    {
        var init = new HostInit {
            AbiVersion = AbiInfo.Version, LayoutHash = AbiInfo.LayoutHash, Ctx = (void*)handle,
            Api = new HostApi { Log = &LogMessage, ReportReload = &Reload, NowNs = &Now, Diagnostic = &Diagnostic }
        };
        var api = new ScriptApi();
        Assert.Equal(0, ScriptRuntime.Init(&init, &api, loader));
        return api;
    }

    [Fact]
    public void Hosts_own_loaders_queues_callbacks_and_generated_type_indexes()
    {
        Logs.Clear(); Reports.Clear(); Diagnostics.Clear();
        var first = new Loader(); var second = new Loader();
        var a = Start(1, first); var b = Start(2, second);
        try
        {
            Assert.True(LoadModules(a, 1) == 0, string.Join("\n", Diagnostics));
            Assert.True(LoadModules(b, 2) == 0, string.Join("\n", Diagnostics));
            Assert.Equal(new nint[] { 1, 2 }, Logs.ToArray());
            Assert.Equal(new nint[] { 1, 2 }, Reports.ToArray());
            using (first.Runtime.Activate())
            {
                Assert.Equal(1u, Host.TypeIndex("Game.Probe"));
                using (second.Runtime.Activate()) Assert.Equal(2u, Host.TypeIndex("Game.Probe"));
                Assert.Equal(1u, Host.TypeIndex("Game.Probe"));
            }
            first.Runtime.Enqueue(new ErrorUpdate { Slot = 0 });
            Assert.Equal(1u, a.HasPending((void*)1));
            Assert.Equal(0u, b.HasPending((void*)2));
            var firstReader = new Thread(() => { using var binding = first.Runtime.Activate(); Log.Info("first async"); });
            var secondReader = new Thread(() => { using var binding = second.Runtime.Activate(); Log.Info("second async"); });
            firstReader.Start(); secondReader.Start();
            firstReader.Join(); secondReader.Join();
            Assert.Equal(2, Logs.Count(h => h == 1));
            Assert.Equal(2, Logs.Count(h => h == 2));
            Assert.Equal(0, a.Shutdown((void*)1));
            Assert.Equal(1, first.Stops);
            Assert.Equal(0, second.Stops);
            Assert.True(LoadModules(b, 2) == 0, string.Join("\n", Diagnostics));
            Assert.Equal(0, a.Shutdown((void*)1));
            Assert.Equal(1, first.Stops);
        }
        finally { a.Shutdown((void*)1); b.Shutdown((void*)2); }
        Assert.Equal(1, second.Stops);
        Assert.Empty(Diagnostics);
        Assert.False(Host.Attached);
    }

    [Fact]
    public void Host_dispatch_is_allocation_free_after_warmup()
    {
        const int count = 100_000;
        var loader = new Loader();
        var api = Start(3, loader);
        var call = new SystemCall();
        try
        {
            for (int i = 0; i < count; i++) api.RunSystem((void*)3, &call);
            var allocated = GC.GetAllocatedBytesForCurrentThread();
            var start = Stopwatch.GetTimestamp();
            for (int i = 0; i < count; i++) loader.Runtime.RunSystem(&call);
            var direct = Stopwatch.GetTimestamp() - start;
            start = Stopwatch.GetTimestamp();
            for (int i = 0; i < count; i++) api.RunSystem((void*)3, &call);
            var routed = Stopwatch.GetTimestamp() - start;
            Assert.Equal(0, GC.GetAllocatedBytesForCurrentThread() - allocated);
            output.WriteLine($"Empty chunk direct {direct * 1e9 / Stopwatch.Frequency / count:F2} ns; host-routed ABI {routed * 1e9 / Stopwatch.Frequency / count:F2} ns; zero allocated bytes");
        }
        finally { api.Shutdown((void*)3); }
    }
}

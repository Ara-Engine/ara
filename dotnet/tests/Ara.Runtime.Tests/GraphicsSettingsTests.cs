using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text.Json;
using Ara.Hosting;

namespace Ara.Runtime.Tests;

public sealed unsafe class GraphicsSettingsTests
{
    static readonly ConcurrentDictionary<nint, GraphicsState> States = new();
    static readonly ConcurrentDictionary<nint, string> Operations = new();

    sealed class Loader : IModuleLoader
    {
        internal ScriptRuntime Runtime = null!;
        public void Attach(ScriptRuntime runtime) => Runtime = runtime;
        public int Load(string json) => 0;
        public bool CanReload => false;
        public int SetHeat(uint slot, bool hot) => 0;
        public int RequestCompile(string json) => 0;
        public int AlcsAlive => 0;
        public int AlcsCreated => 0;
        public int Shutdown() => 0;
    }

    [UnmanagedCallersOnly]
    static int Graphics(void* ctx, byte* input, uint len, byte* output, uint capacity)
    {
        try
        {
            var request = JsonSerializer.Deserialize(new ReadOnlySpan<byte>(input, (int)len), GraphicsJson.Default.GraphicsRequest)!;
            Operations[(nint)ctx] = request.Operation;
            var state = States[(nint)ctx];
            if (request.Operation == "set") state = state with { Pending = request.Settings! };
            if (request.Operation == "apply") state = state with { Applied = state.Pending, Revision = state.Revision + 1 };
            States[(nint)ctx] = state;
            var bytes = JsonSerializer.SerializeToUtf8Bytes(new GraphicsResponse(state, null), GraphicsJson.Default.GraphicsResponse);
            if (bytes.Length > capacity) return -bytes.Length;
            bytes.CopyTo(new Span<byte>(output, (int)capacity));
            return bytes.Length;
        }
        catch { return 0; }
    }

    [UnmanagedCallersOnly]
    static ulong Revision(void* ctx) { try { return States[(nint)ctx].Revision; } catch { return 0; } }

    static (Loader, ScriptApi) Start(nint identity)
    {
        States[identity] = new GraphicsState { Revision = 1 };
        var loader = new Loader();
        var init = new HostInit { AbiVersion = AbiInfo.Version, LayoutHash = AbiInfo.LayoutHash,
            Ctx = (void*)identity, Api = new HostApi { Graphics = &Graphics, GraphicsRevision = &Revision } };
        var api = new ScriptApi();
        Assert.Equal(0, ScriptRuntime.Init(&init, &api, loader));
        return (loader, api);
    }

    [Fact]
    public void Typed_candidates_cross_the_native_contract_and_applied_events_are_per_host()
    {
        var (first, firstApi) = Start(350001);
        var (second, secondApi) = Start(350002);
        try
        {
            var calls = 0;
            using (first.Runtime.Activate())
            {
                var exclusive = Host.EnterExclusive();
                try
                {
                    GraphicsSettings.Applied += _ => calls++;
                    var pending = GraphicsSettings.Pending;
                    GraphicsSettings.Pending = pending with { Groups = pending.Groups with { Shadows = GraphicsTier.Low } };
                    Assert.Null(GraphicsSettings.Current.Groups.Shadows);
                    var applied = GraphicsSettings.Apply();
                    Assert.Equal(GraphicsTier.Low, applied.Applied.Groups.Shadows);
                    Assert.Equal(1, calls);
                    GraphicsSettings.Save();
                    Assert.Equal("save", Operations[350001]);
                    GraphicsSettings.Query();
                    Assert.Equal(1, calls);
                }
                finally { Host.ExitExclusive(exclusive); }
            }
            using (second.Runtime.Activate())
            {
                var exclusive = Host.EnterExclusive();
                try { Assert.Null(GraphicsSettings.Current.Groups.Shadows); GraphicsSettings.Apply(); }
                finally { Host.ExitExclusive(exclusive); }
            }
            Assert.Equal(1, calls);
        }
        finally
        {
            firstApi.Shutdown((void*)350001);
            secondApi.Shutdown((void*)350002);
            States.TryRemove(350001, out _);
            States.TryRemove(350002, out _);
        }
    }

    [Fact]
    public void Growing_a_large_display_response_never_executes_an_apply_twice()
    {
        var (loader, api) = Start(350003);
        States[350003] = States[350003] with { Capabilities = new GraphicsCapabilities {
            Modes = Enumerable.Repeat(new GraphicsDisplayMode { Monitor = 1, Width = 1920, Height = 1080, RefreshRateMillihz = 60000 }, 10000).ToArray()
        } };
        try
        {
            using var binding = loader.Runtime.Activate();
            var exclusive = Host.EnterExclusive();
            try
            {
                var state = GraphicsSettings.Apply();
                Assert.Equal(2UL, state.Revision);
                Assert.Equal(10000, state.Capabilities.Modes.Length);
                Assert.Equal("get", Operations[350003]);
            }
            finally { Host.ExitExclusive(exclusive); }
        }
        finally { api.Shutdown((void*)350003); States.TryRemove(350003, out _); }
    }

    [Fact]
    public void Removing_a_duplicate_subscription_matches_dotnet_event_semantics()
    {
        var (loader, api) = Start(350004);
        try
        {
            using var binding = loader.Runtime.Activate();
            var exclusive = Host.EnterExclusive();
            try
            {
                var calls = 0;
                Action<GraphicsState> handler = _ => calls++;
                GraphicsSettings.Applied += handler;
                GraphicsSettings.Applied += handler;
                GraphicsSettings.Applied -= handler;
                GraphicsSettings.Apply();
                Assert.Equal(1, calls);
                GraphicsSettings.Applied -= handler;
                GraphicsSettings.Apply();
                Assert.Equal(1, calls);
            }
            finally { Host.ExitExclusive(exclusive); }
        }
        finally { api.Shutdown((void*)350004); States.TryRemove(350004, out _); }
    }

    [Fact]
    public void Options_are_immutable_and_wire_names_are_renderer_neutral()
    {
        var options = new GraphicsOptions { Preset = GraphicsPreset.Auto,
            WindowMode = GraphicsWindowMode.Fullscreen, Upscaler = GraphicsUpscaler.Tsr };
        var bytes = JsonSerializer.SerializeToUtf8Bytes(new GraphicsRequest("set", options), GraphicsJson.Default.GraphicsRequest);
        using var doc = JsonDocument.Parse(bytes);
        Assert.Equal("auto", doc.RootElement.GetProperty("settings").GetProperty("preset").GetString());
        Assert.Equal("fullscreen", doc.RootElement.GetProperty("settings").GetProperty("windowMode").GetString());
        Assert.False(options.RayTracing);
        Assert.False(options.Hdr);
        Assert.Equal(options, JsonSerializer.Deserialize(bytes, GraphicsJson.Default.GraphicsRequest)!.Settings);
        Assert.Throws<InvalidOperationException>(() => GraphicsSettings.Query());
    }
}

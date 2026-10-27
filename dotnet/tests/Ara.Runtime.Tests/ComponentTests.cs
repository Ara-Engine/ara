using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Examples;
using Ara.Hosting;
using Xunit.Abstractions;

namespace Ara.Runtime.Tests;

// A real Rust-host test exercises the same recipes against Bevy. This fixture
// isolates the generated C# field codecs and verifies every recipe changes data.
public sealed class ComponentTests(ITestOutputHelper output)
{
    static readonly Dictionary<uint, byte[]> Values = [];
    static readonly HashSet<uint> Written = [];
    static unsafe Transform* s_transform;

    [UnmanagedCallersOnly]
    static unsafe byte* ComponentPtr(void* ctx, ulong entity, uint index, uint write)
    {
        try { return index == 0 ? (byte*)s_transform : null; }
        catch { return null; }
    }

    [UnmanagedCallersOnly]
    static unsafe int ComponentJson(void* ctx, ulong entity, uint index, uint op, byte* data, uint len, byte* output, uint cap)
    {
        try
        {
            if (op == AbiConst.ComponentHas) return Values.ContainsKey(index) ? 1 : 0;
            if (op == AbiConst.ComponentWrite)
            {
                Values[index] = new ReadOnlySpan<byte>(data, (int)len).ToArray();
                Written.Add(index);
                return 1;
            }
            if (!Values.TryGetValue(index, out var value)) return 0;
            if (value.Length > cap) return -value.Length;
            value.CopyTo(new Span<byte>(output, (int)cap));
            return value.Length;
        }
        catch { return 0; }
    }

    [Fact]
    public unsafe void Every_component_example_reads_and_writes_through_its_generated_codec()
    {
        Values.Clear(); Written.Clear();
        for (uint i = 1; i < EngineComponents.Names.Length; i++) Values[i] = Encoding.UTF8.GetBytes(EngineComponents.DefaultJson[i]);
        var before = Values.ToDictionary(p => p.Key, p => JsonNode.Parse(p.Value));
        var transform = new Transform { Scale = System.Numerics.Vector3.One };
        s_transform = &transform;
        using var binding = Host.Bind(new HostContext(new HostInit { Api = new HostApi { ComponentPtr = &ComponentPtr, ComponentJson = &ComponentJson } }));
        var previous = Host.EnterExclusive();
        try
        {
            var tick = new Tick { Frame = 4, Dt = 1f / 60f };
            ComponentExamples.RunAll(new Entity(~0u), in tick);
            EngineAccess.Flush();
        }
        finally { Host.ExitExclusive(previous); s_transform = null; }
        Assert.Equal(EngineComponents.Names.Length - 1, Written.Count);
        Assert.True(transform.Position.Y > 0);
        foreach (var (index, value) in Values)
            Assert.False(JsonNode.DeepEquals(before[index], JsonNode.Parse(value)), $"{EngineComponents.Names[index]} did not change");
    }

    [Fact]
    public async Task Common_authoring_command_examples_run_through_the_agent_sink()
    {
        var calls = await CommandExamples.TraceAsync();
        Assert.Equal(["entity.create", "component.add", "property.set", "component.remove", "entity.delete"], calls.Select(c => c?["command"]?.GetValue<string>()));
        Assert.Equal("Light", calls[1]?["parameters"]?["component"]?.GetValue<string>());
    }

    [Fact]
    public unsafe void Stale_abi_and_layout_hash_refuse_to_start()
    {
        var loader = new StaticModuleLoader();
        var api = new ScriptApi();
        var init = new HostInit { AbiVersion = AbiInfo.Version - 1, LayoutHash = AbiInfo.LayoutHash };
        Assert.Equal(1, ScriptRuntime.Init(&init, &api, loader));
        init.AbiVersion = AbiInfo.Version;
        init.LayoutHash ^= 1;
        Assert.Equal(2, ScriptRuntime.Init(&init, &api, loader));
    }

    [Fact]
    public void Generated_codecs_round_trip_nested_values_lists_options_and_signed_zero()
    {
        var source = JsonNode.Parse(EngineComponents.DefaultJson[MeshRenderer.TypeIndex])!.AsObject();
        source["model"] = "@models/crate.glb#as_crate";
        source["primitive"] = 2;
        source["materialOverrides"] = new JsonArray(new JsonObject { ["primitive"] = 1, ["material"] = "@materials/steel.graph#gr_steel" });
        source["emissiveMaterials"] = new JsonArray("Lamp", "Panel");
        source["emissiveFactor"] = new JsonArray(-0.0f, 1f, 2f);
        using var document = JsonDocument.Parse(source.ToJsonString());
        var mesh = EngineValueCodecs.ReadMeshRenderer(document.RootElement);
        Assert.Equal(2u, mesh.Primitive);
        Assert.Equal("@materials/steel.graph#gr_steel", mesh.MaterialOverrides[0].Material);
        mesh.MaterialOverrides[0].Primitive = 3;
        // Give the writer an explicit IEEE negative zero, independent of JsonNode's writer.
        mesh.EmissiveFactor = new System.Numerics.Vector3(-0.0f, 1, 2);
        using var roundtrip = JsonDocument.Parse(EngineAccess.Encode(mesh, EngineValueCodecs.WriteMeshRenderer));
        var result = EngineValueCodecs.ReadMeshRenderer(roundtrip.RootElement);
        Assert.Equal(3u, result.MaterialOverrides[0].Primitive);
        Assert.True(result.EmissiveMaterials.SequenceEqual(new[] { "Lamp", "Panel" }));
        Assert.True(float.IsNegative(result.EmissiveFactor!.Value.X));
        Assert.Contains("-0.0", roundtrip.RootElement.GetRawText());
    }

    [Fact]
    public unsafe void Native_transform_dispatch_keeps_the_allocation_free_path()
    {
        const int iterations = 1_000_000;
        var transform = new Transform();
        s_transform = &transform;
        using var binding = Host.Bind(new HostContext(new HostInit { Api = new HostApi { ComponentPtr = &ComponentPtr } }));
        var previous = Host.EnterExclusive();
        static long Measure(bool native, int count)
        {
            var entity = new Entity(~0u);
            var start = System.Diagnostics.Stopwatch.GetTimestamp();
            if (native) for (int i = 0; i < count; i++) World.NativeAccess<Transform>(entity, true).Position.X += 1;
            else for (int i = 0; i < count; i++) World.Get<Transform>(entity).Position.X += 1;
            return System.Diagnostics.Stopwatch.GetTimestamp() - start;
        }
        var baseline = new long[5];
        var dispatch = new long[5];
        try
        {
            Measure(true, iterations); Measure(false, iterations);
            var allocated = GC.GetAllocatedBytesForCurrentThread();
            for (int i = 0; i < baseline.Length; i++)
            {
                baseline[i] = Measure(true, iterations);
                dispatch[i] = Measure(false, iterations);
            }
            Assert.Equal(0, GC.GetAllocatedBytesForCurrentThread() - allocated);
        }
        finally { Host.ExitExclusive(previous); s_transform = null; }
        double Nanoseconds(long ticks) => ticks * 1e9 / System.Diagnostics.Stopwatch.Frequency / iterations;
        output.WriteLine($"Transform native baseline {Nanoseconds(baseline.Order().ElementAt(2)):F2} ns/op; generated dispatch {Nanoseconds(dispatch.Order().ElementAt(2)):F2} ns/op; zero allocated bytes");
    }
}

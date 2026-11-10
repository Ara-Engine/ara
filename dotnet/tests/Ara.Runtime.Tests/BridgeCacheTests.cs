using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Hosting;

namespace Ara.Runtime.Tests;

public sealed unsafe class BridgeCacheTests
{
    static readonly Dictionary<(nint Host, ulong Entity, uint Type), byte[]> Values = [];
    static readonly List<(nint Host, ulong Entity)> Writes = [];
    static ulong s_reject;

    [UnmanagedCallersOnly]
    static int ComponentJson(void* context, ulong entity, uint type, uint operation, byte* data, uint length, byte* destination, uint capacity)
    {
        try
        {
            var key = ((nint)context, entity, type);
            if (operation == AbiConst.ComponentHas) return Values.ContainsKey(key) ? 1 : 0;
            if (operation == AbiConst.ComponentWrite)
            {
                if (s_reject == entity) return 0;
                Values[key] = new ReadOnlySpan<byte>(data, (int)length).ToArray();
                Writes.Add(((nint)context, entity));
                return 1;
            }
            if (!Values.TryGetValue(key, out var bytes)) return 0;
            if (bytes.Length > capacity) return -bytes.Length;
            bytes.CopyTo(new Span<byte>(destination, (int)capacity));
            return bytes.Length;
        }
        catch { return 0; }
    }

    static HostContext Context(nint id) => new(new HostInit { Ctx = (void*)id, Api = new HostApi { ComponentJson = &ComponentJson } });
    static void Velocity(nint host, ulong entity, float x = 0) => Values[(host, entity, Ara.Velocity.TypeIndex)] =
        Encoding.UTF8.GetBytes($"{{\"linear\":[{x.ToString(System.Globalization.CultureInfo.InvariantCulture)},0,0],\"angular\":[0,0,0]}}");
    static void Reset() { Values.Clear(); Writes.Clear(); s_reject = 0; }

    [Fact]
    public void Cache_preserves_first_access_write_order_and_rereads_after_flush()
    {
        Reset(); Velocity(1, 1); Velocity(1, 2);
        using var binding = Host.Bind(Context(1));
        var previous = Host.EnterExclusive();
        try
        {
            _ = World.Read<Ara.Velocity>(new(2));
            World.Get<Ara.Velocity>(new(1)).Linear.X = 1;
            World.Get<Ara.Velocity>(new(2)).Linear.X = 2;
            EngineAccess.Flush();
            Assert.Equal(new (nint, ulong)[] { (1, 2), (1, 1) }, Writes);
            Velocity(1, 1, 40);
            Assert.Equal(40, World.Read<Ara.Velocity>(new(1)).Linear.X);
        }
        finally { Host.ExitExclusive(previous); }
    }

    [Fact]
    public void Nested_hosts_isolate_values_and_discard_failed_callback_copies()
    {
        Reset(); Velocity(1, 1); Velocity(2, 1);
        using var outer = Host.Bind(Context(1));
        var previous = Host.EnterExclusive();
        try
        {
            World.Get<Ara.Velocity>(new(1)).Linear.X = 10;
            using (Host.Bind(Context(2)))
            {
                var inner = Host.EnterExclusive();
                try
                {
                    Assert.Equal(0, World.Read<Ara.Velocity>(new(1)).Linear.X);
                    World.Get<Ara.Velocity>(new(1)).Linear.X = 20;
                    EngineAccess.Discard();
                }
                finally { Host.ExitExclusive(inner); }
            }
            Assert.Equal(10, World.Read<Ara.Velocity>(new(1)).Linear.X);
            EngineAccess.Flush();
            Assert.Equal(new (nint, ulong)[] { (1, 1) }, Writes);
        }
        finally { Host.ExitExclusive(previous); }
    }

    [Fact]
    public void Reentering_a_host_through_another_host_keeps_each_callback_frame_isolated()
    {
        Reset(); Velocity(1, 1); Velocity(2, 1);
        var first = Context(1);
        var second = Context(2);
        using var outer = Host.Bind(first);
        var previous = Host.EnterExclusive();
        try
        {
            World.Get<Ara.Velocity>(new(1)).Linear.X = 10;
            using (Host.Bind(second))
            {
                var middle = Host.EnterExclusive();
                try
                {
                    World.Get<Ara.Velocity>(new(1)).Linear.X = 20;
                    using (Host.Bind(first))
                    {
                        var inner = Host.EnterExclusive();
                        try
                        {
                            Assert.Equal(0, World.Read<Ara.Velocity>(new(1)).Linear.X);
                            World.Get<Ara.Velocity>(new(1)).Linear.X = 30;
                            EngineAccess.Flush();
                        }
                        finally { Host.ExitExclusive(inner); }
                    }
                    Assert.Equal(20, World.Read<Ara.Velocity>(new(1)).Linear.X);
                    EngineAccess.Flush();
                }
                finally { Host.ExitExclusive(middle); }
            }
            Assert.Equal(10, World.Read<Ara.Velocity>(new(1)).Linear.X);
            EngineAccess.Flush();
            Assert.Equal(new (nint, ulong)[] { (1, 1), (2, 1), (1, 1) }, Writes);
        }
        finally { Host.ExitExclusive(previous); }
    }

    [Fact]
    public void A_failed_flush_discards_remaining_copies_and_the_writer_can_be_reused()
    {
        Reset(); Velocity(1, 1); Velocity(1, 2);
        using var binding = Host.Bind(Context(1));
        var previous = Host.EnterExclusive();
        try
        {
            World.Get<Ara.Velocity>(new(1)).Linear.X = 10;
            World.Get<Ara.Velocity>(new(2)).Linear.X = 20;
            s_reject = 1;
            Assert.Throws<InvalidOperationException>(EngineAccess.Flush);
            s_reject = 0;
            Assert.Equal(0, World.Read<Ara.Velocity>(new(2)).Linear.X);
            World.Get<Ara.Velocity>(new(2)).Linear.X = 30;
            EngineAccess.Flush();
            Assert.Equal(new (nint, ulong)[] { (1, 2) }, Writes);
        }
        finally { Host.ExitExclusive(previous); }
    }

    [Fact]
    public void Large_components_outlive_the_transfer_buffer_and_trailing_json_is_rejected()
    {
        Reset();
        var source = JsonNode.Parse(EngineComponents.DefaultJson[AcousticPortal.TypeIndex])!.AsObject();
        var name = new string('x', 8192);
        source["a"] = name;
        Values[(1, 1, AcousticPortal.TypeIndex)] = Encoding.UTF8.GetBytes(source.ToJsonString());
        using var binding = Host.Bind(Context(1));
        var previous = Host.EnterExclusive();
        try
        {
            Assert.Equal(name, World.Read<AcousticPortal>(new(1)).A);
            EngineAccess.Flush();
            Values[(1, 1, AcousticPortal.TypeIndex)] = Encoding.UTF8.GetBytes(source + " {}");
            Assert.ThrowsAny<JsonException>(() => World.Read<AcousticPortal>(new(1)));
            Assert.Throws<MissingComponentException>(() => World.Read<Ara.Velocity>(new(99)));
        }
        finally { Host.ExitExclusive(previous); }
    }
}

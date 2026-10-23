using System.Text.Json;
using Ara.Hosting;

namespace Ara.Runtime.Tests;

public class RngTests
{
    [Fact]
    public void Streams_are_deterministic_and_independent()
    {
        var a = new Rng(42, 7);
        var b = new Rng(42, 7);
        var c = new Rng(42, 8);
        var sa = Enumerable.Range(0, 64).Select(_ => a.NextUInt()).ToArray();
        Assert.Equal(sa, Enumerable.Range(0, 64).Select(_ => b.NextUInt()).ToArray());
        Assert.NotEqual(sa, Enumerable.Range(0, 64).Select(_ => c.NextUInt()).ToArray());
    }

    [Fact]
    public void Per_entity_streams_do_not_depend_on_iteration_order()
    {
        var tick = new Tick { Frame = 10, Seed = 99 };
        var e1 = new Entity(0xFFFF_FFF0UL);
        var e2 = new Entity(0xFFFF_FFF1UL);
        var first = (Rng.For(in tick, 1, e1).NextUInt(), Rng.For(in tick, 1, e2).NextUInt());
        var swapped = (Rng.For(in tick, 1, e2).NextUInt(), Rng.For(in tick, 1, e1).NextUInt());
        Assert.Equal(first, (swapped.Item2, swapped.Item1));
        Assert.NotEqual(first.Item1, first.Item2);
    }

    [Fact]
    public void Ranges_stay_in_bounds()
    {
        var r = new Rng(1, 1);
        for (int i = 0; i < 10_000; i++)
        {
            Assert.InRange(r.Range(-3, 4), -3, 3);
            Assert.InRange(r.NextFloat(), 0f, 0.99999994f);
        }
    }
}

public class EntityTests
{
    [Fact]
    public void Default_is_null_and_bits_decode_like_bevy()
    {
        Assert.True(default(Entity).IsNull);
        Assert.Equal(Entity.Null, default);
        // bevy stores the index complemented (NonMaxU32): index 12, generation 1.
        var e = new Entity(~12u | (1UL << 32));
        Assert.Equal((12u, 1u, "12v1"), (e.Index, e.Generation, e.ToString()));
    }
}

[Component]
partial struct Probe
{
    public ushort Hp;
    public ulong Owner;
}

partial struct Probe : IComponent<Probe>
{
    public static uint TypeIndex => 0;
}

public class DescriptorTests
{
    [Fact]
    public unsafe void Descriptors_carry_measured_layouts()
    {
        var b = new ModuleBuilder();
        Probe p = default;
        var at = (byte*)&p;
        b.Component<Probe>("Probe", [(int)((byte*)&p.Hp - at), (int)((byte*)&p.Owner - at)], _ => { });
        b.Type("Probe", """{"kind":"component","doc":"P.","fields":{}}""");
        using var doc = JsonDocument.Parse(b.Descriptor("Game", 2, 5, hot: true));
        var root = doc.RootElement;
        Assert.Equal((2, 5, true), (root.GetProperty("slot").GetInt32(), root.GetProperty("generation").GetInt32(), root.GetProperty("hot").GetBoolean()));
        var layout = root.GetProperty("components")[0];
        Assert.Equal((16, 8), (layout.GetProperty("size").GetInt32(), layout.GetProperty("align").GetInt32()));
        Assert.Equal([0, 8], layout.GetProperty("offsets").EnumerateArray().Select(o => o.GetInt32()));
        Assert.Equal("component", root.GetProperty("types").GetProperty("types").GetProperty("Probe").GetProperty("kind").GetString());
    }
}

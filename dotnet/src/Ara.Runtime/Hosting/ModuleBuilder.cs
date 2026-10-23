using System.Runtime.CompilerServices;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>One column a system reads or writes, in its parameter order.</summary>
public readonly record struct ColumnAccess(string Component, bool Write);

/// <summary>
/// Collects what one module generation declares. Generated <c>Register</c> methods call
/// it (users never do); the runtime turns it into the descriptor the host registers
/// (schema types, layouts, systems with their access sets, behaviours, commands) and
/// into the dispatch tables.
/// </summary>
public sealed unsafe class ModuleBuilder
{
    internal readonly List<(string Name, string Json)> Types = [];
    internal readonly List<LayoutEntry> Components = [];
    internal readonly List<LayoutEntry> Resources = [];
    internal readonly List<LayoutEntry> Cues = [];
    internal readonly List<SystemDecl> Systems = [];
    internal readonly List<BehaviourDecl> Behaviours = [];
    internal readonly List<CommandDecl> Commands = [];

    internal sealed record LayoutEntry(string Name, int Size, int[] Offsets, Action<uint> SetIndex)
    {
        /// <summary>From a generated <c>__AraLayout()</c>: the size, then the field offsets.</summary>
        internal static LayoutEntry Of(string name, int[] layout, Action<uint> setIndex) =>
            new(name, layout.Length > 0 ? layout[0] : 0, layout.Length > 1 ? layout[1..] : [], setIndex);
    }
    internal sealed record SystemDecl(string Name, nint Run, int Order, ColumnAccess[] Columns, ColumnAccess[] Resources, string[] With, string[] Without);
    internal sealed record BehaviourDecl(string Name, string Data, int DataSize, Func<Behaviour> Create);
    internal sealed record CommandDecl(string Name, string Summary, string Params, LayoutEntry ParamsLayout, nint Invoke,
        string[] Reads, string[] Writes, string? Expose, string[] Examples);

    /// <summary>A schema type definition (one <c>types.json</c> entry) for <paramref name="name"/>.</summary>
    public void Type(string name, string json) => Types.Add((name, json));

    // Registration is deliberately non-generic: a generic method per struct type would be
    // one more JIT per component when a generation loads.

    /// <summary>
    /// A component struct. <paramref name="layout"/> is its generated <c>__AraLayout()</c>
    /// (size, then field offsets); <paramref name="setIndex"/> receives its script index.
    /// </summary>
    public void Component(string name, int[] layout, Action<uint> setIndex) =>
        Components.Add(LayoutEntry.Of(name, layout, setIndex));

    /// <summary>A resource struct (see <see cref="Component"/>).</summary>
    public void Resource(string name, int[] layout, Action<uint> setIndex) =>
        Resources.Add(LayoutEntry.Of(name, layout, setIndex));

    /// <summary>A cue struct (see <see cref="Component"/>).</summary>
    public void Cue(string name, int[] layout, Action<uint> setIndex) =>
        Cues.Add(LayoutEntry.Of(name, layout, setIndex));

    /// <summary>A chunk-batched system: <paramref name="run"/> processes one <see cref="SystemCall"/>.</summary>
    public void System(string name, delegate*<SystemCall*, void> run, int order, ColumnAccess[] columns,
        ColumnAccess[] resources, string[] with, string[] without) =>
        Systems.Add(new(name, (nint)run, order, columns, resources, with, without));

    /// <summary>A behaviour type whose per-entity data lives in component <paramref name="data"/> (<paramref name="dataSize"/> bytes per row).</summary>
    public void Behaviour(string name, string data, int dataSize, Func<Behaviour> create) =>
        Behaviours.Add(new(name, data, dataSize, create));

    /// <summary>
    /// A command: <paramref name="invoke"/> runs one <see cref="CommandCall"/> and returns an
    /// <c>AbiConst.Run*</c> status. <paramref name="paramsLayout"/> is its params struct's
    /// generated <c>__AraParamsLayout()</c>.
    /// </summary>
    public void Command(string name, string summary, string paramsType, int[] paramsLayout,
        delegate*<CommandCall*, int> invoke, string[] reads, string[] writes, string? expose, string[] examples) =>
        Commands.Add(new(name, summary, paramsType, LayoutEntry.Of(paramsType, paramsLayout, static _ => { }),
            (nint)invoke, reads, writes, expose, examples));

    /// <summary>The descriptor the host registers (UTF-8 JSON).</summary>
    internal byte[] Descriptor(string module, uint slot, uint generation, bool hot)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms))
        {
            w.WriteStartObject();
            w.WriteString("module", module);
            w.WriteNumber("slot", slot);
            w.WriteNumber("generation", generation);
            w.WriteBoolean("hot", hot);
            w.WriteStartObject("types");
            w.WriteString("$schema", "ara://schema/types/1");
            w.WriteStartObject("types");
            foreach (var (name, json) in Types)
            {
                w.WritePropertyName(name);
                w.WriteRawValue(json);
            }
            w.WriteEndObject();
            w.WriteEndObject();
            WriteLayouts(w, "components", Components);
            WriteLayouts(w, "resources", Resources);
            WriteLayouts(w, "cues", Cues);
            w.WriteStartArray("systems");
            foreach (var s in Systems)
            {
                w.WriteStartObject();
                w.WriteString("name", s.Name);
                w.WriteNumber("order", s.Order);
                WriteAccess(w, "columns", s.Columns);
                WriteAccess(w, "resources", s.Resources);
                WriteStrings(w, "with", s.With);
                WriteStrings(w, "without", s.Without);
                w.WriteEndObject();
            }
            w.WriteEndArray();
            w.WriteStartArray("behaviours");
            foreach (var b in Behaviours)
            {
                w.WriteStartObject();
                w.WriteString("name", b.Name);
                w.WriteString("data", b.Data);
                w.WriteEndObject();
            }
            w.WriteEndArray();
            w.WriteStartArray("commands");
            foreach (var c in Commands)
            {
                w.WriteStartObject();
                w.WriteString("name", c.Name);
                w.WriteString("summary", c.Summary);
                w.WriteString("params", c.Params);
                w.WritePropertyName("paramsLayout");
                WriteLayout(w, c.ParamsLayout);
                WriteStrings(w, "reads", c.Reads);
                WriteStrings(w, "writes", c.Writes);
                if (c.Expose is not null)
                {
                    w.WritePropertyName("expose");
                    w.WriteRawValue(c.Expose);
                }
                w.WriteStartArray("examples");
                foreach (var e in c.Examples) w.WriteRawValue(e);
                w.WriteEndArray();
                w.WriteEndObject();
            }
            w.WriteEndArray();
            w.WriteEndObject();
        }
        return ms.ToArray();
    }

    static void WriteLayouts(Utf8JsonWriter w, string name, List<LayoutEntry> items)
    {
        w.WriteStartArray(name);
        foreach (var l in items) WriteLayout(w, l);
        w.WriteEndArray();
    }

    static void WriteLayout(Utf8JsonWriter w, LayoutEntry l)
    {
        w.WriteStartObject();
        w.WriteString("name", l.Name);
        w.WriteNumber("size", l.Size);
        w.WriteStartArray("offsets");
        foreach (var o in l.Offsets) w.WriteNumberValue(o);
        w.WriteEndArray();
        w.WriteEndObject();
    }

    static void WriteAccess(Utf8JsonWriter w, string name, ColumnAccess[] items)
    {
        w.WriteStartArray(name);
        foreach (var c in items)
        {
            w.WriteStartObject();
            w.WriteString("name", c.Component);
            w.WriteBoolean("write", c.Write);
            w.WriteEndObject();
        }
        w.WriteEndArray();
    }

    static void WriteStrings(Utf8JsonWriter w, string name, string[] items)
    {
        w.WriteStartArray(name);
        foreach (var s in items) w.WriteStringValue(s);
        w.WriteEndArray();
    }
}

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

    internal sealed record LayoutEntry(string Name, int Size, int Align, int[] Offsets, Action<uint> SetIndex);
    internal sealed record SystemDecl(string Name, nint Run, int Order, ColumnAccess[] Columns, ColumnAccess[] Resources, string[] With, string[] Without);
    internal sealed record BehaviourDecl(string Name, string Data, int DataSize, Func<Behaviour> Create);
    internal sealed record CommandDecl(string Name, string Summary, string Params, LayoutEntry ParamsLayout, nint Invoke,
        string[] Reads, string[] Writes, string? Expose, string[] Examples);

    /// <summary>A schema type definition (one <c>types.json</c> entry) for <paramref name="name"/>.</summary>
    public void Type(string name, string json) => Types.Add((name, json));

    /// <summary>A component struct; <paramref name="setIndex"/> receives its script index.</summary>
    public void Component<T>(string name, int[] offsets, Action<uint> setIndex) where T : unmanaged =>
        Components.Add(new(name, Unsafe.SizeOf<T>(), AlignOf<T>(), offsets, setIndex));

    /// <summary>A resource struct; <paramref name="setIndex"/> receives its script index.</summary>
    public void Resource<T>(string name, int[] offsets, Action<uint> setIndex) where T : unmanaged =>
        Resources.Add(new(name, Unsafe.SizeOf<T>(), AlignOf<T>(), offsets, setIndex));

    /// <summary>A cue struct; <paramref name="setIndex"/> receives its script index.</summary>
    public void Cue<T>(string name, int[] offsets, Action<uint> setIndex) where T : unmanaged =>
        Cues.Add(new(name, Unsafe.SizeOf<T>(), AlignOf<T>(), offsets, setIndex));

    /// <summary>A chunk-batched system: <paramref name="run"/> processes one <see cref="SystemCall"/>.</summary>
    public void System(string name, delegate*<SystemCall*, void> run, int order, ColumnAccess[] columns,
        ColumnAccess[] resources, string[] with, string[] without) =>
        Systems.Add(new(name, (nint)run, order, columns, resources, with, without));

    /// <summary>A behaviour type whose per-entity data lives in component <paramref name="data"/>.</summary>
    public void Behaviour<T, TData>(string name, string data, Func<T> create)
        where T : Behaviour where TData : unmanaged =>
        Behaviours.Add(new(name, data, Unsafe.SizeOf<TData>(), create));

    /// <summary>
    /// A command: <paramref name="invoke"/> runs one <see cref="CommandCall"/> and returns an
    /// <c>AbiConst.Run*</c> status. <typeparamref name="TParams"/> is its params struct.
    /// </summary>
    public void Command<TParams>(string name, string summary, string paramsType, int[] paramsOffsets,
        delegate*<CommandCall*, int> invoke, string[] reads, string[] writes, string? expose, string[] examples)
        where TParams : unmanaged =>
        Commands.Add(new(name, summary, paramsType,
            new(paramsType, Unsafe.SizeOf<TParams>(), AlignOf<TParams>(), paramsOffsets, static _ => { }),
            (nint)invoke, reads, writes, expose, examples));

    struct AlignProbe<T> where T : unmanaged
    {
#pragma warning disable CS0649 // never assigned: only the layout matters
        public byte Pad;
        public T Value;
#pragma warning restore CS0649
    }

    static int AlignOf<T>() where T : unmanaged => Unsafe.SizeOf<AlignProbe<T>>() - Unsafe.SizeOf<T>();

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
        w.WriteNumber("align", l.Align);
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

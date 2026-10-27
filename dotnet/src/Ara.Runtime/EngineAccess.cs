using System.Buffers;
using System.Text.Json;
using Ara.Hosting;

namespace Ara;

// Callback-local copies only. The ECS remains authoritative; no value survives Flush.
internal static class EngineAccess
{
    internal interface IEntry { uint Index { get; } ulong Entity { get; } void Flush(); }
    sealed class Entry<T>(Entity entity, uint index, T value, Action<Utf8JsonWriter, T> write) : IEntry
    {
        public T Value = value;
        public bool Dirty;
        public uint Index => index;
        public ulong Entity => entity.Bits;
        public void Flush()
        {
            if (Dirty) Host.ComponentWrite(entity.Bits, index, Encode(Value, write));
        }
    }

    [ThreadStatic] static List<IEntry>? t_entries;

    internal static List<IEntry>? Swap(List<IEntry>? next)
    {
        var previous = t_entries;
        t_entries = next;
        return previous;
    }

    internal static ref T Get<T>(Entity entity, uint index, bool write,
        Func<JsonElement, T> read, Action<Utf8JsonWriter, T> encode)
    {
        var entries = t_entries ??= [];
        foreach (var item in entries)
            if (item.Index == index && item.Entity == entity.Bits)
            {
                var cached = (Entry<T>)item;
                cached.Dirty |= write;
                return ref cached.Value;
            }
        var bytes = Host.ComponentJson(entity.Bits, index) ?? throw new MissingComponentException(entity, typeof(T).Name);
        using var doc = JsonDocument.Parse(bytes);
        var entry = new Entry<T>(entity, index, read(doc.RootElement), encode) { Dirty = write };
        entries.Add(entry);
        return ref entry.Value;
    }

    internal static byte[] Encode<T>(T value, Action<Utf8JsonWriter, T> write)
    {
        var bytes = new ArrayBufferWriter<byte>();
        using (var writer = new Utf8JsonWriter(bytes)) write(writer, value);
        return bytes.WrittenSpan.ToArray();
    }

    internal static void Flush()
    {
        if (t_entries is not { } entries) return;
        try { foreach (var entry in entries) entry.Flush(); }
        finally { entries.Clear(); }
    }

    internal static void Discard() => t_entries?.Clear();
}

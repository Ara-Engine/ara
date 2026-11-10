using System.Buffers;
using System.Text.Json;
using Ara.Hosting;

namespace Ara;

// Callback-local copies only. The ECS remains authoritative; no value survives Flush.
internal static class EngineAccess
{
    internal interface IEntry { void Flush(Cache cache); void Clear(); }
    // A host may be reentered through another host. Each active binding depth
    // owns independent scratch; inactive frames retain capacity for later calls.
    internal sealed class CachePool
    {
        readonly List<Cache?> _frames = [];
        int _depth;
        internal void Enter()
        {
            if (_depth == _frames.Count) _frames.Add(null);
            _depth++;
        }
        internal Cache Current => _frames[_depth - 1] ??= new();
        internal Cache? Existing => _depth == 0 ? null : _frames[_depth - 1];
        internal void Leave() => _depth--;
    }

    internal sealed class Cache
    {
        internal readonly List<IEntry> Entries = [];
        internal readonly Dictionary<(ulong Entity, uint Index), IEntry> Lookup = [];
        internal readonly ArrayBufferWriter<byte> Buffer = new();
        internal readonly Utf8JsonWriter Writer;
        internal int Used;
        internal Cache() => Writer = new(Buffer);
        internal void Clear()
        {
            for (var i = 0; i < Used; i++) Entries[i].Clear();
            Used = 0;
            Lookup.Clear();
            Buffer.Clear();
        }
    }
    sealed class Entry<T> : IEntry
    {
        public T Value = default!;
        public bool Dirty;
        Entity _entity;
        uint _index;
        Action<Utf8JsonWriter, T> _write = null!;
        internal void Reset(Entity entity, uint index, T value, bool dirty, Action<Utf8JsonWriter, T> write)
        {
            _entity = entity; _index = index; Value = value; Dirty = dirty; _write = write;
        }
        public void Clear() { Value = default!; Dirty = false; _entity = default; _index = 0; }
        public void Flush(Cache cache)
        {
            if (!Dirty) return;
            cache.Buffer.Clear();
            cache.Writer.Reset(cache.Buffer);
            _write(cache.Writer, Value);
            cache.Writer.Flush();
            Host.ComponentWrite(_entity.Bits, _index, cache.Buffer.WrittenSpan);
        }
    }

    internal static ref T Get<T>(Entity entity, uint index, bool write,
        Func<JsonElement, T> read, Action<Utf8JsonWriter, T> encode)
    {
        var entries = Host.AccessCache;
        if (entries.Lookup.TryGetValue((entity.Bits, index), out var item))
        {
            var cached = (Entry<T>)item;
            cached.Dirty |= write;
            return ref cached.Value;
        }
        var value = Host.ReadComponent(entity, index, read);
        Entry<T> entry;
        if (entries.Used < entries.Entries.Count)
        {
            entry = entries.Entries[entries.Used] as Entry<T> ?? new Entry<T>();
            entries.Entries[entries.Used] = entry;
        }
        else
        {
            entry = new Entry<T>();
            entries.Entries.Add(entry);
        }
        entry.Reset(entity, index, value, write, encode);
        entries.Used++;
        entries.Lookup.Add((entity.Bits, index), entry);
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
        if (Host.ExistingAccessCache is not { } entries) return;
        try { for (var i = 0; i < entries.Used; i++) entries.Entries[i].Flush(entries); }
        finally { entries.Clear(); }
    }

    internal static void Discard() => Host.ExistingAccessCache?.Clear();
}

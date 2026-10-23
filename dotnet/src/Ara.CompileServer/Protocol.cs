using System.Buffers.Binary;
using System.Diagnostics;
using System.Text;

namespace Ara.Hosting;

/// <summary>Message kinds on the compile-server pipe.</summary>
public enum MessageKind : byte
{
    /// <summary>Host → server: protocol version, host process id, EnC availability, runtime EnC capabilities.</summary>
    Hello = 1,
    /// <summary>Host → server: compile a module folder (after the modules it references), send its first image, then watch it.</summary>
    Watch = 2,
    /// <summary>Host → server: recompile a module hot (Debug IL, EnC) or cold (optimized).</summary>
    SetHeat = 3,
    /// <summary>Host → server: send a full image (a delta could not be applied).</summary>
    Rebuild = 4,
    /// <summary>Host → server: stop.</summary>
    Shutdown = 5,
    /// <summary>Host → server: files of a module changed (or are about to); compile them with this origin.</summary>
    Change = 6,
    /// <summary>Server → host: a compile started (1) or finished (0).</summary>
    CompileState = 10,
    /// <summary>
    /// Server → host: new generations of one or more modules (initial load, L3, a dependency
    /// cascade), applied together. An entry without PE bytes relinks: the module's current
    /// image and EnC deltas, loaded again against its dependencies' new generations.
    /// </summary>
    Reload = 11,
    /// <summary>Server → host: an EnC delta (L1 or L2).</summary>
    Delta = 12,
    /// <summary>Server → host: diagnostics (ara-diag JSON array); errors mean the edit did not apply.</summary>
    Diagnostics = 13,
    /// <summary>Server → host: a log line.</summary>
    Log = 14,
    /// <summary>Server → host: the change cannot be applied in this process (L4); the last good code keeps running.</summary>
    Restart = 15,
}

/// <summary>
/// Compile-server phase timings of one reload, in microseconds: file event → compile start
/// (<see cref="DetectUs"/>), read + parse + classify, generator, emit, analyzers, and the
/// whole server time from file event to message sent.
/// </summary>
public readonly record struct CompileTiming(uint DetectUs, uint ParseUs, uint GenerateUs, uint EmitUs, uint AnalyzeUs, ulong ServerUs)
{
    /// <summary>Microseconds between two <see cref="Stopwatch.GetTimestamp"/> values (0 if negative).</summary>
    public static uint Micros(long from, long to) =>
        (uint)Math.Clamp((to - from) * 1_000_000 / Stopwatch.Frequency, 0, uint.MaxValue);
}

/// <summary>
/// The compile-server wire format: <c>[u32 length][u8 kind][payload]</c>, little endian;
/// payloads are written with <see cref="Writer"/> and read with <see cref="Reader"/>.
/// Shared by Ara.Host and Ara.CompileServer. <see cref="MessageKind.Reload"/> and <see cref="MessageKind.Delta"/>
/// payloads start with the sender's <see cref="Stopwatch.GetTimestamp"/> (a machine-wide
/// monotonic clock), so the host can measure the transfer.
/// </summary>
public static class Protocol
{
    /// <summary>Bumped on any incompatible change.</summary>
    public const int Version = 2;

    /// <summary>Writes one framed message.</summary>
    public static void Send(Stream s, MessageKind kind, ReadOnlySpan<byte> payload)
    {
        Span<byte> header = stackalloc byte[5];
        BinaryPrimitives.WriteUInt32LittleEndian(header, (uint)payload.Length + 1);
        header[4] = (byte)kind;
        lock (s)
        {
            s.Write(header);
            s.Write(payload);
            s.Flush();
        }
    }

    /// <summary>Reads one framed message, or null at end of stream.</summary>
    public static (MessageKind Kind, byte[] Payload)? Receive(Stream s)
    {
        Span<byte> header = stackalloc byte[5];
        if (!ReadExactly(s, header)) return null;
        var len = (int)BinaryPrimitives.ReadUInt32LittleEndian(header) - 1;
        var payload = new byte[len];
        if (!ReadExactly(s, payload)) return null;
        return ((MessageKind)header[4], payload);
    }

    static bool ReadExactly(Stream s, Span<byte> buf)
    {
        while (buf.Length > 0)
        {
            var n = s.Read(buf);
            if (n == 0) return false;
            buf = buf[n..];
        }
        return true;
    }

    /// <summary>Builds a payload.</summary>
    public sealed class Writer
    {
        readonly MemoryStream _ms = new();

        /// <summary>A 32-bit unsigned integer.</summary>
        public Writer U32(uint v) { Span<byte> b = stackalloc byte[4]; BinaryPrimitives.WriteUInt32LittleEndian(b, v); _ms.Write(b); return this; }

        /// <summary>A 64-bit unsigned integer.</summary>
        public Writer U64(ulong v) { Span<byte> b = stackalloc byte[8]; BinaryPrimitives.WriteUInt64LittleEndian(b, v); _ms.Write(b); return this; }

        /// <summary>A bool as one byte.</summary>
        public Writer Bool(bool v) { _ms.WriteByte(v ? (byte)1 : (byte)0); return this; }

        /// <summary>Length-prefixed bytes.</summary>
        public Writer Bytes(ReadOnlySpan<byte> v) { U32((uint)v.Length); _ms.Write(v); return this; }

        /// <summary>A length-prefixed UTF-8 string.</summary>
        public Writer Str(string v) => Bytes(Encoding.UTF8.GetBytes(v));

        /// <summary>A count-prefixed string list.</summary>
        public Writer Strs(IReadOnlyCollection<string> v)
        {
            U32((uint)v.Count);
            foreach (var s in v) Str(s);
            return this;
        }

        /// <summary>Phase timings.</summary>
        public Writer Timing(CompileTiming t) =>
            U32(t.DetectUs).U32(t.ParseUs).U32(t.GenerateUs).U32(t.EmitUs).U32(t.AnalyzeUs).U64(t.ServerUs);

        /// <summary>The current <see cref="Stopwatch.GetTimestamp"/> (write it first; the reader measures the transfer).</summary>
        public Writer Now() => U64((ulong)Stopwatch.GetTimestamp());

        /// <summary>The payload so far.</summary>
        public byte[] ToArray() => _ms.ToArray();
    }

    /// <summary>Reads a payload in the order it was written.</summary>
    public sealed class Reader(byte[] data)
    {
        int _at;

        /// <summary>A 32-bit unsigned integer.</summary>
        public uint U32() { var v = BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan(_at)); _at += 4; return v; }

        /// <summary>A 64-bit unsigned integer.</summary>
        public ulong U64() { var v = BinaryPrimitives.ReadUInt64LittleEndian(data.AsSpan(_at)); _at += 8; return v; }

        /// <summary>A bool.</summary>
        public bool Bool() => data[_at++] != 0;

        /// <summary>Length-prefixed bytes.</summary>
        public byte[] Bytes() { var n = (int)U32(); var v = data.AsSpan(_at, n).ToArray(); _at += n; return v; }

        /// <summary>A length-prefixed UTF-8 string.</summary>
        public string Str() { var n = (int)U32(); var v = Encoding.UTF8.GetString(data, _at, n); _at += n; return v; }

        /// <summary>A count-prefixed string list.</summary>
        public string[] Strs()
        {
            var v = new string[U32()];
            for (int i = 0; i < v.Length; i++) v[i] = Str();
            return v;
        }

        /// <summary>Phase timings.</summary>
        public CompileTiming Timing() => new(U32(), U32(), U32(), U32(), U32(), U64());

        /// <summary>Microseconds since a timestamp written with <see cref="Writer.Now"/>.</summary>
        public uint Since() => CompileTiming.Micros((long)U64(), Stopwatch.GetTimestamp());
    }
}

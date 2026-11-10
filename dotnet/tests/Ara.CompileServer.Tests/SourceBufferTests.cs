using System.Buffers;
using System.Text;

namespace Ara.CompileServer.Tests;

public sealed class SourceBufferTests
{
    [Fact]
    public void Small_reads_share_one_block_and_large_reads_keep_their_contents()
    {
        var bytes = Enumerable.Range(0, 12_000).Select(i => (byte)(i % 251)).ToArray();
        var inner = new ProbeStream(bytes);
        var pool = new TrackingPool();
        using (var stream = new PooledReadAheadStream(inner, pool))
        {
            var output = new byte[4096];
            for (var i = 0; i < 4; i++) Assert.Equal(1024, stream.Read(output, i * 1024, 1024));
            Assert.Equal(bytes[..4096], output);
            Assert.Equal([4096], inner.Requests);
            Assert.Equal(1, pool.Rents);
            var large = new byte[5000];
            Assert.Equal(large.Length, stream.Read(large));
            Assert.Equal(bytes[4096..9096], large);
            Assert.Equal(1, pool.Rents);
        }
        Assert.Equal(1, pool.Returns);
        Assert.Equal(1, inner.Disposals);
    }

    [Fact]
    public void Fragmented_bom_decoding_matches_the_unchanged_reader()
    {
        var text = new string('x', 4095) + "\U0001f600 café\r\n";
        Encoding[] encodings = [new UTF8Encoding(true), new UnicodeEncoding(false, true),
            new UnicodeEncoding(true, true), new UTF32Encoding(false, true), new UTF32Encoding(true, true)];
        foreach (var encoding in encodings)
            foreach (var cap in new[] { 1, 3, 4095 })
            {
                var bytes = encoding.GetPreamble().Concat(encoding.GetBytes(text)).ToArray();
                using var original = new StreamReader(new ProbeStream(bytes, cap));
                // Default StreamReader may not recognize split non-UTF8 BOMs.
                // Preserve its exact behavior rather than imposing a different decoder.
                var expected = original.ReadToEnd();
                using var pooled = new StreamReader(new PooledReadAheadStream(new ProbeStream(bytes, cap), ArrayPool<byte>.Shared));
                Assert.Equal(expected, pooled.ReadToEnd());
            }
    }

    [Fact]
    public void An_inner_disposal_failure_still_returns_the_lease_once()
    {
        var inner = new ProbeStream([1, 2, 3], failDispose: true);
        var pool = new TrackingPool();
        var stream = new PooledReadAheadStream(inner, pool);
        Assert.Equal(1, stream.Read(new byte[1]));
        Assert.Throws<IOException>(stream.Dispose);
        Assert.Equal(1, inner.Disposals);
        Assert.Equal(1, pool.Returns);
        stream.Dispose();
        Assert.Equal(1, pool.Returns);
    }

    sealed class TrackingPool : ArrayPool<byte>
    {
        internal int Rents, Returns;
        public override byte[] Rent(int minimumLength) { Rents++; return new byte[minimumLength]; }
        public override void Return(byte[] array, bool clearArray = false) => Returns++;
    }

    sealed class ProbeStream(byte[] bytes, int cap = int.MaxValue, bool failDispose = false) : Stream
    {
        readonly MemoryStream _inner = new(bytes);
        internal readonly List<int> Requests = [];
        internal int Disposals;
        public override bool CanRead => _inner.CanRead;
        public override bool CanSeek => true;
        public override bool CanWrite => false;
        public override long Length => _inner.Length;
        public override long Position { get => _inner.Position; set => throw new NotSupportedException(); }
        public override int Read(byte[] buffer, int offset, int count) => Read(buffer.AsSpan(offset, count));
        public override int Read(Span<byte> buffer)
        {
            Requests.Add(buffer.Length);
            return _inner.Read(buffer[..Math.Min(cap, buffer.Length)]);
        }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        protected override void Dispose(bool disposing)
        {
            Disposals++;
            _inner.Dispose();
            base.Dispose(disposing);
            if (failDispose) throw new IOException("injected disposal failure");
        }
    }
}

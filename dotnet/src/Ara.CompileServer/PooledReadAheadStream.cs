// Licensed to the .NET Foundation under one or more agreements.
// The .NET Foundation licenses this file to you under the MIT license.
// Read-ahead control flow adapted for an owned pooled reader; see THIRD_PARTY_NOTICES.md.

using System.Buffers;

namespace Ara.CompileServer;

/// <summary>Owned forward-only reader with a pooled 4 KiB read-ahead block.</summary>
/// <remarks>Private to synchronous source decoding; no callers seek or write this stream.</remarks>
internal sealed class PooledReadAheadStream : Stream
{
    internal const int BufferBytes = 4096;
    readonly Stream inner;
    readonly ArrayPool<byte> pool;
    byte[]? buffer;
    int start, end;
    bool disposed;

    // Ownership transfers on successful construction. The caller still owns the
    // raw FileStream using-scope, so constructor failures cannot leak its handle.
    internal PooledReadAheadStream(Stream inner, ArrayPool<byte> pool)
    {
        ArgumentNullException.ThrowIfNull(inner);
        ArgumentNullException.ThrowIfNull(pool);
        if (!inner.CanRead) throw new ArgumentException("Readable stream required", nameof(inner));
        this.inner = inner;
        this.pool = pool;
    }

    public override bool CanRead => !disposed && inner.CanRead;
    public override bool CanSeek => false;
    public override bool CanWrite => false;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }

    public override int Read(byte[] destination, int offset, int count)
    {
        ValidateBufferArguments(destination, offset, count);
        return Read(destination.AsSpan(offset, count));
    }

    public override int Read(Span<byte> destination)
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        var available = end - start;
        var shortFill = false;
        if (available == 0)
        {
            // Large requests and nonseekable devices retain direct-read behavior.
            if (!inner.CanSeek || destination.Length >= BufferBytes)
            {
                var direct = inner.Read(destination);
                start = end = 0;
                return direct;
            }
            buffer ??= pool.Rent(BufferBytes);
            available = inner.Read(buffer, 0, BufferBytes);
            if (available == 0) return 0;
            start = 0;
            end = available;
            shortFill = available < BufferBytes;
        }

        var copied = Math.Min(available, destination.Length);
        buffer.AsSpan(start, copied).CopyTo(destination);
        start += copied;
        // A newly short read returns immediately. A request crossing an existing
        // block may issue one direct remainder read, as the installed runtime does.
        if (inner.CanSeek && copied < destination.Length && !shortFill)
        {
            copied += inner.Read(destination[copied..]);
            start = end = 0;
        }
        return copied;
    }

    public override void Flush() => ObjectDisposedException.ThrowIf(disposed, this);
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();
    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        if (!disposing || disposed) { base.Dispose(disposing); return; }
        disposed = true;
        var lease = buffer;
        buffer = null;
        try { inner.Dispose(); }
        finally
        {
            try { if (lease is not null) pool.Return(lease); }
            finally { base.Dispose(disposing); }
        }
    }
}

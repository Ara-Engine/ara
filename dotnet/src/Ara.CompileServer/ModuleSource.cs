using System.Buffers;
using System.Diagnostics;
using System.Text;

namespace Ara.CompileServer;

/// <summary>Reads every decoded source unit, retaining only an exactly equal prior string.</summary>
internal static class ModuleSource
{
    internal const int BufferChars = 4096;

    /// <summary>Reads with editor sharing; a prior string avoids allocation only after exact decoded equality.</summary>
    internal static string? ReadWithRetry(string path, string? previous)
    {
        var sw = Stopwatch.StartNew();
        while (true)
        {
            try
            {
                if (!File.Exists(path)) return null;
                using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                using var reader = new StreamReader(stream);
                return ReadDecoded(reader, previous, ArrayPool<char>.Shared);
            }
            catch (IOException) when (sw.ElapsedMilliseconds < 200) { Thread.Yield(); }
        }
    }

    // The injected pool is only a test seam for proving return-on-failure. Callers
    // retain no reader, handle, prior-string chain or per-file global cache here.
    internal static string ReadDecoded(TextReader reader, string? previous, ArrayPool<char> pool)
    {
        if (previous is null) return reader.ReadToEnd();
        var buffer = pool.Rent(BufferChars);
        try
        {
            var matched = 0;
            StringBuilder? changed = null;
            while (true)
            {
                var count = reader.Read(buffer.AsSpan(0, BufferChars));
                if (count == 0)
                    return changed is not null ? changed.ToString()
                        : matched == previous.Length ? previous : previous[..matched];
                var actual = buffer.AsSpan(0, count);
                if (changed is not null)
                {
                    changed.Append(actual);
                    continue;
                }
                var comparable = Math.Min(count, previous.Length - matched);
                var expected = previous.AsSpan(matched, comparable);
                if (count == comparable && actual.SequenceEqual(expected))
                {
                    matched += count;
                    continue;
                }
                var equal = 0;
                while (equal < comparable && actual[equal] == expected[equal]) equal++;
                changed = new StringBuilder(Math.Max(count, 16));
                changed.Append(previous.AsSpan(0, matched + equal));
                changed.Append(actual[equal..]);
            }
        }
        finally { pool.Return(buffer); }
    }
}

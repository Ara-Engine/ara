using System.Security.Cryptography;
using System.Text;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class SourceHashTests
{
    [Fact]
    public void Captured_hash_matches_UTF8_SHA256_across_buffer_boundaries_and_arbitrary_UTF16()
    {
        var random = new Random(149);
        foreach (var length in new[] { 0, 1, 255, 1023, 1024, 1025, 4096, 65537 })
        {
            Check(new string('a', length));
            Check(new string(Enumerable.Range(0, length).Select(_ => (char)random.Next(65536)).ToArray()));
        }
        Check("é🦀\r\n\ud800unterminated\udfff");
        static void Check(string text) => Assert.Equal(
            Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text))), PublishedSnapshot.TextHash(text));
    }

    [Fact]
    public void Concurrent_large_sources_keep_independent_fingerprints()
    {
        var sources = Enumerable.Range(0, 16).Select(i => new string((char)('a' + i), 65537 + i)).ToArray();
        var expected = sources.Select(text => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text)))).ToArray();
        Parallel.For(0, 128, i => Assert.Equal(expected[i % sources.Length], PublishedSnapshot.TextHash(sources[i % sources.Length])));
    }
}

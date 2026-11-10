using System.Security.Cryptography;

namespace Ara.CompileServer.Tests;

public sealed class SdkHashTests
{
    [Fact]
    public void Streamed_SDK_identity_matches_whole_file_reads_and_releases_handles()
    {
        var root = InstalledTools.Scratch("sdk-hashes");
        foreach (var size in new[] { 0, 1, 4095, 4096, 4097, 65535, 65536, 65537, 1048576 })
        {
            var path = Path.Combine(root, $"café-日本-{size}.bin");
            File.WriteAllBytes(path, Enumerable.Range(0, size).Select(i => (byte)(i * 31 + 7)).ToArray());
            var expected = Original(path);
            Assert.Equal(expected, ProjectCompiler.HashFile(path));
            using (var writer = new FileStream(path, FileMode.Open, FileAccess.ReadWrite, FileShare.None))
            {
                var original = Record.Exception(() => Original(path));
                Assert.NotNull(original);
                Assert.Equal(original.GetType(), Record.Exception(() => ProjectCompiler.HashFile(path))?.GetType());
            }
            Assert.Equal(expected, ProjectCompiler.HashFile(path));
            File.Delete(path);
            Assert.False(File.Exists(path));
        }
        foreach (var path in new[] { root, Path.Combine(root, "missing.dll") })
        {
            var original = Record.Exception(() => Original(path));
            Assert.NotNull(original);
            Assert.Equal(original.GetType(), Record.Exception(() => ProjectCompiler.HashFile(path))?.GetType());
        }
    }

    [Fact]
    public void Installed_SDK_images_have_identical_streamed_fingerprints()
    {
        var directory = Path.GetDirectoryName(typeof(ProjectCompiler).Assembly.Location)!;
        foreach (var name in new[] { "Ara.CompileServer.dll", "Ara.Runtime.dll", "Ara.Generators.dll", "Ara.Editor.dll" })
        {
            var path = Path.Combine(directory, name);
            Assert.Equal(Original(path), ProjectCompiler.HashFile(path));
        }
    }

    static string Original(string path) => Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(path)));
}

using System.Diagnostics;
using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

var server = Path.GetFullPath(args[0]);
AssemblyLoadContext.Default.Resolving += (_, name) => {
    var path = Path.Combine(Path.GetDirectoryName(server)!, name.Name + ".dll");
    return File.Exists(path) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(server);
var type = assembly.GetType("Ara.Hosting.PublishedSnapshot", throwOnError: true)!;
var hash = type.GetMethod("TextHash", BindingFlags.NonPublic | BindingFlags.Static)!.CreateDelegate<Func<string, string>>();
var inputs = new Dictionary<string, string> {
    ["small_ascii"] = new('a', 384), ["small_unicode"] = string.Concat(Enumerable.Repeat("é🦀", 128)),
    ["boundary_1024"] = new('a', 1024), ["boundary_1025"] = new('a', 1025),
    ["medium_16KiB"] = new('a', 16384), ["large_512KiB"] = new('a', 524288)
};
var random = new Random(149);
foreach (var length in new[] { 0, 1, 255, 1023, 1024, 1025, 4096, 65537 }) {
    var text = new string(Enumerable.Range(0, length).Select(_ => (char)random.Next(65536)).ToArray());
    if (hash(text) != Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text)))) throw new Exception("UTF-16 hash mismatch");
}
var rows = new List<object>();
foreach (var (name, text) in inputs) {
    var expected = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text)));
    for (var i = 0; i < 32; i++) if (hash(text) != expected) throw new Exception("warmup hash mismatch");
    var count = Math.Max(20, 4 * 1024 * 1024 / Encoding.UTF8.GetByteCount(text));
    var samples = new List<object>();
    for (var sample = 0; sample < 20; sample++) {
        var bytes = GC.GetAllocatedBytesForCurrentThread();
        var start = Stopwatch.GetTimestamp();
        var value = "";
        for (var i = 0; i < count; i++) value = hash(text);
        var elapsed = Stopwatch.GetElapsedTime(start).TotalMilliseconds;
        var allocated = GC.GetAllocatedBytesForCurrentThread() - bytes;
        if (value != expected) throw new Exception("measured hash mismatch");
        samples.Add(new { count, milliseconds = elapsed, allocationBytes = allocated, bytesPerCall = (double)allocated / count });
    }
    rows.Add(new { name, utf8Bytes = Encoding.UTF8.GetByteCount(text), hash = expected, samples });
}
Console.WriteLine(JsonSerializer.Serialize(new { runtime = System.Environment.Version.ToString(), server, serverHash = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(server))), rows }));

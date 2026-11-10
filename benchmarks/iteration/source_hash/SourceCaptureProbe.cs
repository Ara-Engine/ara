using System.Collections;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

/// <summary>Counts allocations in the actual project-capture method; never measures clocks.</summary>
internal static class SourceCaptureProbe
{
    internal static void Run(Assembly assembly, string root)
    {
        var marker = Path.Combine(root, ".ara-source-capture-probe");
        const string ownership = "Ara source-capture allocation fixture v1\n";
        if (Directory.Exists(root) && Directory.EnumerateFileSystemEntries(root).Any()
            && (!File.Exists(marker) || File.ReadAllText(marker) != ownership))
            throw new ArgumentException("Use a fresh fixture directory or one owned by this probe.");
        Directory.CreateDirectory(root);
        File.WriteAllText(marker, ownership);
        var capture = assembly.GetType("Ara.CompileServer.ProjectSourceCapture", throwOnError: true)!;
        var fileType = assembly.GetType("Ara.CompileServer.SourceFile", throwOnError: true)!;
        var read = capture.GetMethod("Read", BindingFlags.Static | BindingFlags.NonPublic)!;
        var reusable = read.GetParameters().Length == 3;
        var results = new List<object>();
        foreach (var (units, count) in new[] { (384, 32), (16384, 32), (524288, 4) })
        foreach (var (scenario, edit) in new[] { ("cold", "none"), ("unchanged", "none") }
            .Concat(new[] { "early", "middle", "late", "replacement" }
                .SelectMany(edit => new[] { ("one_changed", edit), ("all_changed", edit) })))
        {
            var files = Array.CreateInstance(fileType, count);
            var prior = new Dictionary<string, string>(StringComparer.Ordinal);
            var expected = new Dictionary<string, (string Text, string Hash)>(StringComparer.Ordinal);
            var original = string.Concat(Enumerable.Repeat("é🦀漢字", units / 5)) + new string('a', units % 5);
            for (var i = 0; i < count; i++)
            {
                var storage = $"Source{i}.cs";
                var path = Path.Combine(root, storage);
                var changed = scenario == "all_changed" || scenario == "one_changed" && i == 0;
                var text = changed ? Change(original, edit) : original;
                File.WriteAllText(path, text);
                prior[path] = original;
                expected[path] = (text, Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(text))));
                files.SetValue(Activator.CreateInstance(fileType, storage, storage, "fixture", false), i);
            }
            Func<string, string?> previous = path => prior.GetValueOrDefault(path);
            object?[] parameters = reusable ? [root, files, scenario == "cold" ? null : previous] : [root, files];
            object Read() => read.Invoke(null, parameters)!;
            void Validate(object result)
            {
                var sources = (IEnumerable)capture.GetProperty("Sources", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(result)!;
                var seen = 0;
                foreach (var source in sources)
                {
                    var type = source.GetType();
                    var path = (string)type.GetProperty("Path")!.GetValue(source)!;
                    var text = (string)type.GetProperty("Text")!.GetValue(source)!;
                    var hash = (string)type.GetProperty("Hash")!.GetValue(source)!;
                    if (text != expected[path].Text || hash != expected[path].Hash)
                        throw new InvalidDataException("captured text/hash mismatch");
                    if (reusable && scenario != "cold" && expected[path].Text == prior[path] && !ReferenceEquals(text, prior[path]))
                        throw new InvalidDataException("unchanged source was not reused");
                    seen++;
                }
                if (seen != count) throw new InvalidDataException("incorrect capture inventory");
            }
            for (var warmup = 0; warmup < 8; warmup++) Validate(Read());
            var samples = new List<long>();
            for (var sample = 0; sample < 20; sample++)
            {
                var before = GC.GetAllocatedBytesForCurrentThread();
                var result = Read();
                var allocated = GC.GetAllocatedBytesForCurrentThread() - before;
                Validate(result); // Reflection, expected hashing and file writes stay outside counters.
                samples.Add(allocated);
            }
            results.Add(new { utf16UnitsPerFile = original.Length, utf8BytesPerFile = Encoding.UTF8.GetByteCount(original),
                files = count, scenario, edit, allocatedBytesPerSet = samples });
        }
        Console.WriteLine(JsonSerializer.Serialize(new { runtime = Environment.Version.ToString(), reusable, rows = results }));
    }

    static string Change(string original, string edit)
    {
        if (edit == "replacement")
            return string.Concat(Enumerable.Repeat("ø🐚文語", original.Length / 5)) + new string('b', original.Length % 5);
        var index = edit switch { "early" => 0, "middle" => original.Length / 2, "late" => original.Length - 1,
            _ => throw new ArgumentException("unknown edit") };
        while (char.IsSurrogate(original[index])) index++;
        var unit = original[index] switch { 'é' => 'ø', '漢' => '文', '字' => '語', _ => 'b' };
        return original[..index] + unit + original[(index + 1)..];
    }
}

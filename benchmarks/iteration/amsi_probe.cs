// What does the operating system's scan of a freshly emitted module image cost, and what drives it? (Windows)
//
//   1. Dump real images from a bench run:   $env:ARA_DUMP_IMAGES = "C:\amsi-images"
//        cargo run -p ara-script --release --example reload_bench -- --edits 12
//      (or `dotnet run benchmarks/iteration/reload_timing.cs -- --runs 1 --edits 12` with the variable set)
//   2. dotnet run benchmarks/iteration/amsi_probe.cs -- --images C:\amsi-images
//
// L3 reloads load a new image with AssemblyLoadContext.LoadFromStream, which on Windows has the antivirus scan the bytes through
// AMSI (measured: ~145 ms of a 317 ms L3 for a 5.2k-line module). This probe only MEASURES, with the documented AMSI client API
// (AmsiScanBuffer, the call the runtime itself makes), and never alters, skips or works around a scan. It answers:
//   A. How long does the scan of the image take, and how does that change with the bytes? Each iteration scans a copy whose PE
//      timestamp differs, so no result is served from a content cache. Variants of the same image: as emitted; method bodies
//      zeroed; string heaps zeroed; blob heap zeroed; all metadata zeroed; first half only; random bytes of the same length.
//   B. Which way of loading costs what, with unique bytes each time: LoadFromStream (what the host does), LoadFromAssemblyPath
//      from a file in the temp folder, in %LOCALAPPDATA%\Ara\amsi-probe, and in the current folder, and a file written two
//      seconds before it is loaded (does a scan on write overlap with other work?). And the same bytes loaded twice (cache).
// Prints median / p95 in ms per row and writes amsi_probe.json next to the images. Needs only the .NET 10 SDK.
using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Text.Json;
using System.Text.Json.Nodes;

var options = args.ToList();
string? Option(string name) => options.IndexOf(name) is var i and >= 0 && i + 1 < options.Count ? options[i + 1] : null;
var folders = options.Select((value, i) => (value, i)).Where(x => x.value == "--images" && x.i + 1 < options.Count).Select(x => options[x.i + 1]).ToArray();
if (folders.Length == 0) throw new ArgumentException("usage: amsi_probe.cs -- --images <folder with the dumped .dll files> [--images <another folder>] [--iterations 15]");
var imagesFolder = folders[0];
var iterations = int.Parse(Option("--iterations") ?? "15");
// Several folders (images dumped from runs with different --lines) give the scan time against module size.
var images = folders.SelectMany(folder => Directory.GetFiles(folder, "*.dll")).OrderByDescending(path => new FileInfo(path).Length).ToArray();
if (images.Length == 0) throw new FileNotFoundException("no .dll files in " + imagesFolder);
var imagePath = images[0]; // the biggest: an L3 image of the whole module
var original = File.ReadAllBytes(imagePath);
Console.WriteLine($"{RuntimeInformation.OSDescription} | {RuntimeInformation.ProcessArchitecture} | dotnet {Environment.Version}");
Console.WriteLine($"image: {Path.GetFileName(imagePath)}, {original.Length:N0} bytes ({images.Length} dumped); {iterations} iterations per row (+2 warm-up)");
Console.WriteLine($"Defender real-time protection: {Defender()}");

var rows = new List<(string Group, string Name, double[] Ms, string Note)>();

// The variants are built on every OS so their construction can be checked without a scanner. Each variant is checked here: it
// must have the image's length (the half image, half of it) and change no more bytes than the ranges it names plus the 4-byte
// timestamp. A variant that fails the check would measure the wrong thing, so the probe stops.
var variants = Variants(original);
foreach (var (name, make, namedBytes) in variants)
{
    var made = make(1);
    var comparable = Math.Min(made.Length, original.Length);
    var changed = Enumerable.Range(0, comparable).Count(i => made[i] != original[i]);
    var expectedLength = name.StartsWith("first half", StringComparison.Ordinal) ? original.Length / 2 : original.Length;
    var ok = made.Length == expectedLength && (name.StartsWith("random", StringComparison.Ordinal) || changed <= namedBytes + 4);
    Console.WriteLine($"  variant {name,-46} {made.Length,9:N0} bytes, {changed,9:N0} differ from the image {(ok ? "" : "  <-- SELF-CHECK FAILED")}");
    if (!ok) throw new InvalidOperationException($"variant '{name}' does not change what it says it changes");
    if (name.StartsWith("VALID", StringComparison.Ordinal))
    {
        // A VALID variant must still be a loadable assembly with readable metadata and bodies, or it measures a broken file.
        using (var reader = new PEReader(new MemoryStream(made)))
        {
            var md = reader.GetMetadataReader();
            foreach (var method in md.MethodDefinitions)
                if (md.GetMethodDefinition(method).RelativeVirtualAddress is var rva and > 0) _ = reader.GetMethodBody(rva).GetILBytes();
        }
        var context = new AssemblyLoadContext("amsi-probe-check", isCollectible: true);
        try { context.LoadFromStream(new MemoryStream(made)); }
        finally { context.Unload(); }
    }
}

// ---- A. the scan itself, by content
if (OperatingSystem.IsWindows())
{
    using var amsi = Amsi.Open("AraAmsiProbe");
    if (amsi is null) Console.WriteLine("AMSI is not available (no provider registered): the scan rows are skipped.");
    else
    {
        foreach (var (name, make, _) in variants)
        {
            var times = new List<double>();
            var results = new HashSet<int>();
            for (var i = -2; i < iterations; i++)
            {
                var bytes = make(i + 3);
                var sw = Stopwatch.StartNew();
                var result = amsi.Scan(bytes, name);
                if (i >= 0) times.Add(sw.Elapsed.TotalMilliseconds);
                results.Add(result);
            }
            rows.Add(("scan", name, [.. times], "AMSI_RESULT " + string.Join(",", results)));
        }
        // Every dumped image as emitted: Release-IL (cold) against Debug-IL (hot) images, and module size against scan time.
        foreach (var path in images.OrderBy(path => new FileInfo(path).Length))
        {
            var bytes = File.ReadAllBytes(path);
            var times = new List<double>();
            for (var i = -1; i < Math.Min(iterations, 8); i++)
            {
                var copy = (byte[])bytes.Clone();
                StampUnique(copy, 700 + i);
                var sw = Stopwatch.StartNew();
                amsi.Scan(copy, "image");
                if (i >= 0) times.Add(sw.Elapsed.TotalMilliseconds);
            }
            rows.Add(("image", $"{Path.GetFileName(Path.GetDirectoryName(path))}/{Path.GetFileName(path)} ({bytes.Length:N0} bytes)", [.. times], "as emitted"));
        }
    }
}
else Console.WriteLine("Not Windows: AMSI rows skipped (the load rows still run, for comparison).");

// ---- B. loading
double Load(Func<AssemblyLoadContext, Assembly> load)
{
    var context = new AssemblyLoadContext("amsi-probe", isCollectible: true);
    var sw = Stopwatch.StartNew();
    try { load(context); }
    finally { context.Unload(); }
    return sw.Elapsed.TotalMilliseconds;
}

byte[] Unique(int n) { var copy = (byte[])original.Clone(); StampUnique(copy, n); return copy; }

var stream = new List<double>();
for (var i = -2; i < iterations; i++)
{
    var bytes = Unique(1000 + i);
    var ms = Load(context => context.LoadFromStream(new MemoryStream(bytes)));
    if (i >= 0) stream.Add(ms);
}
rows.Add(("load", "LoadFromStream, unique bytes (what the host does)", [.. stream], ""));

var twice = new List<double>();
for (var i = 0; i < iterations; i++)
{
    var bytes = Unique(5000 + i);
    Load(context => context.LoadFromStream(new MemoryStream(bytes)));
    twice.Add(Load(context => context.LoadFromStream(new MemoryStream(bytes))));
}
rows.Add(("load", "LoadFromStream, the same bytes a second time", [.. twice], "a content cache, if there is one"));

foreach (var (label, folder) in new[]
{
    ("temp folder", Path.Combine(Path.GetTempPath(), "ara-amsi-probe")),
    ("%LOCALAPPDATA%\\Ara\\amsi-probe", Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Ara", "amsi-probe")),
    ("current folder", Path.Combine(Directory.GetCurrentDirectory(), "amsi-probe-files")),
})
{
    Directory.CreateDirectory(folder);
    var write = new List<double>();
    var load = new List<double>();
    for (var i = -2; i < iterations; i++)
    {
        var path = Path.Combine(folder, $"probe-{Environment.ProcessId}-{i + 10}.dll");
        var bytes = Unique(2000 + i);
        var sw = Stopwatch.StartNew();
        File.WriteAllBytes(path, bytes);
        var written = sw.Elapsed.TotalMilliseconds;
        var ms = Load(context => context.LoadFromAssemblyPath(path));
        if (i >= 0) { write.Add(written); load.Add(ms); }
        TryDelete(path);
    }
    rows.Add(("file", $"write file, {label}", [.. write], ""));
    rows.Add(("file", $"LoadFromAssemblyPath, {label}", [.. load], "load only, file freshly written"));
}

{
    var folder = Path.Combine(Path.GetTempPath(), "ara-amsi-probe");
    var load = new List<double>();
    for (var i = -1; i < Math.Min(iterations, 8); i++)
    {
        var path = Path.Combine(folder, $"early-{Environment.ProcessId}-{i + 10}.dll");
        File.WriteAllBytes(path, Unique(3000 + i));
        Thread.Sleep(2000); // the time an eager writer would have before the image is needed
        var ms = Load(context => context.LoadFromAssemblyPath(path));
        if (i >= 0) load.Add(ms);
        TryDelete(path);
    }
    rows.Add(("file", "LoadFromAssemblyPath, file written 2 s earlier", [.. load], "does a scan on write overlap with other work?"));
}

Console.WriteLine();
Console.WriteLine($"{"group",-6}{"row",-62}{"median",9}{"p95",9}  note");
foreach (var (group, name, ms, note) in rows)
    Console.WriteLine($"{group,-6}{name,-62}{Median(ms),9:F1}{Percentile(ms, 0.95),9:F1}  {note}");
var json = new JsonObject
{
    ["os"] = RuntimeInformation.OSDescription, ["image"] = Path.GetFileName(imagePath), ["bytes"] = original.Length, ["iterations"] = iterations,
    ["rows"] = new JsonArray(rows.Select(r => (JsonNode)new JsonObject
    {
        ["group"] = r.Group, ["name"] = r.Name, ["median_ms"] = Median(r.Ms), ["p95_ms"] = Percentile(r.Ms, 0.95), ["note"] = r.Note,
        ["samples_ms"] = new JsonArray(r.Ms.Select(v => (JsonNode)JsonValue.Create(v)).ToArray()),
    }).ToArray()),
};
File.WriteAllText(Path.Combine(imagesFolder, "amsi_probe.json"), json.ToJsonString(new JsonSerializerOptions { WriteIndented = true }));
Console.WriteLine($"\nwritten: {Path.Combine(imagesFolder, "amsi_probe.json")}");

static double Median(double[] v) => Percentile(v, 0.5);
static double Percentile(double[] v, double p) => v.Length == 0 ? double.NaN : v.Order().ElementAt((int)Math.Round((v.Length - 1) * p));
static void TryDelete(string path) { try { File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { } }

/// <summary>A different COFF timestamp makes the bytes (and any content hash) differ without changing the code.</summary>
static void StampUnique(byte[] image, int n)
{
    var peOffset = BitConverter.ToInt32(image, 0x3C);
    BitConverter.GetBytes(0x5A000000 + n * 7919).CopyTo(image, peOffset + 8);
}

/// <summary>
/// Variants of the image that blank out one kind of content at a time (plus the first half and random bytes), each with a unique
/// stamp; the last element is how many bytes the variant is allowed to change besides the stamp.
/// </summary>
static List<(string Name, Func<int, byte[]> Make, long NamedBytes)> Variants(byte[] original)
{
    using var pe = new PEReader(new MemoryStream(original));
    var metadata = pe.GetMetadataReader();
    var headers = pe.PEHeaders;
    int FileOffset(int rva)
    {
        var section = headers.SectionHeaders.First(s => rva >= s.VirtualAddress && rva < s.VirtualAddress + Math.Max(s.VirtualSize, s.SizeOfRawData));
        return rva - section.VirtualAddress + section.PointerToRawData;
    }
    var bodies = new List<(int Offset, int Length)>();
    foreach (var handle in metadata.MethodDefinitions)
    {
        var rva = metadata.GetMethodDefinition(handle).RelativeVirtualAddress;
        if (rva == 0) continue;
        bodies.Add((FileOffset(rva), pe.GetMethodBody(rva).Size));
    }
    (int Offset, int Length) Heap(HeapIndex heap) => (headers.MetadataStartOffset + metadata.GetHeapMetadataOffset(heap), metadata.GetHeapSize(heap));
    var strings = new[] { Heap(HeapIndex.String), Heap(HeapIndex.UserString) };
    var blob = new[] { Heap(HeapIndex.Blob) };
    var whole = new[] { (headers.MetadataStartOffset, headers.MetadataSize) };

    Func<int, byte[]> Zero(IEnumerable<(int Offset, int Length)> ranges) => n =>
    {
        var copy = (byte[])original.Clone();
        foreach (var (offset, length) in ranges) Array.Clear(copy, offset, Math.Min(length, copy.Length - offset));
        StampUnique(copy, n);
        return copy;
    };
    // The user-string literals, read now (the reader is disposed when this method returns): where each one starts in the image,
    // the width of its length prefix, its length in bytes, and whether it is one of the generated {json} descriptors.
    var literals = new List<(int At, int LengthBytes, int Chars, bool Json)>();
    {
        var heap = Heap(HeapIndex.UserString);
        var handle = MetadataTokens.UserStringHandle(1);
        while (!handle.IsNil)
        {
            var text = metadata.GetUserString(handle);
            var at = heap.Offset + MetadataTokens.GetHeapOffset(handle);
            var lengthBytes = (original[at] & 0x80) == 0 ? 1 : (original[at] & 0xC0) == 0x80 ? 2 : 4;
            literals.Add((at, lengthBytes, text.Length * 2, text.StartsWith('{')));
            handle = metadata.GetNextHandle(handle);
        }
    }

    // Valid-structure variants: the metadata and IL still parse, so a scanner that gives up on a broken image (the zeroed rows
    // above) still has to do its normal work. Same size as the image, so only the content differs.
    byte[] Mutate(int n, bool nopIl, Func<bool, bool>? blankLiteral)
    {
        var copy = (byte[])original.Clone();
        if (nopIl)
            foreach (var (offset, _) in bodies)
            {
                var first = copy[offset];
                int start, size;
                if ((first & 3) == 2) { start = offset + 1; size = first >> 2; }
                else { start = offset + (BitConverter.ToUInt16(copy, offset) >> 12) * 4; size = BitConverter.ToInt32(copy, offset + 4); }
                if (size <= 0) continue;
                Array.Fill(copy, (byte)0x00, start, size); // nop
                copy[start + size - 1] = 0x2A;             // ret
            }
        if (blankLiteral is not null)
            foreach (var (at, lengthBytes, chars, json) in literals)
                if (blankLiteral(json))
                    for (var i = 0; i + 1 < chars; i += 2) { copy[at + lengthBytes + i] = (byte)'a'; copy[at + lengthBytes + i + 1] = 0; }
        StampUnique(copy, n);
        return copy;
    }
    var jsonBytes = literals.Where(l => l.Json).Sum(l => (long)l.Chars);
    var literalBytes = (long)metadata.GetHeapSize(HeapIndex.UserString);
    var ilBytes = bodies.Sum(b => (long)b.Length);

    return
    [
        ("as emitted", n => { var copy = (byte[])original.Clone(); StampUnique(copy, n); return copy; }, 0),
        ($"method bodies zeroed ({ilBytes:N0} bytes)", Zero(bodies), ilBytes),
        ($"string heaps zeroed ({strings.Sum(s => (long)s.Length):N0} bytes)", Zero(strings), strings.Sum(s => (long)s.Length)),
        ($"blob heap zeroed ({blob.Sum(s => (long)s.Length):N0} bytes)", Zero(blob), blob.Sum(s => (long)s.Length)),
        ($"all metadata zeroed ({whole.Sum(s => (long)s.Item2):N0} bytes)", Zero(whole), whole.Sum(s => (long)s.Item2)),
        ("first half only", n => { var copy = original[..(original.Length / 2)]; StampUnique(copy, n); return copy; }, 0),
        ("random bytes, same length", n => { var copy = new byte[original.Length]; new Random(n).NextBytes(copy); return copy; }, original.Length),
        ($"VALID: IL replaced by nops + ret ({ilBytes:N0} bytes)", n => Mutate(n, true, null), ilBytes),
        ($"VALID: all string literals blanked ({literalBytes:N0} bytes)", n => Mutate(n, false, _ => true), literalBytes),
        ($"VALID: only the {{json}} literals blanked ({jsonBytes:N0} bytes)", n => Mutate(n, false, json => json), jsonBytes),
        ($"VALID: IL nopped and all literals blanked", n => Mutate(n, true, _ => true), ilBytes + literalBytes),
    ];
}

static string Defender()
{
    try
    {
        var info = new ProcessStartInfo("powershell", "-NoProfile -Command \"(Get-MpComputerStatus).RealTimeProtectionEnabled\"")
        { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
        using var process = Process.Start(info)!;
        var text = process.StandardOutput.ReadToEnd().Trim();
        process.WaitForExit();
        return text.Length > 0 ? text : "unknown";
    }
    catch (Exception) { return "unknown"; }
}

/// <summary>The documented AMSI client API, used only to time a scan (the runtime makes the same call on an in-memory load).</summary>
sealed class Amsi : IDisposable
{
    readonly nint _context;
    readonly nint _session;

    Amsi(nint context, nint session) => (_context, _session) = (context, session);

    internal static Amsi? Open(string appName)
    {
        try
        {
            if (AmsiInitialize(appName, out var context) != 0) return null;
            return AmsiOpenSession(context, out var session) != 0 ? null : new Amsi(context, session);
        }
        catch (DllNotFoundException) { return null; }
    }

    /// <summary>The AMSI_RESULT of scanning <paramref name="bytes"/>.</summary>
    internal int Scan(byte[] bytes, string name)
    {
        var hr = AmsiScanBuffer(_context, bytes, (uint)bytes.Length, name, _session, out var result);
        return hr == 0 ? result : -1;
    }

    public void Dispose()
    {
        AmsiCloseSession(_context, _session);
        AmsiUninitialize(_context);
    }

    [DllImport("amsi.dll")] static extern int AmsiInitialize([MarshalAs(UnmanagedType.LPWStr)] string appName, out nint amsiContext);
    [DllImport("amsi.dll")] static extern void AmsiUninitialize(nint amsiContext);
    [DllImport("amsi.dll")] static extern int AmsiOpenSession(nint amsiContext, out nint amsiSession);
    [DllImport("amsi.dll")] static extern void AmsiCloseSession(nint amsiContext, nint amsiSession);
    [DllImport("amsi.dll")] static extern int AmsiScanBuffer(nint amsiContext, byte[] buffer, uint length, [MarshalAs(UnmanagedType.LPWStr)] string contentName, nint amsiSession, out int result);
}

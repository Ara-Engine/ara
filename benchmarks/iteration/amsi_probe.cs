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
var imagesFolder = Option("--images") ?? throw new ArgumentException("usage: amsi_probe.cs -- --images <folder with the dumped .dll files> [--iterations 15]");
var iterations = int.Parse(Option("--iterations") ?? "15");
var images = Directory.GetFiles(imagesFolder, "*.dll").OrderByDescending(path => new FileInfo(path).Length).ToArray();
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
    return
    [
        ("as emitted", n => { var copy = (byte[])original.Clone(); StampUnique(copy, n); return copy; }, 0),
        ($"method bodies zeroed ({bodies.Sum(b => (long)b.Length):N0} bytes)", Zero(bodies), bodies.Sum(b => (long)b.Length)),
        ($"string heaps zeroed ({strings.Sum(s => (long)s.Length):N0} bytes)", Zero(strings), strings.Sum(s => (long)s.Length)),
        ($"blob heap zeroed ({blob.Sum(s => (long)s.Length):N0} bytes)", Zero(blob), blob.Sum(s => (long)s.Length)),
        ($"all metadata zeroed ({whole.Sum(s => (long)s.Item2):N0} bytes)", Zero(whole), whole.Sum(s => (long)s.Item2)),
        ("first half only", n => { var copy = original[..(original.Length / 2)]; StampUnique(copy, n); return copy; }, 0),
        ("random bytes, same length", n => { var copy = new byte[original.Length]; new Random(n).NextBytes(copy); return copy; }, original.Length),
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

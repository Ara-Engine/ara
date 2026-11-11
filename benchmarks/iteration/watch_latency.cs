// How late does the OS file watcher report a save? Run on each platform:
//
//   dotnet run benchmarks/iteration/watch_latency.cs
//   dotnet run benchmarks/iteration/watch_latency.cs -- --saves 100
//
// It watches a scratch folder with the same FileSystemWatcher settings the compile server uses (recursive, LastWrite |
// FileName | DirectoryName | Size) and saves a source file two ways, timing each from just before the write to the first
// event for that file: an in-place save (truncate and write, as most editors do) and an atomic save (write a temp file,
// rename it over the source). The compile server then adds a 1 ms settle before it reads the file.
// macOS (FSEvents) measured ~11 ms median for in-place saves; Windows and Linux are expected to be about 1 ms or less.
// Needs only the .NET 10 SDK. Prints median / p95 / max in milliseconds and the OS.
using System.Diagnostics;
using System.Runtime.InteropServices;

var saves = 60;
for (var i = 0; i < args.Length; i++)
    if (args[i] == "--saves" && i + 1 < args.Length) saves = int.Parse(args[++i]);

var dir = Path.Combine(Path.GetTempPath(), "ara-watch-latency-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(dir);
var file = Path.Combine(dir, "Probe.cs");
File.WriteAllText(file, "class Probe { }");
var first = 0L;
var signal = new ManualResetEventSlim();
var watcher = new FileSystemWatcher(dir, "*")
{
    IncludeSubdirectories = true,
    NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.Size,
    InternalBufferSize = 64 * 1024,
};
void OnEvent(object? _, FileSystemEventArgs e)
{
    if (Path.GetFileName(e.FullPath) != "Probe.cs") return;
    if (Interlocked.CompareExchange(ref first, Stopwatch.GetTimestamp(), 0) == 0) signal.Set();
}
watcher.Changed += OnEvent;
watcher.Created += OnEvent;
watcher.Renamed += (_, e) => OnEvent(null, e);
watcher.EnableRaisingEvents = true;
Thread.Sleep(500); // the OS stream starts asynchronously on some platforms

double Measure(Action save)
{
    Thread.Sleep(120); // let the previous save's trailing events pass
    Interlocked.Exchange(ref first, 0);
    signal.Reset();
    var start = Stopwatch.GetTimestamp();
    save();
    if (!signal.Wait(3000)) return double.NaN;
    return Stopwatch.GetElapsedTime(start, Volatile.Read(ref first)).TotalMilliseconds;
}

var inPlace = new List<double>();
var atomic = new List<double>();
for (var i = 0; i < saves; i++)
{
    inPlace.Add(Measure(() => File.WriteAllText(file, $"class Probe {{ int v{i}; }}")));
    atomic.Add(Measure(() =>
    {
        var temp = file + ".tmp";
        File.WriteAllText(temp, $"class Probe {{ int a{i}; }}");
        File.Move(temp, file, overwrite: true);
    }));
}
watcher.Dispose();
Directory.Delete(dir, recursive: true);

static string Summary(List<double> values)
{
    var seen = values.Where(v => !double.IsNaN(v)).Order().ToArray();
    if (seen.Length == 0) return "no events";
    var missed = values.Count - seen.Length;
    return $"median {seen[seen.Length / 2]:F2} ms   p95 {seen[(int)Math.Round((seen.Length - 1) * 0.95)]:F2} ms   max {seen[^1]:F2} ms   ({seen.Length} saves{(missed > 0 ? $", {missed} MISSED" : "")})";
}

Console.WriteLine($"{RuntimeInformation.OSDescription} | {RuntimeInformation.ProcessArchitecture} | dotnet {Environment.Version}");
Console.WriteLine($"in-place save: {Summary(inPlace)}");
Console.WriteLine($"atomic save:   {Summary(atomic)}");

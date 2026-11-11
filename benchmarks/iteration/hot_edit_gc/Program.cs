using Ara.CompileServer;
using Ara.Hosting;
using System.Collections.Immutable;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Security.Cryptography;
using System.Text.Json;

var root = Path.GetFullPath(args[0]);
if (Directory.Exists(root)) throw new IOException("fixture must be fresh");
Directory.CreateDirectory(root);
var files = int.Parse(args[1], System.Globalization.CultureInfo.InvariantCulture);
var all = args.Length > 2 && args[2] == "all";
var source = Path.Combine(root, "Logic.cs");
for (var file = 0; file < files; file++)
    File.WriteAllText(Path.Combine(root, $"Extra{file:D4}.cs"), Extra(file, 0, all));
File.WriteAllText(source, Source(0));
var context = new Context();
var module = new ModuleSession(0, "Game", root, true, [typeof(Ara.Behaviour).Assembly.Location], [], "gameplay", new AnalyzerSet());
var initial = module.Initial(context);
if (initial.Kind != OutcomeKind.Image || initial.Image is null || initial.Problems.Any(d => d.Severity == Microsoft.CodeAnalysis.DiagnosticSeverity.Error))
    throw new InvalidDataException("initial compilation failed");
var initialPe = Hash(initial.Image.Pe); var initialPdb = Hash(initial.Image.Pdb);
module.Commit(initial);
var rows = new List<object>();
for (var revision = 1; revision <= 25; revision++)
{
    File.WriteAllText(source, Source(revision));
    var paths = new List<string> { source };
    if (all)
        for (var file = 0; file < files; file++)
        {
            var path = Path.Combine(root, $"Extra{file:D4}.cs");
            File.WriteAllText(path, Extra(file, revision, all));
            paths.Add(path);
        }
    var start = GC.GetTotalAllocatedBytes(precise: true);
    var outcome = module.Compile(paths, 0, context);
    module.Commit(outcome);
    var allocated = GC.GetTotalAllocatedBytes(precise: true) - start;
    if (outcome.Kind != OutcomeKind.Delta || outcome.ReloadKind != AbiConst.ReloadL1 || outcome.Delta is not { } delta
        || outcome.Problems.Count != 0 || module.State!.Generation != 0 || delta.Il.Length == 0 || delta.Metadata.Length == 0 || delta.Pdb.Length == 0)
        throw new InvalidDataException("expected clean nonempty L1 candidate with stable generation");
    using var metadata = MetadataReaderProvider.FromMetadataImage(ImmutableArray.Create(delta.Metadata));
    var fields = metadata.GetMetadataReader().GetTableRowCount(TableIndex.Field);
    if (fields != 0) throw new InvalidDataException("ordinary body edit added a field");
    rows.Add(new { revision, warmup = revision <= 5, allocatedBytes = allocated, ilSha256 = Hash(delta.Il), metadataSha256 = Hash(delta.Metadata), pdbSha256 = Hash(delta.Pdb), fields });
}
Console.WriteLine(JsonSerializer.Serialize(new
{
    runtime = System.Environment.Version.ToString(), files, all, initialPe, initialPdb, rows,
    timings_accepted = false,
    scope = "Direct production ModuleSession.Compile+Commit allocation; five warmups/twenty measured L1 candidate deltas, full source reads and policy checks. No runtime ApplyUpdate or latency conclusion. EnC metadata/PDB identities can differ; initial PE/PDB and per-request IL are compared across arms."
}));

static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));
static string Extra(int file, int revision, bool all) => all
    ? $"namespace Game; public static partial class Logic {{ public static int Read{file}() => {revision + file}; }}"
    : $"public static class Extra{file} {{ public static int Read() => {file}; }}";
static string Source(int revision) => $$"""
    namespace Game;
    /// <summary>State.</summary>
    [Component] public partial struct State { /// <summary>Value.</summary>
        public int Value;
    }
    [Simulation] public static partial class Logic {
        [Query] static void Run(ref State state) { state.Value += {{revision + 1}}; }
    }
    """;

sealed class Context : ICompileContext
{
    public bool Enc => true;
    public EncCapabilities Caps => EncCapabilities.BodiesOnly;
    public ModuleState? Dependency(string name) => null;
    public bool HasDependents(ModuleSession module) => false;
}

using System.Collections.Immutable;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.Hosting;

/// <summary>A full module image: PE plus portable PDB.</summary>
internal sealed record ModuleImage(byte[] Pe, byte[] Pdb);

/// <summary>An EnC delta for <c>MetadataUpdater.ApplyUpdate</c>, plus the baseline for the next generation.</summary>
internal sealed record ModuleDelta(byte[] Metadata, byte[] Il, byte[] Pdb, EmitBaseline Next);

/// <summary>
/// In-process Roslyn for one gameplay module. Holds the reference set and options; compilations are
/// retained by <see cref="ModuleHost"/> and updated with <c>ReplaceSyntaxTree</c>, so unchanged files
/// keep their parse and declaration work.
/// </summary>
internal sealed class ModuleCompiler
{
    internal const string AssemblyName = "Game";
    static readonly Guid EncLocalSlotMap = new("755F52A8-91C5-45BE-B4B8-209571E552BD");
    static readonly Guid EncLambdaAndClosureMap = new("A643004C-0240-496F-A783-30D64F4979DE");
    static readonly Guid EncStateMachineStateMap = new("8B78CD68-2EDE-420B-980B-E15884B8AAA3");

    internal readonly CSharpParseOptions ParseOptions = new(LanguageVersion.Latest, DocumentationMode.None);
    internal readonly CSharpCompilationOptions Options;
    internal readonly ImmutableArray<MetadataReference> References;
    internal readonly string ReferenceSource;

    internal ModuleCompiler(bool debugIL)
    {
        Options = new CSharpCompilationOptions(
            OutputKind.DynamicallyLinkedLibrary,
            optimizationLevel: debugIL ? OptimizationLevel.Debug : OptimizationLevel.Release,
            allowUnsafe: true,
            concurrentBuild: Environment.GetEnvironmentVariable("ARA_CONCURRENT_BUILD") != "0",
            nullableContextOptions: NullableContextOptions.Enable);
        (References, ReferenceSource) = LoadReferences();
    }

    /// <summary>Framework reference assemblies (the ref pack when installed, else the runtime's TPA) plus Ara.Runtime.</summary>
    static (ImmutableArray<MetadataReference>, string) LoadReferences()
    {
        var runtimeDir = Path.GetDirectoryName(typeof(object).Assembly.Location)!;    // ...\shared\Microsoft.NETCore.App\10.0.x
        var dotnetRoot = Path.GetFullPath(Path.Combine(runtimeDir, "..", "..", ".."));
        var refRoot = Path.Combine(dotnetRoot, "packs", "Microsoft.NETCore.App.Ref");
        var major = Environment.Version.Major;
        var refDir = Directory.Exists(refRoot)
            ? Directory.GetDirectories(refRoot, $"{major}.*")
                .OrderByDescending(d => Version.TryParse(Path.GetFileName(d), out var v) ? v : new Version())
                .Select(d => Path.Combine(d, "ref", $"net{major}.0"))
                .FirstOrDefault(Directory.Exists)
            : null;
        IEnumerable<string> files = refDir is not null
            ? Directory.GetFiles(refDir, "*.dll")
            : ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!).Split(Path.PathSeparator)
                .Where(p => Path.GetFileName(p).StartsWith("System.", StringComparison.Ordinal) || Path.GetFileName(p) is "netstandard.dll" or "mscorlib.dll");
        var refs = files.Select(f => (MetadataReference)MetadataReference.CreateFromFile(f)).ToList();
        refs.Add(MetadataReference.CreateFromFile(typeof(Host).Assembly.Location));
        return ([.. refs], refDir ?? "TPA");
    }

    internal SyntaxTree Parse(string path, string text) =>
        CSharpSyntaxTree.ParseText(text, ParseOptions, path, Encoding.UTF8);

    internal CSharpCompilation Create(IEnumerable<SyntaxTree> trees) =>
        CSharpCompilation.Create(AssemblyName, trees, References, Options);

    /// <summary>Emits the whole module. Returns null and the errors on failure.</summary>
    internal static (ModuleImage? image, ImmutableArray<Diagnostic> errors) Emit(CSharpCompilation c)
    {
        using var pe = new MemoryStream();
        using var pdb = new MemoryStream();
        var r = c.Emit(pe, pdb, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.PortablePdb));
        return r.Success
            ? (new ModuleImage(pe.ToArray(), pdb.ToArray()), [])
            : (null, [.. r.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error)]);
    }

    /// <summary>Creates the EnC baseline for a freshly emitted image, reading EnC maps from its PDB.</summary>
    internal static EmitBaseline CreateBaseline(CSharpCompilation c, ModuleImage image)
    {
        var module = ModuleMetadata.CreateFromImage(image.Pe);
        var peReader = new PEReader(ImmutableArray.Create(image.Pe));
        var peMeta = peReader.GetMetadataReader();
        var pdbMeta = MetadataReaderProvider.FromPortablePdbImage(ImmutableArray.Create(image.Pdb)).GetMetadataReader();
        return EmitBaseline.CreateInitialBaseline(
            c,
            module,
            h => EncInfo(pdbMeta, h),
            h =>
            {
                var rva = peMeta.GetMethodDefinition(h).RelativeVirtualAddress;
                return rva == 0 ? default : peReader.GetMethodBody(rva).LocalSignature;
            },
            hasPortableDebugInformation: true);
    }

    static EditAndContinueMethodDebugInformation EncInfo(MetadataReader pdb, MethodDefinitionHandle h)
    {
        ImmutableArray<byte> slots = default, lambdas = default, states = default;
        foreach (var ch in pdb.GetCustomDebugInformation(h))
        {
            var cdi = pdb.GetCustomDebugInformation(ch);
            var kind = pdb.GetGuid(cdi.Kind);
            if (kind == EncLocalSlotMap) slots = pdb.GetBlobContent(cdi.Value);
            else if (kind == EncLambdaAndClosureMap) lambdas = pdb.GetBlobContent(cdi.Value);
            else if (kind == EncStateMachineStateMap) states = pdb.GetBlobContent(cdi.Value);
        }
        return EditAndContinueMethodDebugInformation.Create(slots, lambdas, states);
    }

    /// <summary>Emits an EnC delta of <paramref name="next"/> against <paramref name="baseline"/>. Returns null on any failure.</summary>
    internal static (ModuleDelta? delta, ImmutableArray<Diagnostic> errors) EmitDelta(
        CSharpCompilation next, EmitBaseline baseline, IEnumerable<SemanticEdit> edits)
    {
        using var md = new MemoryStream();
        using var il = new MemoryStream();
        using var pdb = new MemoryStream();
        var r = next.EmitDifference(baseline, edits, _ => false, md, il, pdb, CancellationToken.None);
        return r.Success && r.Baseline is { } nextBaseline
            ? (new ModuleDelta(md.ToArray(), il.ToArray(), pdb.ToArray(), nextBaseline), [])
            : (null, [.. r.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error)]);
    }
}

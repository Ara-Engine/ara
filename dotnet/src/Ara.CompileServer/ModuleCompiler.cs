using System.Collections.Immutable;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.CompileServer;

/// <summary>A full module image: PE plus portable PDB.</summary>
internal sealed record ModuleImage(byte[] Pe, byte[] Pdb);

/// <summary>An EnC delta for <c>MetadataUpdater.ApplyUpdate</c>, plus the baseline for the next one.</summary>
internal sealed record ModuleDelta(byte[] Metadata, byte[] Il, byte[] Pdb, EmitBaseline Next);

/// <summary>
/// Roslyn for one module: references, options, parsing, the Ara generator, full emit and
/// EnC deltas (adapted from the S2 spike). Compilations are retained by
/// <see cref="ModuleSession"/> and updated with <c>ReplaceSyntaxTree</c>, so unchanged
/// files keep their parse and declaration work.
/// </summary>
internal sealed class ModuleCompiler
{
    static readonly Guid EncLocalSlotMap = new("755F52A8-91C5-45BE-B4B8-209571E552BD");
    static readonly Guid EncLambdaAndClosureMap = new("A643004C-0240-496F-A783-30D64F4979DE");
    static readonly Guid EncStateMachineStateMap = new("8B78CD68-2EDE-420B-980B-E15884B8AAA3");

    /// <summary>The usings the SDK would add for a module project (kept in sync with the sample csproj).</summary>
    internal const string GlobalUsings =
        "global using System;\nglobal using System.Collections.Generic;\nglobal using System.Linq;\nglobal using System.Numerics;\nglobal using Ara;\n";

    /// <summary>Doc comments must be parsed: the generator turns them into schema docs.</summary>
    internal static readonly CSharpParseOptions ParseOptions = new(LanguageVersion.Latest, DocumentationMode.Parse);

    internal readonly string AssemblyName;
    internal readonly ImmutableArray<MetadataReference> References;

    internal ModuleCompiler(string assemblyName, IEnumerable<string> extraReferences)
    {
        AssemblyName = assemblyName;
        References = [.. FrameworkReferences(), .. extraReferences.Select(p => MetadataReference.CreateFromFile(p))];
    }

    internal static CSharpCompilationOptions Options(bool debugIL) => new(
        OutputKind.DynamicallyLinkedLibrary,
        optimizationLevel: debugIL ? OptimizationLevel.Debug : OptimizationLevel.Release,
        allowUnsafe: true,
        deterministic: true,
        nullableContextOptions: NullableContextOptions.Enable);

    /// <summary>Framework reference assemblies: the ref pack when installed, else the runtime's own.</summary>
    static IEnumerable<MetadataReference> FrameworkReferences()
    {
        var runtimeDir = Path.GetDirectoryName(typeof(object).Assembly.Location)!; // ...\shared\Microsoft.NETCore.App\10.0.x
        var refRoot = Path.GetFullPath(Path.Combine(runtimeDir, "..", "..", "..", "packs", "Microsoft.NETCore.App.Ref"));
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
        return files.Select(f => (MetadataReference)MetadataReference.CreateFromFile(f));
    }

    internal static SyntaxTree Parse(string path, string text) =>
        CSharpSyntaxTree.ParseText(text, ParseOptions, path, Encoding.UTF8);

    internal CSharpCompilation Create(IEnumerable<SyntaxTree> trees, bool debugIL) =>
        CSharpCompilation.Create(AssemblyName, trees, References, Options(debugIL));

    /// <summary>A generator driver running the Ara module generator over a module's compilation.</summary>
    internal static GeneratorDriver Driver() =>
        CSharpGeneratorDriver.Create([new Ara.Generators.ModuleGenerator().AsSourceGenerator()], parseOptions: ParseOptions);

    /// <summary>Emits the whole module; returns null and the errors on failure.</summary>
    internal static (ModuleImage? Image, ImmutableArray<Diagnostic> Errors) Emit(Compilation c)
    {
        using var pe = new MemoryStream();
        using var pdb = new MemoryStream();
        var r = c.Emit(pe, pdb, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.PortablePdb));
        return r.Success
            ? (new ModuleImage(pe.ToArray(), pdb.ToArray()), [])
            : (null, [.. r.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error)]);
    }

    /// <summary>The EnC baseline of a freshly emitted image, with EnC maps read from its PDB.</summary>
    internal static EmitBaseline Baseline(Compilation c, ModuleImage image)
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

    /// <summary>Emits an EnC delta of <paramref name="next"/> against <paramref name="baseline"/>; null on any failure.</summary>
    internal static (ModuleDelta? Delta, ImmutableArray<Diagnostic> Errors) EmitDelta(
        Compilation next, EmitBaseline baseline, IEnumerable<SemanticEdit> edits)
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

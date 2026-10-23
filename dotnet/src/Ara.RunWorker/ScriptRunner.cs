using System.Collections.Immutable;
using System.Reflection;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ara.Agent;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Emit;

namespace Ara.RunWorker;

/// <summary>The outcome of one script: its result, or diagnostics in the ara-diag JSON shape.</summary>
internal sealed record RunResult(bool Ok, JsonNode? Result, string? Why, IReadOnlyList<string> Log, int Calls, JsonArray Diagnostics);

/// <summary>
/// Compiles an agent script (the body of an <c>async</c> method with <c>ara</c> in scope)
/// against the <c>Ara.Agent</c> facade only, checks it against the <see cref="Whitelist"/>,
/// and runs it in a throwaway collectible ALC. Script lines map 1:1 to diagnostics.
/// </summary>
internal sealed class ScriptRunner
{
    const string Usings = "using System;\nusing System.Collections.Generic;\nusing System.Linq;\nusing System.Text;\nusing System.Text.Json.Nodes;\nusing System.Threading.Tasks;\nusing Ara.Agent;\n";
    const string RunFailed = "ARA0962";
    const string RunAccess = "ARA0961";

    readonly ImmutableArray<MetadataReference> _references;
    int _count;

    internal ScriptRunner()
    {
        _references = [.. FrameworkReferences(), MetadataReference.CreateFromFile(typeof(ScriptContext).Assembly.Location)];
    }

    static IEnumerable<MetadataReference> FrameworkReferences()
    {
        var runtimeDir = Path.GetDirectoryName(typeof(object).Assembly.Location)!;
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

    /// <summary>The C# source a script body compiles as (leading <c>using</c> lines move to the top).</summary>
    internal static string Wrap(string code)
    {
        var lines = code.Replace("\r\n", "\n").Split('\n');
        var usings = lines.TakeWhile(l => l.TrimStart().StartsWith("using ", StringComparison.Ordinal) && l.TrimEnd().EndsWith(';')).ToList();
        var body = string.Join("\n", lines.Skip(usings.Count));
        return Usings + string.Join("\n", usings) +
            "\n#pragma warning disable CS0162, CS1998\npublic static class __AraScript\n{\n    public static async Task<object?> Run(ScriptContext ara)\n    {\n" +
            $"#line {usings.Count + 1} \"script\"\n" + body + "\n#line default\n        return null;\n    }\n}\n";
    }

    /// <summary>Compiles, checks and runs one script.</summary>
    internal async Task<RunResult> Run(string code, ICommandSink sink, CancellationToken cancel)
    {
        var diags = new JsonArray();
        var tree = CSharpSyntaxTree.ParseText(Wrap(code), new CSharpParseOptions(LanguageVersion.Latest), "script.g.cs", Encoding.UTF8);
        var compilation = CSharpCompilation.Create($"AraScript{Interlocked.Increment(ref _count)}", [tree], _references,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, optimizationLevel: OptimizationLevel.Release,
                allowUnsafe: false, nullableContextOptions: NullableContextOptions.Disable));
        using var pe = new MemoryStream();
        var emit = compilation.Emit(pe, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.Embedded));
        foreach (var d in emit.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error))
        {
            var span = d.Location.GetMappedLineSpan();
            diags.Add(Diag(RunFailed, $"{d.Id}: {d.GetMessage()}", span.StartLinePosition.Line + 1, span.StartLinePosition.Character + 1));
        }
        if (!emit.Success) return new RunResult(false, null, null, [], 0, diags);
        var image = pe.ToArray();
        foreach (var problem in Whitelist.Check(image)) diags.Add(Diag(RunAccess, problem));
        if (diags.Count > 0) return new RunResult(false, null, null, [], 0, diags);

        var alc = new ScriptLoadContext();
        var ctx = new ScriptContext(sink, cancel);
        try
        {
            var asm = alc.LoadFromStream(new MemoryStream(image));
            var run = asm.GetType("__AraScript")!.GetMethod("Run")!.CreateDelegate<Func<ScriptContext, Task<object?>>>();
            var value = await run(ctx).ConfigureAwait(false);
            var result = value is null ? null : JsonSerializer.SerializeToNode(value, value.GetType(), AgentJson.Options);
            return new RunResult(true, result, ctx.Why, ctx.Lines, ctx.Calls, diags);
        }
        catch (Exception e)
        {
            var (line, column) = ScriptLine(e);
            var message = e is AgentCommandException ace ? ace.Message : $"{e.GetType().Name}: {e.Message}";
            diags.Add(Diag(RunFailed, $"the script threw {message}", line, column));
            return new RunResult(false, null, ctx.Why, ctx.Lines, ctx.Calls, diags);
        }
        finally
        {
            alc.Unload();
        }
    }

    /// <summary>The first stack frame inside the script.</summary>
    static (int Line, int Column) ScriptLine(Exception e)
    {
        foreach (var f in new System.Diagnostics.StackTrace(e, true).GetFrames())
            if (f.GetFileName() == "script") return (f.GetFileLineNumber(), f.GetFileColumnNumber());
        return (0, 0);
    }

    internal static JsonObject Diag(string code, string message, int line = 0, int column = 0)
    {
        var d = new JsonObject { ["code"] = code, ["severity"] = "error", ["message"] = message };
        if (line > 0)
        {
            var at = new JsonObject { ["file"] = "script", ["line"] = line };
            if (column > 0) at["column"] = column;
            d["location"] = at;
        }
        return d;
    }

    /// <summary>A throwaway context; <c>Ara.Agent</c> resolves to the worker's copy so the facade types match.</summary>
    sealed class ScriptLoadContext() : AssemblyLoadContext("ara_run", isCollectible: true)
    {
        static readonly Assembly Agent = typeof(ScriptContext).Assembly;

        protected override Assembly? Load(AssemblyName name) => name.Name == Agent.GetName().Name ? Agent : null;
    }
}

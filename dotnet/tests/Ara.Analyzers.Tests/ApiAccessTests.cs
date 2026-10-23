using System.Collections.Immutable;
using System.Diagnostics.CodeAnalysis;
using Ara.Analyzers.CodeFixes;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Diagnostics;

namespace Ara.Analyzers.Tests;

public class ApiAccessTests
{
    /// <summary>The ARA05xx ids reported for a method body under a profile.</summary>
    static List<string> Ids(string body, string profile = ApiProfiles.Gameplay, string attributes = "")
    {
        var source = $$"""
            {{attributes}}
            public static partial class G
            {
                static object o = new();
                public static async System.Threading.Tasks.Task<object?> F(int n)
                {
                    {{body}}
                    return null;
                }
            }
            """;
        var options = new AnalyzerOptions([], new Profile(profile));
        return [.. Verify.Compile(source).WithAnalyzers([new ApiAccessAnalyzer()], options).GetAnalyzerDiagnosticsAsync().GetAwaiter().GetResult()
            .Where(d => d.Id.StartsWith("ARA05", StringComparison.Ordinal)).Select(d => d.Id).Distinct().Order()];
    }

    [Theory]
    [InlineData("var e = System.IO.File.Exists(\"x\");", "ARA0501")]
    [InlineData("var d = new System.IO.DirectoryInfo(\"x\");", "ARA0501")]
    [InlineData("var h = System.Net.Dns.GetHostName();", "ARA0502")]
    [InlineData("var e = typeof(string).GetEvents();", "ARA0503")]
    [InlineData("var a = System.Reflection.Assembly.GetExecutingAssembly();", "ARA0503")]
    [InlineData("var c = System.Runtime.Loader.AssemblyLoadContext.Default;", "ARA0503")]
    [InlineData("new System.Threading.Thread(() => { }).Start();", "ARA0504")]
    [InlineData("int x = 0; System.Threading.Interlocked.Increment(ref x);", "ARA0504")]
    [InlineData("lock (o) { }", "ARA0504")]
    [InlineData("var p = System.Runtime.InteropServices.Marshal.AllocHGlobal(4);", "ARA0505")]
    [InlineData("System.Console.WriteLine(n);", "ARA0506")]
    [InlineData("var s = System.Environment.NewLine;", "ARA0506")]
    [InlineData("System.GC.Collect();", "ARA0506")]
    [InlineData("var p = System.Diagnostics.Process.GetCurrentProcess();", "ARA0506")]
    [InlineData("var j = System.Text.Json.JsonSerializer.Serialize(n);", "ARA0500")]
    public void Gameplay_modules_are_refused_ambient_authority(string body, string id) => Assert.Equal([id], Ids(body));

    [Fact]
    public void The_core_runtime_is_allowed()
    {
        Assert.Equal(["ARA0504"], Ids("""
            var list = new List<int> { 1, 2, 3 };
            var sum = list.Where(i => i > 1).Sum() + (int)Math.Sqrt(n) + (int)MathF.Max(1f, 2f);
            var sb = new System.Text.StringBuilder().Append(string.Join(",", list)).ToString();
            var v = System.Numerics.Vector3.Normalize(new System.Numerics.Vector3(1, 2, 3));
            var size = System.Runtime.CompilerServices.Unsafe.SizeOf<int>();
            Span<int> span = stackalloc int[4];
            var bytes = System.Runtime.InteropServices.MemoryMarshal.AsBytes(span);
            var pool = System.Buffers.ArrayPool<int>.Shared.Rent(4);
            var ms = new System.IO.MemoryStream();
            var r = new Random(1).Next() + DateTime.Now.Second + Guid.NewGuid().GetHashCode();
            var re = System.Text.RegularExpressions.Regex.IsMatch("a", "a");
            System.Diagnostics.Debug.Assert(sum > 0);
            var t = System.Diagnostics.Stopwatch.StartNew();
            Log.Info(sb);
            await System.Threading.Tasks.Task.Yield();
            """)); // only the await (and its Task): gameplay code has no tasks
    }

    [Fact]
    public void Extern_methods_are_native_interop() =>
        Assert.Contains("ARA0505", Ids("", attributes: "public static partial class N { [System.Runtime.InteropServices.DllImport(\"x\")] static extern int F(); }"));

    [Fact]
    public void Editor_modules_get_files_network_tasks_and_reflection()
    {
        Assert.Empty(Ids("""
            var e = System.IO.File.Exists("x");
            using var http = new System.Net.Http.HttpClient();
            System.Console.WriteLine(n);
            var m = typeof(string).GetEvents();
            await System.Threading.Tasks.Task.Run(() => 1);
            lock (o) { }
            """, ApiProfiles.Editor));
        Assert.Equal(["ARA0506"], Ids("System.Environment.Exit(1);", ApiProfiles.Editor));
        Assert.Equal(["ARA0505"], Ids("var p = System.Runtime.InteropServices.Marshal.AllocHGlobal(4);", ApiProfiles.Editor));
        Assert.Equal(["ARA0503"], Ids("var c = System.Runtime.Loader.AssemblyLoadContext.Default;", ApiProfiles.Editor));
    }

    [Fact]
    public void Simulation_code_is_left_to_the_determinism_rule() =>
        Assert.Empty(Ids("var e = System.IO.File.Exists(\"x\");", attributes: "[Simulation] public static partial class G { }"));

    [Fact]
    public void Console_output_becomes_log_calls()
    {
        var fixedCode = Verify.Fix("""
            public static class G
            {
                public static string F(int n)
                {
                    System.Console.WriteLine("hi");
                    System.Console.WriteLine(n);
                    System.Console.Error.WriteLine("bad {0}", n);
                    System.Diagnostics.Debug.WriteLine($"n = {n}");
                    System.GC.Collect();
                    return "a" + System.Environment.NewLine;
                }
            }
            """, new ApiAccessAnalyzer(), new ApiAccessFix(), "ARA0506");
        Assert.Equal("""
            public static class G
            {
                public static string F(int n)
                {
                    Log.Info("hi");
                    Log.Info($"{n}");
                    Log.Error(string.Format("bad {0}", n));
                    Log.Info($"n = {n}");
                    return "a" + "\n";
                }
            }
            """, fixedCode);
        Assert.Empty(Verify.Errors(fixedCode));
    }

    /// <summary>Global analyzer options carrying the module profile (what the compile server passes).</summary>
    sealed class Profile(string profile) : AnalyzerConfigOptionsProvider
    {
        public override AnalyzerConfigOptions GlobalOptions { get; } = new Options(profile);
        public override AnalyzerConfigOptions GetOptions(SyntaxTree tree) => new Options(null);
        public override AnalyzerConfigOptions GetOptions(AdditionalText textFile) => new Options(null);

        sealed class Options(string? profile) : AnalyzerConfigOptions
        {
            public override bool TryGetValue(string key, [NotNullWhen(true)] out string? value)
            {
                value = profile is not null && key == ApiAccessAnalyzer.ProfileOption ? profile : null;
                return value is not null;
            }
        }
    }
}

using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text.Json;

// Actual guard-method allocations only; parsing/fixture setup stays outside the samples.
var root = Path.GetFullPath(args[0]);
AssemblyLoadContext.Default.Resolving += (_, name) =>
{
    var path = Path.Combine(root, name.Name + ".dll");
    return File.Exists(path) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(root, "Ara.CompileServer.dll"));
var change = assembly.GetType("Ara.CompileServer.FileChange", throwOnError: true)!;
var parse = assembly.GetType("Ara.CompileServer.ModuleCompiler", throwOnError: true)!
    .GetMethod("Parse", BindingFlags.NonPublic | BindingFlags.Static)!;
var guard = assembly.GetType("Ara.CompileServer.EditClassifier", throwOnError: true)!
    .GetMethod("DocsChanged", BindingFlags.NonPublic | BindingFlags.Static)!;
var cases = new List<object>();
foreach (var fields in new[] { 0, 1, 128, 1024 })
{
    var members = string.Concat(Enumerable.Range(0, fields).Select(i => $"/// <summary>Value {i}.</summary>\npublic int Value{i};\n"));
    var before = "public class Probe { static int Helper() { return 7; }\n" + members + "}";
    var after = before.Replace("return 7;", "var amount = 12345; return amount;", StringComparison.Ordinal);
    var oldTree = parse.Invoke(null, ["Probe.cs", before]);
    var newTree = parse.Invoke(null, ["Probe.cs", after]);
    var array = Array.CreateInstance(change, 1);
    array.SetValue(Activator.CreateInstance(change, ["Probe.cs", oldTree, newTree]), 0);
    object?[] arguments = [array];
    bool Check() => (bool)guard.Invoke(null, arguments)!;
    for (var warmup = 0; warmup < 8; warmup++)
        if (Check()) throw new InvalidDataException("body-only edit invalidated docs");
    var samples = new long[20];
    for (var sample = 0; sample < samples.Length; sample++)
    {
        var start = GC.GetAllocatedBytesForCurrentThread();
        var result = Check();
        samples[sample] = GC.GetAllocatedBytesForCurrentThread() - start;
        if (result) throw new InvalidDataException("body-only edit invalidated docs");
    }
    cases.Add(new { fields, samples });
}
Console.WriteLine(JsonSerializer.Serialize(new
{
    runtime = System.Environment.Version.ToString(),
    compilerSha256 = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(Path.Combine(root, "Ara.CompileServer.dll")))).ToLowerInvariant(),
    scope = "Actual DocsChanged allocation including reflection return boxing; no latency admission. Body edits with 0/1/128/1024 documented fields.",
    cases
}));

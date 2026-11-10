using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

// Outside-window differential probe of the real documentation extraction entry.
var root = Path.GetFullPath(args[0]);
AssemblyLoadContext.Default.Resolving += (_, name) =>
{
    var path = Path.Combine(root, name.Name + ".dll");
    return File.Exists(path) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(root, "Ara.Generators.dll"));
var method = assembly.GetType("Ara.Generators.Docs", throwOnError: true)!
    .GetMethod("Tag", BindingFlags.NonPublic | BindingFlags.Static) ?? throw new InvalidDataException("missing documentation entry");
var tag = method.CreateDelegate<Func<string?, string, string?, string>>();
var cases = JsonSerializer.Deserialize<string?[]>(File.ReadAllText(args[1])) ?? throw new InvalidDataException("missing corpus");
var output = cases.Select(xml => new { summary = tag(xml, "summary", null), parameter = tag(xml, "param", "value") }).ToArray();
Console.WriteLine(JsonSerializer.Serialize(new
{
    cases = cases.Length,
    corpusSha256 = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(args[1]))).ToLowerInvariant(),
    generatorSha256 = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(Path.Combine(root, "Ara.Generators.dll")))).ToLowerInvariant(),
    outputSha256 = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(output)))).ToLowerInvariant(),
    output
}));

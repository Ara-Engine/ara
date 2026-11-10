using System.Linq.Expressions;
using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

// Read-only actual-method probe. Input projects must already have real publications.
var server = Path.GetFullPath(args[0]);
AssemblyLoadContext.Default.Resolving += (_, name) => {
    var path = Path.Combine(Path.GetDirectoryName(server)!, name.Name + ".dll");
    return File.Exists(path) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path) : null;
};
var assembly = AssemblyLoadContext.Default.LoadFromAssemblyPath(server);
var inventoryType = assembly.GetType("Ara.CompileServer.ProjectInventory", true)!;
var requestType = assembly.GetType("Ara.CompileServer.InventoryRequest", true)!;
var moduleType = assembly.GetType("Ara.CompileServer.ProjectModule", true)!;
var scan = inventoryType.GetMethod("Scan", BindingFlags.Static | BindingFlags.NonPublic)!;
var scoped = scan.GetParameters().Length == 3;
var parameter = Expression.Parameter(moduleType, "module");
var predicateType = typeof(Predicate<>).MakeGenericType(moduleType);
var onlyEditor = Expression.Lambda(predicateType, Expression.Equal(Expression.Property(parameter, "Profile"), Expression.Constant("editor")), parameter).Compile();
var none = Expression.Lambda(predicateType, Expression.Constant(false), parameter).Compile();
var options = new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };
var output = new List<object>();
foreach (var project in args.Skip(1))
{
    var request = Activator.CreateInstance(requestType, Path.GetFullPath(project), "content")!;
    JsonObject? publicInventory = null;
    foreach (var mode in scoped ? new[] { "public", "ownership", "gameplay-discovery", "editor-discovery" } : ["public", "ownership"])
    {
        object? filter = mode == "gameplay-discovery" ? onlyEditor : mode == "editor-discovery" ? none : null;
        object?[] parameters = scoped ? [request, mode != "ownership", filter] : [request, mode != "ownership"];
        object Read() => scan.Invoke(null, parameters)!;
        for (var i = 0; i < 8; i++) _ = Read();
        var allocations = new List<long>();
        object? last = null;
        for (var i = 0; i < 20; i++)
        {
            var before = GC.GetAllocatedBytesForCurrentThread();
            last = Read();
            allocations.Add(GC.GetAllocatedBytesForCurrentThread() - before);
        }
        var document = JsonSerializer.SerializeToNode(last, last!.GetType(), options)!.AsObject();
        var modules = document["modules"]!.AsArray();
        if (document["diagnostics"]!.AsArray().Count != 0) throw new InvalidDataException("fixture discovery diagnostics");
        var published = modules.Count(module => (string?)module!["result"] == "published");
        if (mode == "public")
        {
            if (published == 0) throw new InvalidDataException("fixture has no valid publication");
            publicInventory = document;
        }
        else
        {
            var expected = publicInventory!.DeepClone().AsObject();
            foreach (var module in expected["modules"]!.AsArray())
            {
                if (mode == "gameplay-discovery" && (string?)module!["profile"] == "editor") continue;
                module!["schema"] = null;
                module["fingerprint"] = null;
                module["result"] = "unbuilt";
            }
            if (!JsonNode.DeepEquals(expected, document)) throw new InvalidDataException("inventory contract mismatch");
        }
        var json = document.ToJsonString();
        output.Add(new { project, mode, modules = modules.Count, published, sources = document["sources"]!.AsArray().Count,
            responseSha256 = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(json))), allocations });
    }
}
Console.WriteLine(JsonSerializer.Serialize(new { scope = "Read-only real Rust-published fixtures; actual discovery method allocation counters, not compile latency.",
    runtime = Environment.Version.ToString(), scoped, rows = output }));

using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;

// Only controlled fixture witnesses execute: public static float Read() => a constant.
var request = JsonSerializer.Deserialize<Request>(File.ReadAllText(args[0]), new JsonSerializerOptions { PropertyNameCaseInsensitive = true })!;
var context = new FixtureContext(request.Sdk, request.Modules);
var count = 0;
try
{
    foreach (var module in request.Modules)
    {
        var assembly = context.LoadFromAssemblyPath(Path.GetFullPath(module.Path));
        var witnesses = assembly.GetTypes().Where(type => type.FullName?.StartsWith(module.Namespace + ".Witness", StringComparison.Ordinal) == true).ToArray();
        if (witnesses.Length != module.Witnesses.Length) throw new InvalidDataException("unexpected witness inventory: " + module.Name);
        foreach (var name in module.Witnesses)
        {
            var type = assembly.GetType(module.Namespace + "." + name, throwOnError: true)!;
            var method = type.GetMethod("Read", BindingFlags.Public | BindingFlags.Static, Type.EmptyTypes)
                ?? throw new InvalidDataException("missing witness: " + name);
            if (method.ReturnType != typeof(float) || method.Invoke(null, null) is not float value
                || BitConverter.SingleToInt32Bits(value) != request.RateBits)
                throw new InvalidDataException("stale or incorrect compiled constant: " + module.Name + "." + name);
            count++;
        }
    }
}
finally { context.Unload(); }
Console.WriteLine(JsonSerializer.Serialize(new { witnesses = count, rateBits = request.RateBits }));

sealed record Request(string Sdk, int RateBits, Module[] Modules);
sealed record Module(string Name, string Path, string Namespace, string[] Witnesses);
sealed class FixtureContext(string sdk, Module[] modules) : AssemblyLoadContext(isCollectible: true)
{
    protected override Assembly? Load(AssemblyName name)
    {
        var path = modules.FirstOrDefault(module => string.Equals(module.Name, name.Name, StringComparison.OrdinalIgnoreCase))?.Path
            ?? Path.Combine(sdk, name.Name + ".dll");
        return File.Exists(path) ? LoadFromAssemblyPath(Path.GetFullPath(path)) : null;
    }
}

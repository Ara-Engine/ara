using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Loader;

namespace Ara.Hosting;

/// <summary>
/// The dev-build entry point the Rust host resolves through hostfxr
/// (<c>Ara.Hosting.DevExports, Ara.Host</c> → <c>Init</c>). It starts the runtime with the
/// reloading <see cref="DevModuleLoader"/>.
/// </summary>
public static unsafe class DevExports
{
    /// <summary>Creates one world's loader and dispatch in the shared CLR. Returns 0 on success.</summary>
    [UnmanagedCallersOnly]
    public static int Init(HostInit* host, ScriptApi* api)
    {
        try { return ScriptRuntime.Init(host, api, new DevModuleLoader()); }
        catch { return 99; }
    }
}

/// <summary>
/// Collectible context for one generation of one gameplay module. <c>Ara.Runtime</c>
/// resolves to the host's copy in the default context, so module types bind to the one
/// runtime (S2 finding 14); referenced modules resolve to the generations staged when this
/// context was created (<paramref name="modules"/>, by assembly name).
/// </summary>
internal sealed class ModuleLoadContext(string name, IReadOnlyDictionary<string, Assembly> modules)
    : AssemblyLoadContext(name, isCollectible: true)
{
    static readonly Assembly Runtime = typeof(ScriptRuntime).Assembly;
    static readonly string RuntimeName = Runtime.GetName().Name!;

    protected override Assembly? Load(AssemblyName n) =>
        n.Name == RuntimeName ? Runtime : n.Name is { } other && modules.TryGetValue(other, out var asm) ? asm : null;
}

using System.Runtime.InteropServices;

namespace Ara.Hosting;

/// <summary>
/// The dev-build entry point the Rust host resolves through hostfxr
/// (<c>Ara.Hosting.DevExports, Ara.Host</c> → <c>Init</c>). It starts the runtime with the
/// reloading <see cref="DevModuleLoader"/>.
/// </summary>
public static unsafe class DevExports
{
    /// <summary><c>ara_script_init</c> for JIT hosts. Returns 0 on success.</summary>
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
/// runtime (S2 finding 14).
/// </summary>
internal sealed class ModuleLoadContext(string name) : System.Runtime.Loader.AssemblyLoadContext(name, isCollectible: true)
{
    static readonly System.Reflection.Assembly Runtime = typeof(ScriptRuntime).Assembly;

    protected override System.Reflection.Assembly? Load(System.Reflection.AssemblyName n) =>
        n.Name == Runtime.GetName().Name ? Runtime : null;
}

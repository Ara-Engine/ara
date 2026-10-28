using System.Diagnostics.CodeAnalysis;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Text;

namespace Ara.Game;

/// <summary>
/// The only entry points Rust calls. Every one catches everything: an exception escaping an
/// <c>[UnmanagedCallersOnly]</c> method kills the process. <see cref="Init"/> is exported by name
/// from the NativeAOT image and fetched with <c>coreclr_create_delegate</c> on the R2R lane.
/// </summary>
public static unsafe class Exports
{
    static HostApi s_host;

    /// <summary>Checks the ABI, keeps the host table and fills <paramref name="api"/>. 0 = bound.</summary>
    [UnmanagedCallersOnly(EntryPoint = "ara_script_init")]
    public static int Init(HostApi* host, ScriptApi* api)
    {
        try
        {
            if (host->AbiVersion != Abi.Version || host->StructSize != (uint)sizeof(HostApi))
                return 1;
            s_host = *host;
            api->AbiVersion = Abi.Version;
            api->StructSize = (uint)sizeof(ScriptApi);
            api->RunSystem = &RunSystem;
            api->Checksum = &Checksum;
            api->Nop = &Nop;
            api->CallHost = &CallHost;
            api->LoadMod = &LoadMod;
            Log(1, $"ara_script_init: {RuntimeInformation.FrameworkDescription} {RuntimeInformation.RuntimeIdentifier} " +
                   $"dynamicCode={RuntimeFeature.IsDynamicCodeSupported} jit={RuntimeFeature.IsDynamicCodeCompiled}");
            return 0;
        }
        catch (Exception e)
        {
            Log(2, $"init faulted: {e}");
            return 99;
        }
    }

    [UnmanagedCallersOnly]
    static int RunSystem(uint id, Chunk* chunks, uint count, Tick* tick)
    {
        try
        {
            switch (id)
            {
                case 0:
                    Movement.Run(chunks, count, tick);
                    return 0;
                case 1:
                    throw new InvalidOperationException("intentional fault in system 1 (boundary test)");
                default:
                    return 2;
            }
        }
        catch (Exception e)
        {
            Log(2, $"system {id} faulted and was contained: {e.GetType().Name}: {e.Message}");
            return 1;
        }
    }

    [UnmanagedCallersOnly]
    static ulong Checksum(Chunk* chunks, uint count)
    {
        try
        {
            ulong h = 0xcbf29ce484222325UL;
            ulong rows = 0;
            for (uint c = 0; c < count; c++)
            {
                var span = new ReadOnlySpan<uint>(chunks[c].Col0, (int)chunks[c].Len * 4);
                foreach (var w in span)
                {
                    h ^= w;
                    h *= 0x100000001b3UL;
                }
                rows += chunks[c].Len;
            }
            Log(1, $"checksum 0x{h:x16} over {rows} entities");
            return h;
        }
        catch (Exception e)
        {
            Log(2, $"checksum faulted: {e.Message}");
            return 0;
        }
    }

    [UnmanagedCallersOnly]
    static uint Nop(uint x) => x + 1;

    [UnmanagedCallersOnly]
    static ulong CallHost(ulong n, uint fast)
    {
        ulong acc = 0;
        if (fast != 0)
        {
            var f = (delegate* unmanaged[SuppressGCTransition]<uint, uint>)(void*)s_host.Nop;
            for (ulong i = 0; i < n; i++) acc += f((uint)i);
        }
        else
        {
            var f = s_host.Nop;
            for (ulong i = 0; i < n; i++) acc += f((uint)i);
        }
        return acc;
    }

    [UnmanagedCallersOnly]
    [UnconditionalSuppressMessage("Trimming", "IL2026", Justification = "Mods load only on the R2R lane; NativeAOT rejects them at runtime by design.")]
    [UnconditionalSuppressMessage("Trimming", "IL2075", Justification = "As above: the mod's entry type is found by name.")]
    static int LoadMod(byte* path, nuint len)
    {
        try
        {
            var file = Path.GetFullPath(Encoding.UTF8.GetString(path, (int)len));
            var asm = AssemblyLoadContext.Default.LoadFromAssemblyPath(file);
            var entry = asm.GetType("Ara.Mod.Entry", throwOnError: true)!;
            var result = entry.GetMethod("Run")!.Invoke(null, null);
            Log(1, $"mod {asm.GetName().Name} loaded: {result}");
            return 0;
        }
        catch (Exception e)
        {
            Log(2, $"mod rejected: {e.GetType().Name}: {e.Message}");
            return 2;
        }
    }

    /// <summary>Sends one UTF-8 line to the host log. No-op before init.</summary>
    internal static void Log(uint level, string msg)
    {
        var log = s_host.Log;
        if (log == null)
            return;
        int max = Encoding.UTF8.GetMaxByteCount(msg.Length);
        Span<byte> buf = max <= 512 ? stackalloc byte[512] : new byte[max];
        int n = Encoding.UTF8.GetBytes(msg, buf);
        fixed (byte* p = buf)
            log(level, p, (nuint)n);
    }
}

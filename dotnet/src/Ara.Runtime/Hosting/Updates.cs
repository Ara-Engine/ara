namespace Ara.Hosting;

/// <summary>
/// A change to one module slot, produced by a loader (on any thread) and applied by the
/// runtime at the next tick boundary, when no C# code is running.
/// </summary>
public abstract class PendingUpdate
{
    /// <summary>The module slot.</summary>
    public required uint Slot { get; init; }
    /// <summary>Timing and outcome; the runtime fills generation and apply time.</summary>
    public ReloadReport Report;
}

/// <summary>
/// L3 (or the initial load): a new generation's declarations. The runtime registers them
/// with the host, builds new tables and installs them, then calls
/// <see cref="Installed"/>; on failure the old generation keeps running and
/// <see cref="Rejected"/> is called.
/// </summary>
public sealed class SwapUpdate : PendingUpdate
{
    /// <summary>Module name (<c>ara.module.toml</c>).</summary>
    public required string Name { get; init; }
    /// <summary>
    /// The new generation's declarations: loaders run its generated <c>Register</c> while
    /// staging, so the first call into new code (and its JIT) never lands on the tick thread.
    /// </summary>
    public required ModuleBuilder Module { get; init; }
    /// <summary>True if compiled as Debug IL with EnC enabled.</summary>
    public required bool Hot { get; init; }
    /// <summary>Called on the tick thread once the new tables are live (unload the old generation here).</summary>
    public Action? Installed { get; init; }
    /// <summary>Called on the tick thread if registration failed (unload the new generation here).</summary>
    public Action<string>? Rejected { get; init; }
}

/// <summary>L1: an EnC delta. <see cref="Apply"/> runs <c>MetadataUpdater.ApplyUpdate</c> on the tick thread.</summary>
public sealed class DeltaUpdate : PendingUpdate
{
    /// <summary>Applies the delta; throwing marks the update failed and asks the loader for L3.</summary>
    public required Action Apply { get; init; }
    /// <summary>Called if <see cref="Apply"/> threw.</summary>
    public Action<Exception>? Failed { get; init; }
}

/// <summary>A compile error: the diagnostics were already reported; this records the reload outcome.</summary>
public sealed class ErrorUpdate : PendingUpdate;

/// <summary>
/// Produces module generations: the dev loader (Ara.Host) compiles through the
/// out-of-process compile server and loads collectible ALCs; the NativeAOT loader
/// registers the statically linked modules.
/// </summary>
public interface IModuleLoader
{
    /// <summary>
    /// Loads the modules described by <paramref name="configJson"/>. Initial generations
    /// must be enqueued (<see cref="ScriptRuntime.Enqueue"/>) before this returns.
    /// Returns 0 on success.
    /// </summary>
    int Load(string configJson);

    /// <summary>True if modules can be recompiled at runtime.</summary>
    bool CanReload { get; }

    /// <summary>Recompiles a module hot (Debug IL, EnC) or cold (optimized). Returns 0 if accepted.</summary>
    int SetHeat(uint slot, bool hot);

    /// <summary>Retired generations still alive (leaks).</summary>
    int AlcsAlive { get; }

    /// <summary>Generations created so far.</summary>
    int AlcsCreated { get; }

    /// <summary>Stops background work and unloads everything. Returns the leaked-generation count.</summary>
    int Shutdown();
}

/// <summary>The NativeAOT loader: modules are linked into the image; nothing reloads.</summary>
public sealed class StaticModuleLoader(params (string Name, Action<ModuleBuilder> Register)[] modules) : IModuleLoader
{
    /// <inheritdoc/>
    public int Load(string configJson)
    {
        for (int i = 0; i < modules.Length; i++)
        {
            var builder = new ModuleBuilder();
            try { modules[i].Register(builder); }
            catch (Exception e)
            {
                Diag.Report(AraCodes.LoadFailed, "error", $"module `{modules[i].Name}`: Register threw {e.GetType().Name}: {e.Message}");
                return 1;
            }
            ScriptRuntime.Enqueue(new SwapUpdate { Slot = (uint)i, Name = modules[i].Name, Module = builder, Hot = false });
        }
        return 0;
    }

    /// <inheritdoc/>
    public bool CanReload => false;

    /// <inheritdoc/>
    public int SetHeat(uint slot, bool hot) => -1;

    /// <inheritdoc/>
    public int AlcsAlive => 0;

    /// <inheritdoc/>
    public int AlcsCreated => 0;

    /// <inheritdoc/>
    public int Shutdown() => 0;
}

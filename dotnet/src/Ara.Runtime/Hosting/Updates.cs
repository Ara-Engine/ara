namespace Ara.Hosting;

/// <summary>
/// A change produced by a loader (on any thread) and applied by the runtime at the next
/// tick boundary, when no C# code is running. Updates apply in the order they were enqueued.
/// </summary>
public abstract class PendingUpdate;

/// <summary>One module of a <see cref="SwapUpdate"/>: a new generation's declarations.</summary>
public sealed class ModuleSwap
{
    /// <summary>The module slot.</summary>
    public required uint Slot { get; init; }
    /// <summary>Module name (<c>ara.module.toml</c>).</summary>
    public required string Name { get; init; }
    /// <summary>
    /// The new generation's declarations: loaders run its generated <c>Register</c> while
    /// staging, so the first call into new code (and its JIT) never lands on the tick thread.
    /// </summary>
    public required ModuleBuilder Module { get; init; }
    /// <summary>True if compiled as Debug IL with EnC enabled.</summary>
    public required bool Hot { get; init; }
    /// <summary>Timing and outcome; the runtime fills generation, apply time and the cascade size.</summary>
    public ReloadReport Report;
    /// <summary>Called on the tick thread once the new tables are live (unload the old generation here).</summary>
    public Action? Installed { get; init; }
    /// <summary>Called on the tick thread if registration failed (unload the new generation here).</summary>
    public Action<string>? Rejected { get; init; }
}

/// <summary>
/// L3 (or the initial load): new generations of one or more modules, swapped together.
/// A dependency cascade (a module and the modules that reference it, in dependency
/// order) is one update: the host registers all of them or none, so every module keeps
/// a consistent view of the others.
/// </summary>
public sealed class SwapUpdate : PendingUpdate
{
    /// <summary>The modules, dependencies first.</summary>
    public required IReadOnlyList<ModuleSwap> Modules { get; init; }
}

/// <summary>L1/L2: an EnC delta. <see cref="Apply"/> runs <c>MetadataUpdater.ApplyUpdate</c> on the tick thread.</summary>
public sealed class DeltaUpdate : PendingUpdate
{
    /// <summary>The module slot.</summary>
    public required uint Slot { get; init; }
    /// <summary>Timing and outcome (kind <c>ReloadL1</c> or <c>ReloadL2</c>).</summary>
    public ReloadReport Report;
    /// <summary>Applies the delta; throwing marks the update failed and asks the loader for L3.</summary>
    public required Action Apply { get; init; }
    /// <summary>Called if <see cref="Apply"/> threw.</summary>
    public Action<Exception>? Failed { get; init; }
}

/// <summary>
/// An outcome without new code: a compile error (diagnostics were already reported) or
/// an L4 restart request. Records the reload report.
/// </summary>
public sealed class ErrorUpdate : PendingUpdate
{
    /// <summary>The module slot.</summary>
    public required uint Slot { get; init; }
    /// <summary>Kind <c>ReloadError</c> or <c>ReloadL4</c>.</summary>
    public ReloadReport Report;
}

/// <summary>
/// Produces module generations: the dev loader (Ara.Host) compiles through the
/// out-of-process compile server and loads collectible ALCs; the NativeAOT loader
/// registers the statically linked modules.
/// </summary>
public interface IModuleLoader
{
    /// <summary>Captures the host that owns this loader and its asynchronous updates.</summary>
    void Attach(ScriptRuntime runtime);

    /// <summary>
    /// Loads the modules described by <paramref name="configJson"/> (already in dependency
    /// order). Initial generations must be enqueued (<see cref="ScriptRuntime.Enqueue"/>)
    /// before this returns. Returns 0 on success.
    /// </summary>
    int Load(string configJson);

    /// <summary>True if modules can be recompiled at runtime.</summary>
    bool CanReload { get; }

    /// <summary>Recompiles a module hot (Debug IL, EnC) or cold (optimized). Returns 0 if accepted.</summary>
    int SetHeat(uint slot, bool hot);

    /// <summary>
    /// Queues a compile of changed files (<c>{"module", "paths", "origin"}</c>). Must not
    /// block. Returns 0 if accepted.
    /// </summary>
    int RequestCompile(string json);

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
    ScriptRuntime? _runtime;
    /// <inheritdoc/>
    public void Attach(ScriptRuntime runtime) => _runtime = runtime;

    /// <inheritdoc/>
    public int Load(string configJson)
    {
        var swaps = new List<ModuleSwap>(modules.Length);
        for (int i = 0; i < modules.Length; i++)
        {
            var builder = new ModuleBuilder();
            try { modules[i].Register(builder); }
            catch (Exception e)
            {
                Diag.Report(AraCodes.LoadFailed, "error", $"module `{modules[i].Name}`: Register threw {e.GetType().Name}: {e.Message}");
                return 1;
            }
            swaps.Add(new ModuleSwap { Slot = (uint)i, Name = modules[i].Name, Module = builder, Hot = false });
        }
        // Linked modules come in reference order (the generator emits them that way).
        (_runtime ?? throw new InvalidOperationException("loader is not attached")).Enqueue(new SwapUpdate { Modules = swaps });
        return 0;
    }

    /// <inheritdoc/>
    public bool CanReload => false;

    /// <inheritdoc/>
    public int SetHeat(uint slot, bool hot) => -1;

    /// <inheritdoc/>
    public int RequestCompile(string json) => -1;

    /// <inheritdoc/>
    public int AlcsAlive => 0;

    /// <inheritdoc/>
    public int AlcsCreated => 0;

    /// <inheritdoc/>
    public int Shutdown() => 0;
}

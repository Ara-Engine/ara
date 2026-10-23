using System.Collections.Concurrent;
using System.Runtime;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>
/// The runtime core: module slots, the dispatch tables, and applying pending updates at
/// tick boundaries. Every host-facing entry point is in <see cref="Exports"/>; this
/// class does the work.
/// </summary>
public static unsafe class ScriptRuntime
{
    static IModuleLoader? s_loader;
    static volatile ModuleSlot[] s_slots = [];
    static readonly ConcurrentQueue<PendingUpdate> s_pending = new();
    static int s_faults;
    static int s_threadsSeen;
    [ThreadStatic] static bool t_seen;

    /// <summary>
    /// Checks the ABI identity, captures the host table, fills <paramref name="api"/> and
    /// installs <paramref name="loader"/>. Called by the hostfxr export (Ara.Host) or the
    /// generated NativeAOT <c>ara_script_init</c>. Returns 0 on success.
    /// </summary>
    public static int Init(HostInit* host, ScriptApi* api, IModuleLoader loader)
    {
        try
        {
            Host.Attach(*host);
            if (host->AbiVersion != AbiInfo.Version)
            {
                Diag.Report(AraCodes.StaleBindings, "error", $"ABI version mismatch: host {host->AbiVersion}, C# runtime {AbiInfo.Version}");
                return 1;
            }
            if (host->LayoutHash != AbiInfo.LayoutHash)
            {
                Diag.Report(AraCodes.StaleBindings, "error",
                    $"stale generated bindings: host layout hash {host->LayoutHash:X16}, C# {AbiInfo.LayoutHash:X16}; " +
                    "regenerate (ARA_UPDATE_GENERATED=1 cargo test -p ara-script --test generated) and rebuild dotnet/");
                return 2;
            }
            if (AbiInfo.VerifySizes() is { } err)
            {
                Diag.Report(AraCodes.StaleBindings, "error", "layout mismatch: " + err);
                return 3;
            }
            GCSettings.LatencyMode = GCLatencyMode.SustainedLowLatency;
            // A process may start several hosts in turn (tests, editor restarts): start clean.
            s_loader?.Shutdown();
            s_slots = [];
            s_pending.Clear();
            s_loader = loader;
            Exports.Fill(api);
            return 0;
        }
        catch (Exception e)
        {
            Diag.Report(AraCodes.RuntimeStart, "error", e.ToString());
            return 99;
        }
    }

    /// <summary>Queues an update for the next tick boundary. Safe from any thread.</summary>
    public static void Enqueue(PendingUpdate update) => s_pending.Enqueue(update);

    internal static bool HasPending => !s_pending.IsEmpty;

    internal static IModuleLoader? Loader => s_loader;

    internal static int Faults => s_faults;

    internal static int ThreadsSeen => s_threadsSeen;

    internal static int LoadModules(string json)
    {
        if (s_loader is null) return 1;
        var rc = s_loader.Load(json);
        ApplyPending();
        return rc;
    }

    /// <summary>Applies every queued update in order. Runs on the tick thread with no C# code running.</summary>
    internal static uint ApplyPending()
    {
        uint flags = 0;
        while (s_pending.TryDequeue(out var u))
        {
            flags |= u switch
            {
                SwapUpdate s => ApplySwap(s),
                DeltaUpdate d => ApplyDelta(d),
                _ => Finish(u, SlotOf(u.Slot)),
            };
        }
        return flags;
    }

    static ModuleSlot? SlotOf(uint index)
    {
        var slots = s_slots;
        return index < slots.Length ? slots[index] : null;
    }

    static uint Finish(PendingUpdate u, ModuleSlot? slot)
    {
        u.Report.Module = u.Slot;
        if (slot is not null)
        {
            u.Report.Generation = slot.Generation;
            u.Report.DeltaCount = slot.Deltas;
            u.Report.Hot = slot.Hot ? 1u : 0u;
        }
        u.Report.TApplied = Host.Now();
        Host.ReportReload(u.Report);
        return 0;
    }

    static uint ApplySwap(SwapUpdate u)
    {
        var slot = SlotOf(u.Slot);
        var builder = u.Module;
        var generation = slot is null ? 0u : slot.Generation + 1;
        byte[] response;
        try
        {
            response = Host.RegisterModule(builder.Descriptor(u.Name, u.Slot, generation, u.Hot));
        }
        catch (Exception e)
        {
            return Reject(u, slot, $"module `{u.Name}`: registration failed: {e.Message}");
        }
        using var doc = JsonDocument.Parse(response);
        var root = doc.RootElement;
        if (!root.GetProperty("ok").GetBoolean())
            return Reject(u, slot, null); // the host reported why
        SetIndexes(root, "components", builder.Components);
        SetIndexes(root, "resources", builder.Resources);
        SetIndexes(root, "cues", builder.Cues);
        slot ??= AddSlot(u.Slot, u.Name);
        slot.Tables = BuildTables(builder, slot.Tables);
        slot.Generation = generation;
        slot.Deltas = 0;
        slot.Hot = u.Hot;
        u.Installed?.Invoke();
        Finish(u, slot);
        return AbiConst.BoundaryReloaded | AbiConst.BoundarySchemaChanged;
    }

    static uint Reject(SwapUpdate u, ModuleSlot? slot, string? why)
    {
        if (why is not null) Diag.Report(AraCodes.LoadFailed, "error", why + " (the last good code keeps running)");
        u.Report.Kind = AbiConst.ReloadError;
        u.Rejected?.Invoke(why ?? "registration rejected");
        Finish(u, slot);
        return 0;
    }

    static uint ApplyDelta(DeltaUpdate d)
    {
        var slot = SlotOf(d.Slot);
        try
        {
            d.Apply();
        }
        catch (Exception e)
        {
            Diag.Report(AraCodes.LoadFailed, "error", $"EnC delta failed: {e.Message}; recompiling as a full module");
            d.Report.Kind = AbiConst.ReloadError;
            d.Failed?.Invoke(e);
            return Finish(d, slot);
        }
        if (slot is not null)
        {
            slot.Deltas++;
            slot.Tables.ClearFaults();
        }
        Finish(d, slot);
        return AbiConst.BoundaryReloaded;
    }

    static ModuleSlot AddSlot(uint index, string name)
    {
        var slots = s_slots;
        var next = new ModuleSlot[Math.Max(slots.Length, (int)index + 1)];
        slots.CopyTo(next, 0);
        for (int i = slots.Length; i < next.Length; i++) next[i] = new ModuleSlot((uint)i, i == index ? name : "");
        s_slots = next;
        return next[index];
    }

    static void SetIndexes(JsonElement root, string section, List<ModuleBuilder.LayoutEntry> entries)
    {
        if (!root.TryGetProperty(section, out var map)) return;
        foreach (var e in entries)
            if (map.TryGetProperty(e.Name, out var idx)) e.SetIndex(idx.GetUInt32());
    }

    static ModuleTables BuildTables(ModuleBuilder b, ModuleTables old)
    {
        var systems = b.Systems.Select(s => new SystemEntry(s.Name, (delegate*<SystemCall*, void>)s.Run)).ToArray();
        var behaviours = b.Behaviours.Select(d =>
        {
            var entry = new BehaviourEntry(d.Name, d.DataSize, d.Create);
            // Objects of the previous generation are recreated lazily; they get OnReload, not OnSpawn.
            if (old.Behaviours.FirstOrDefault(o => o.Name == d.Name) is { } prev)
                entry.Reloaded = [.. prev.Live.Keys, .. prev.Reloaded];
            return entry;
        }).ToArray();
        var commands = b.Commands.Select(c => new CommandEntry(c.Name, (delegate*<CommandCall*, int>)c.Invoke)).ToArray();
        return new ModuleTables(systems, behaviours, commands);
    }

    /// <summary>Runs one system over one chunk. Called concurrently from task-pool threads.</summary>
    internal static int RunSystem(SystemCall* call)
    {
        if (!t_seen) { t_seen = true; Interlocked.Increment(ref s_threadsSeen); }
        var id = call->System;
        if (SlotOf(id >> 16)?.Tables.Systems is not { } systems || (id & 0xFFFF) >= systems.Length) return AbiConst.RunBadId;
        var e = systems[id & 0xFFFF];
        if (Volatile.Read(ref e.Faulted) != 0) return AbiConst.RunDisabled;
        try
        {
            e.Run(call);
            return AbiConst.RunOk;
        }
        catch (Exception ex)
        {
            if (Interlocked.Exchange(ref e.Faulted, 1) == 0)
            {
                Interlocked.Increment(ref s_faults);
                Diag.Fault(AraCodes.SystemFault, $"system `{e.Name}`", ex);
            }
            return AbiConst.RunFaulted;
        }
    }

    /// <summary>Runs one behaviour type over one chunk, serially, with exclusive world access.</summary>
    internal static int RunBehaviours(BehaviourCall* call)
    {
        var id = call->Behaviour;
        if (SlotOf(id >> 16)?.Tables.Behaviours is not { } all || (id & 0xFFFF) >= all.Length) return AbiConst.RunBadId;
        var entry = all[id & 0xFFFF];
        if (Volatile.Read(ref entry.Faulted) != 0) return AbiConst.RunDisabled;
        var tick = call->Phase == AbiConst.PhaseTick;
        var frame = call->Tick->Frame;
        var data = (byte*)call->Data;
        var previous = Host.EnterExclusive();
        try
        {
            if (tick) entry.Sweep(frame);
            for (int r = 0; r < (int)call->Len; r++)
            {
                var bits = call->Entities[r];
                var created = false;
                if (!entry.Live.TryGetValue(bits, out var live))
                {
                    if (!tick) continue; // objects are created by the tick phase
                    live = (entry.Create(), frame);
                    live.Obj.Entity = new Entity(bits);
                    created = true;
                }
                var obj = live.Obj;
                if (tick) entry.Live[bits] = (obj, frame);
                obj.DataPtr = data + (long)r * entry.DataSize;
                obj.CommandBuffer = call->Commands;
                obj.SetRng(Rng.For(in *call->Tick, entry.RngKey, obj.Entity));
                try
                {
                    if (created)
                    {
                        if (entry.Reloaded.Remove(bits)) obj.OnReload();
                        else obj.OnSpawn();
                    }
                    if (tick) obj.OnTick(in *call->Tick);
                    else obj.OnFrame(in *call->Frame);
                }
                finally
                {
                    obj.DataPtr = null;
                    obj.CommandBuffer = null;
                }
            }
            return AbiConst.RunOk;
        }
        catch (Exception ex)
        {
            if (Interlocked.Exchange(ref entry.Faulted, 1) == 0)
            {
                Interlocked.Increment(ref s_faults);
                Diag.Fault(AraCodes.BehaviourFault, $"behaviour `{entry.Name}`", ex);
            }
            return AbiConst.RunFaulted;
        }
        finally
        {
            Host.ExitExclusive(previous);
        }
    }

    /// <summary>Runs one command with exclusive world access.</summary>
    internal static int RunCommand(CommandCall* call)
    {
        var id = call->Command;
        if (SlotOf(id >> 16)?.Tables.Commands is not { } all || (id & 0xFFFF) >= all.Length) return AbiConst.RunBadId;
        var entry = all[id & 0xFFFF];
        if (Volatile.Read(ref entry.Faulted) != 0) return AbiConst.RunDisabled;
        var previous = Host.EnterExclusive();
        try
        {
            return entry.Invoke(call);
        }
        catch (Exception ex)
        {
            if (Interlocked.Exchange(ref entry.Faulted, 1) == 0)
            {
                Interlocked.Increment(ref s_faults);
                Diag.Fault(AraCodes.CommandFault, $"command `{entry.Name}`", ex);
            }
            return AbiConst.RunFaulted;
        }
        finally
        {
            Host.ExitExclusive(previous);
        }
    }

    internal static int Shutdown()
    {
        // Drop the tables (delegates, behaviour objects) first: they keep module ALCs alive.
        s_slots = [];
        s_pending.Clear();
        return s_loader?.Shutdown() ?? 0;
    }
}

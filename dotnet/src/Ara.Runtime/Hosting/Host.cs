using System.Text;

namespace Ara.Hosting;

/// <summary>
/// The Rust host's function table, captured once by <see cref="ScriptRuntime.Init"/>.
/// Runtime infrastructure only; gameplay state never lives here.
/// </summary>
public static unsafe class Host
{
    /// <summary>Copies sorted handles without exposing engine storage to managed code.</summary>
    public static int QueryEntities(uint component, Span<Entity> entities)
    {
        RequireExclusive();
        fixed (Entity* p = entities)
        {
            var count = s_init.Api.QueryEntities(s_init.Ctx, component, (ulong*)p, (uint)entities.Length);
            if (count > entities.Length) throw new ArgumentException($"Query needs {count} entity slots", nameof(entities));
            return (int)count;
        }
    }

    /// <summary>Samples an action from the current fixed tick, in exclusive gameplay callbacks.</summary>
    public static float InputAxis(ulong player, string name, uint axis)
    {
        RequireExclusive();
        var count = Encoding.UTF8.GetByteCount(name);
        if (count > 128) throw new ArgumentException("Action names are limited to 128 UTF-8 bytes", nameof(name));
        Span<byte> bytes = stackalloc byte[count];
        Encoding.UTF8.GetBytes(name, bytes);
        fixed (byte* p = bytes) return s_init.Api.InputAxis(s_init.Ctx, player, p, (uint)count, axis);
    }

    /// <summary>The authored input owner of this entity, or zero.</summary>
    public static ulong InputPlayer(Entity entity)
    {
        RequireExclusive();
        return s_init.Api.InputPlayer(s_init.Ctx, entity.Bits);
    }

    /// <summary>Queues a destination for the engine navmesh agent, after the gameplay callback.</summary>
    public static void Navigate(Entity entity, System.Numerics.Vector3 target, bool enabled)
    {
        RequireExclusive();
        s_init.Api.Navigate(s_init.Ctx, entity.Bits, target.X, target.Y, target.Z, enabled ? 1u : 0u);
    }
    /// <summary>Suppresses this actor's movement and jump for the current fixed tick.</summary>
    public static void CharacterStop(Entity entity)
    {
        RequireExclusive();
        s_init.Api.CharacterStop(s_init.Ctx, entity.Bits);
    }
    /// <summary>Queues a named boolean animator input at the fixed-tick boundary.</summary>
    public static void AnimationBool(Entity entity, string name, bool value)
    {
        RequireExclusive();
        var count = Encoding.UTF8.GetByteCount(name);
        if (count > 128) throw new ArgumentException("Animation names are limited to 128 UTF-8 bytes", nameof(name));
        Span<byte> bytes = stackalloc byte[count];
        Encoding.UTF8.GetBytes(name, bytes);
        fixed (byte* p = bytes) s_init.Api.AnimationBool(s_init.Ctx, entity.Bits, p, (uint)count, value ? 1u : 0u);
    }
    static HostInit s_init;
    static volatile bool s_detached;
    [ThreadStatic] static bool t_exclusive;

    internal static void Attach(in HostInit init)
    {
        s_init = init;
        s_detached = false;
    }

    /// <summary>
    /// Called by <see cref="ScriptRuntime.Shutdown"/>: later logs, diagnostics and compile
    /// notifications become no-ops (the host leaks its context, so a call racing this is safe).
    /// </summary>
    internal static void Detach() => s_detached = true;

    /// <summary>True while a host is attached (after <see cref="ScriptRuntime.Init"/>, before shutdown).</summary>
    public static bool Attached => !s_detached && s_init.Api.Log != null;

    /// <summary>
    /// Marks the current thread as running an exclusive phase (behaviours, commands),
    /// where <see cref="World"/> access is allowed. Returns the previous value.
    /// </summary>
    internal static bool EnterExclusive()
    {
        var was = t_exclusive;
        t_exclusive = true;
        return was;
    }

    internal static void ExitExclusive(bool previous) => t_exclusive = previous;

    static void RequireExclusive()
    {
        if (!t_exclusive)
            throw new InvalidOperationException(
                "World access is only available in behaviours and commands; systems take components as parameters");
    }

    /// <summary>Logs a message. Level: 0 info, 1 warning, 2 error.</summary>
    public static void Log(uint level, string message)
    {
        if (!Attached) return;
        var bytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* p = bytes) s_init.Api.Log(s_init.Ctx, level, p, (uint)bytes.Length);
    }

    /// <summary>Reports a structured diagnostic (JSON in the ara-diag shape).</summary>
    public static void Diagnostic(ReadOnlySpan<byte> json)
    {
        if (!Attached) return;
        fixed (byte* p = json) s_init.Api.Diagnostic(s_init.Ctx, p, (uint)json.Length);
    }

    /// <summary>Host monotonic clock in nanoseconds (diagnostics only, never simulation).</summary>
    public static ulong Now() => Attached ? s_init.Api.NowNs(s_init.Ctx) : 0;

    /// <summary>The <c>HostInit</c> flags (<c>AbiConst.Init*</c>).</summary>
    public static uint Flags => s_init.Flags;

    internal static void ReportReload(in ReloadReport report)
    {
        if (!Attached) return;
        fixed (ReloadReport* p = &report) s_init.Api.ReportReload(s_init.Ctx, p);
    }

    /// <summary>
    /// Sends module descriptors (a JSON array, dependencies first) and returns the host's
    /// response (JSON); the host registers all of them or none. Only valid while the host
    /// is inside <c>load_modules</c> or <c>tick_boundary</c>.
    /// </summary>
    internal static byte[] RegisterModules(ReadOnlySpan<byte> descriptor)
    {
        var buf = new byte[64 * 1024];
        while (true)
        {
            int n;
            fixed (byte* d = descriptor)
            fixed (byte* o = buf)
                n = s_init.Api.RegisterModules(s_init.Ctx, d, (uint)descriptor.Length, o, (uint)buf.Length);
            if (n >= 0) return buf.AsSpan(0, n).ToArray();
            buf = new byte[-n];
        }
    }

    internal static byte* ComponentPtr(ulong entity, uint component, bool write)
    {
        RequireExclusive();
        return s_init.Api.ComponentPtr(s_init.Ctx, entity, component, write ? 1u : 0u);
    }

    internal static byte* ResourcePtr(uint resource, bool write)
    {
        RequireExclusive();
        return s_init.Api.ResourcePtr(s_init.Ctx, resource, write ? 1u : 0u);
    }

    internal static bool EntityAlive(ulong entity)
    {
        RequireExclusive();
        return s_init.Api.EntityAlive(s_init.Ctx, entity) != 0;
    }

    internal static ulong ReserveEntity(void* buf)
    {
        if (buf == null) throw new InvalidOperationException("no command buffer in this context");
        return s_init.Api.ReserveEntity(s_init.Ctx, buf);
    }

    internal static void CmdPush(void* buf, uint op, uint typeIndex, ulong entity, void* data, uint len)
    {
        if (buf == null) throw new InvalidOperationException("no command buffer in this context");
        s_init.Api.CmdPush(s_init.Ctx, buf, op, typeIndex, entity, (byte*)data, len);
    }

    /// <summary>Tells the host a compile started (true) or finished (false), for tick-thread priority.</summary>
    public static void CompileState(bool active)
    {
        if (Attached) s_init.Api.CompileState(s_init.Ctx, active ? 1u : 0u);
    }
}

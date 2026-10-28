using System.Text;

namespace Ara.Hosting;

/// <summary>
/// The Rust host's function table, captured once by <see cref="ScriptRuntime.Init"/>.
/// Runtime infrastructure only; gameplay state never lives here.
/// </summary>
public static unsafe class Host
{
    /// <summary>Copies scene identity or hierarchy matches from the active ECS world.</summary>
    public static int SceneEntities(uint operation, Entity entity, string selector, Span<Entity> entities)
    {
        var context = RequireExclusive();
        var count = Encoding.UTF8.GetByteCount(selector);
        if (count > 4096) throw new ArgumentException("Entity paths are limited to 4096 UTF-8 bytes", nameof(selector));
        Span<byte> bytes = stackalloc byte[count];
        Encoding.UTF8.GetBytes(selector, bytes);
        fixed (byte* name = bytes)
        fixed (Entity* output = entities)
        {
            var needed = context.Init.Api.SceneEntities(context.Init.Ctx, operation, entity.Bits, name, (uint)count, (ulong*)output, (uint)entities.Length);
            if (operation == AbiConst.SceneFind && needed > 1) throw new InvalidOperationException($"Entity name/path `{selector}` is ambiguous ({needed} matches); use a full path or stable ID");
            if (needed > entities.Length) throw new ArgumentException($"Hierarchy query needs {needed} entity slots", nameof(entities));
            return (int)needed;
        }
    }

    [ThreadStatic] static HostContext? t_context;
    /// <summary>Copies sorted handles without exposing engine storage to managed code.</summary>
    public static int QueryEntities(uint component, Span<Entity> entities)
    {
        var context = RequireExclusive();
        fixed (Entity* p = entities)
        {
            var count = context.Init.Api.QueryEntities(context.Init.Ctx, component, (ulong*)p, (uint)entities.Length);
            if (count > entities.Length) throw new ArgumentException($"Query needs {count} entity slots", nameof(entities));
            return (int)count;
        }
    }

    /// <summary>Samples an action from the current fixed tick, in exclusive gameplay callbacks.</summary>
    public static float InputAxis(ulong player, string name, uint axis)
    {
        var context = RequireExclusive();
        var count = Encoding.UTF8.GetByteCount(name);
        if (count > 128) throw new ArgumentException("Action names are limited to 128 UTF-8 bytes", nameof(name));
        Span<byte> bytes = stackalloc byte[count];
        Encoding.UTF8.GetBytes(name, bytes);
        fixed (byte* p = bytes) return context.Init.Api.InputAxis(context.Init.Ctx, player, p, (uint)count, axis);
    }

    /// <summary>The authored input owner of this entity, or zero.</summary>
    public static ulong InputPlayer(Entity entity)
    {
        var context = RequireExclusive();
        return context.Init.Api.InputPlayer(context.Init.Ctx, entity.Bits);
    }

    /// <summary>Queues a destination for the engine navmesh agent, after the gameplay callback.</summary>
    public static void Navigate(Entity entity, System.Numerics.Vector3 target, bool enabled)
    {
        var context = RequireExclusive();
        context.Init.Api.Navigate(context.Init.Ctx, entity.Bits, target.X, target.Y, target.Z, enabled ? 1u : 0u);
    }
    /// <summary>Suppresses this actor's movement and jump for the current fixed tick.</summary>
    public static void CharacterStop(Entity entity)
    {
        var context = RequireExclusive();
        context.Init.Api.CharacterStop(context.Init.Ctx, entity.Bits);
    }
    /// <summary>Queues a named boolean animator input at the fixed-tick boundary.</summary>
    public static void AnimationBool(Entity entity, string name, bool value)
    {
        var context = RequireExclusive();
        var count = Encoding.UTF8.GetByteCount(name);
        if (count > 128) throw new ArgumentException("Animation names are limited to 128 UTF-8 bytes", nameof(name));
        Span<byte> bytes = stackalloc byte[count];
        Encoding.UTF8.GetBytes(name, bytes);
        fixed (byte* p = bytes) context.Init.Api.AnimationBool(context.Init.Ctx, entity.Bits, p, (uint)count, value ? 1u : 0u);
    }
    [ThreadStatic] static bool t_exclusive;

    static HostContext Current => t_context ?? throw new InvalidOperationException("no script host is active on this thread");

    /// <summary>Restores the previous callback context when a host entry point returns.</summary>
    public readonly struct Binding : IDisposable
    {
        readonly HostContext? _previous;
        readonly bool _exclusive;
        readonly bool _changed;
        readonly List<EngineAccess.IEntry>? _cache;

        internal Binding(HostContext context)
        {
            _previous = t_context;
            _exclusive = t_exclusive;
            _changed = !ReferenceEquals(context, _previous);
            _cache = _changed ? EngineAccess.Swap(null) : null;
            t_context = context;
            if (_changed) t_exclusive = false;
        }

        /// <inheritdoc/>
        public void Dispose()
        {
            if (_changed)
            {
                EngineAccess.Discard();
                EngineAccess.Swap(_cache);
            }
            t_context = _previous;
            t_exclusive = _exclusive;
        }
    }

    internal static Binding Bind(HostContext context) => new(context);

    /// <summary>Resolves a generated module type's index in the current host.</summary>
    public static uint TypeIndex(string name) => Current.Indexes.GetValueOrDefault(name, uint.MaxValue);

    /// <summary>Registers a generated module type's index for the current host.</summary>
    public static void SetTypeIndex(string name, uint index) => Current.Indexes[name] = index;

    /// <summary>True while this thread is bound to a live host with callbacks.</summary>
    public static bool Attached => t_context is { Detached: false } context && context.Init.Api.Log != null;

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

    internal static void ExitExclusive(bool previous)
    {
        // Successful callbacks flush inside their fault boundary. Failed callbacks
        // discard copies here, so cleanup cannot write or throw after fault handling.
        try { if (!previous) EngineAccess.Discard(); }
        finally { t_exclusive = previous; }
    }

    static HostContext RequireExclusive()
    {
        var context = Current;
        if (!t_exclusive || context.Detached)
            throw new InvalidOperationException(
                "World access is only available in behaviours and commands; systems take components as parameters");
        return context;
    }

    /// <summary>Logs a message. Level: 0 info, 1 warning, 2 error.</summary>
    public static void Log(uint level, string message)
    {
        if (!Attached) return;
        var bytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* p = bytes) Current.Init.Api.Log(Current.Init.Ctx, level, p, (uint)bytes.Length);
    }

    /// <summary>Reports a structured diagnostic (JSON in the ara-diag shape).</summary>
    public static void Diagnostic(ReadOnlySpan<byte> json)
    {
        if (!Attached) return;
        fixed (byte* p = json) Current.Init.Api.Diagnostic(Current.Init.Ctx, p, (uint)json.Length);
    }

    /// <summary>Host monotonic clock in nanoseconds (diagnostics only, never simulation).</summary>
    public static ulong Now() => Attached ? Current.Init.Api.NowNs(Current.Init.Ctx) : 0;

    /// <summary>The <c>HostInit</c> flags (<c>AbiConst.Init*</c>).</summary>
    public static uint Flags => Current.Init.Flags;

    internal static void ReportReload(in ReloadReport report)
    {
        if (!Attached) return;
        fixed (ReloadReport* p = &report) Current.Init.Api.ReportReload(Current.Init.Ctx, p);
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
                n = Current.Init.Api.RegisterModules(Current.Init.Ctx, d, (uint)descriptor.Length, o, (uint)buf.Length);
            if (n >= 0) return buf.AsSpan(0, n).ToArray();
            buf = new byte[-n];
        }
    }

    internal static byte* ComponentPtr(ulong entity, uint component, bool write)
    {
        var context = RequireExclusive();
        return context.Init.Api.ComponentPtr(context.Init.Ctx, entity, component, write ? 1u : 0u);
    }

    internal static byte[]? ComponentJson(ulong entity, uint component)
    {
        var context = RequireExclusive();
        var buffer = new byte[1024];
        while (true)
        {
            int n;
            fixed (byte* output = buffer)
                n = context.Init.Api.ComponentJson(context.Init.Ctx, entity, component, AbiConst.ComponentRead, null, 0, output, (uint)buffer.Length);
            if (n == 0) return null;
            if (n > 0) return buffer.AsSpan(0, n).ToArray();
            buffer = new byte[-n];
        }
    }

    internal static bool ComponentHas(ulong entity, uint component)
    {
        var context = RequireExclusive();
        return context.Init.Api.ComponentJson(context.Init.Ctx, entity, component, AbiConst.ComponentHas, null, 0, null, 0) != 0;
    }

    internal static void ComponentWrite(ulong entity, uint component, ReadOnlySpan<byte> json)
    {
        var context = RequireExclusive();
        fixed (byte* data = json)
            if (context.Init.Api.ComponentJson(context.Init.Ctx, entity, component, AbiConst.ComponentWrite, data, (uint)json.Length, null, 0) != 1)
                throw new InvalidOperationException($"component #{component} on {new Entity(entity)} rejected the value");
    }

    internal static byte* ResourcePtr(uint resource, bool write)
    {
        var context = RequireExclusive();
        return context.Init.Api.ResourcePtr(context.Init.Ctx, resource, write ? 1u : 0u);
    }

    internal static bool EntityAlive(ulong entity)
    {
        var context = RequireExclusive();
        return context.Init.Api.EntityAlive(context.Init.Ctx, entity) != 0;
    }

    internal static ulong ReserveEntity(void* buf)
    {
        if (buf == null) throw new InvalidOperationException("no command buffer in this context");
        return Current.Init.Api.ReserveEntity(Current.Init.Ctx, buf);
    }

    internal static void CmdPush(void* buf, uint op, uint typeIndex, ulong entity, void* data, uint len)
    {
        if (buf == null) throw new InvalidOperationException("no command buffer in this context");
        Current.Init.Api.CmdPush(Current.Init.Ctx, buf, op, typeIndex, entity, (byte*)data, len);
    }

    /// <summary>Tells the host a compile started (true) or finished (false), for tick-thread priority.</summary>
    public static void CompileState(bool active)
    {
        if (Attached) Current.Init.Api.CompileState(Current.Init.Ctx, active ? 1u : 0u);
    }
}

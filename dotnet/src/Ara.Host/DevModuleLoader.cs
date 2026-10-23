using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Runtime.CompilerServices;
using System.Text.Json;

namespace Ara.Hosting;

/// <summary>
/// The dev-build module loader. Each module generation lives in its own collectible
/// ALC; images and EnC deltas come from the out-of-process compile server (or, without
/// one, from a prebuilt assembly). Everything is staged off the tick thread (ALC load,
/// registration delegate, pre-JIT) and applied by the runtime at the next tick boundary;
/// the previous generation's ALC is unloaded once the new one is installed.
/// </summary>
public sealed class DevModuleLoader : IModuleLoader
{
    sealed class Module(uint slot, string name, string dir)
    {
        public readonly uint Slot = slot;
        public readonly string Name = name;
        public readonly string Dir = dir;
        public bool Hot;
        public ModuleLoadContext? Alc;
        public Assembly? Asm;
        public uint Generation;
        public readonly ManualResetEventSlim Staged = new(false);
        public bool Failed;
    }

    readonly List<Module> _modules = [];
    readonly List<WeakReference> _retired = [];
    readonly object _lock = new();
    int _created;
    CompileClient? _client;

    /// <inheritdoc/>
    public bool CanReload => _client is not null;

    /// <inheritdoc/>
    public int AlcsCreated => _created;

    /// <inheritdoc/>
    public int AlcsAlive
    {
        get { lock (_lock) return _retired.Count(w => w.IsAlive); }
    }

    /// <inheritdoc/>
    public int Load(string configJson)
    {
        using var doc = JsonDocument.Parse(configJson);
        var root = doc.RootElement;
        var server = root.TryGetProperty("compileServer", out var cs) && cs.ValueKind == JsonValueKind.String ? cs.GetString() : null;
        var waiting = new List<Module>();
        var rc = 0;
        foreach (var spec in root.GetProperty("modules").EnumerateArray())
        {
            var name = spec.GetProperty("name").GetString() ?? "Module";
            var dir = spec.GetProperty("dir").GetString() ?? "";
            var hot = spec.TryGetProperty("hot", out var h) && h.GetBoolean();
            var watch = spec.TryGetProperty("watch", out var w) && w.GetBoolean();
            var assembly = spec.TryGetProperty("assembly", out var a) && a.ValueKind == JsonValueKind.String ? a.GetString() : null;
            Module m;
            lock (_lock)
            {
                m = new Module((uint)_modules.Count, name, dir) { Hot = hot };
                _modules.Add(m);
            }
            if (assembly is not null && !watch)
            {
                var pdbPath = Path.ChangeExtension(assembly, ".pdb");
                var report = new ReloadReport { Kind = AbiConst.ReloadInitial, TReceived = Host.Now() };
                try
                {
                    Stage(m, File.ReadAllBytes(assembly), File.Exists(pdbPath) ? File.ReadAllBytes(pdbPath) : [], 0, hot, report);
                }
                catch (Exception e)
                {
                    Diag.Report(AraCodes.LoadFailed, "error", $"module `{name}`: cannot load `{assembly}`: {e.Message}");
                    rc = 1;
                }
                continue;
            }
            if (server is null)
            {
                Diag.Report(AraCodes.LoadFailed, "error", $"module `{name}` has no prebuilt assembly and no compile server is configured");
                rc = 1;
                continue;
            }
            try
            {
                _client ??= new CompileClient(server, this);
                _client.Watch(m.Slot, name, dir, hot);
                waiting.Add(m);
            }
            catch (Exception e)
            {
                Diag.Report(AraCodes.CompileServer, "error", $"compile server: {e.Message}");
                return 1;
            }
        }
        foreach (var m in waiting)
        {
            if (!m.Staged.Wait(TimeSpan.FromMinutes(3)))
            {
                Diag.Report(AraCodes.CompileServer, "error", $"module `{m.Name}`: the compile server sent no image within 3 minutes");
                rc = 1;
            }
            else if (m.Failed) rc = 1;
        }
        return rc;
    }

    Module? Find(uint slot)
    {
        lock (_lock) return slot < _modules.Count ? _modules[(int)slot] : null;
    }

    /// <summary>
    /// Loads an image into a fresh collectible ALC, resolves its generated
    /// <c>Register</c> and pre-JITs its entry points (compile-client thread), then queues
    /// the swap for the next tick boundary.
    /// </summary>
    void Stage(Module m, byte[] pe, byte[] pdb, uint generation, bool hot, ReloadReport report)
    {
        var alc = new ModuleLoadContext($"{m.Name}#{generation}");
        Interlocked.Increment(ref _created);
        Action<ModuleBuilder> register;
        try
        {
            var t0 = Host.Now();
            var asm = pdb.Length > 0 ? alc.LoadFromStream(new MemoryStream(pe), new MemoryStream(pdb)) : alc.LoadFromStream(new MemoryStream(pe));
            var t1 = Host.Now();
            var attr = asm.GetCustomAttribute<AraModuleAttribute>()
                ?? throw new InvalidOperationException("the assembly has no [AraModule]; is Ara.Generators referenced?");
            register = attr.Entry.GetMethod("Register", [typeof(ModuleBuilder)])?.CreateDelegate<Action<ModuleBuilder>>()
                ?? throw new InvalidOperationException($"{attr.Entry} has no Register(ModuleBuilder)");
            var builder = new ModuleBuilder();
            register(builder);
            Prepare(asm);
            report.TStaged = Host.Now();
            Host.Log(0, $"module `{m.Name}` generation {generation} staged: load {(t1 - t0) / 1e6:F1} ms, register + pre-JIT {(report.TStaged - t1) / 1e6:F1} ms ({pe.Length / 1024} KB PE)");
            report.Bytes = (ulong)(pe.Length + pdb.Length);
            ScriptRuntime.Enqueue(new SwapUpdate
            {
                Slot = m.Slot,
                Name = m.Name,
                Module = builder,
                Hot = hot,
                Report = report,
                Installed = () =>
                {
                    lock (_lock)
                    {
                        var old = m.Alc;
                        (m.Alc, m.Asm, m.Generation, m.Hot) = (alc, asm, generation, hot);
                        if (old is not null)
                        {
                            old.Unload();
                            _retired.Add(new WeakReference(old));
                        }
                    }
                },
                Rejected = _ =>
                {
                    lock (_lock)
                    {
                        alc.Unload();
                        _retired.Add(new WeakReference(alc));
                    }
                },
            });
        }
        catch
        {
            alc.Unload();
            lock (_lock) _retired.Add(new WeakReference(alc));
            m.Failed = true;
            m.Staged.Set();
            throw;
        }
        m.Staged.Set();
    }

    /// <summary>JITs the generated entry points now, so the first tick on new code does not pay for it.</summary>
    static void Prepare(Assembly asm)
    {
        const BindingFlags all = BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        foreach (var t in asm.GetTypes())
            foreach (var method in t.GetMethods(all))
                if (method.Name.StartsWith("__AraRun_", StringComparison.Ordinal) || method.Name == "__AraInvoke")
                    RuntimeHelpers.PrepareMethod(method.MethodHandle);
    }

    internal void OnImage(uint slot, uint generation, bool hot, uint kind, uint escalation, ulong serverUs, byte[] pe, byte[] pdb)
    {
        if (Find(slot) is not { } m) return;
        var report = new ReloadReport { Kind = kind, Escalation = escalation, ServerUs = serverUs, TReceived = Host.Now() };
        try { Stage(m, pe, pdb, generation, hot, report); }
        catch (Exception e) { Diag.Report(AraCodes.LoadFailed, "error", $"module `{m.Name}`: {e.Message} (the last good code keeps running)"); }
    }

    internal void OnDelta(uint slot, uint generation, uint changed, ulong serverUs, byte[] md, byte[] il, byte[] pdb)
    {
        if (Find(slot) is not { } m) return;
        if (Debugger.IsAttached)
        {
            // The runtime refuses ApplyUpdate with a debugger attached (R10 finding 1.2): take L3.
            _client?.Rebuild(slot, AbiConst.EscDebugger);
            return;
        }
        var report = new ReloadReport
        {
            Kind = AbiConst.ReloadL1,
            ChangedMembers = changed,
            ServerUs = serverUs,
            TReceived = Host.Now(),
            Bytes = (ulong)(md.Length + il.Length + pdb.Length),
        };
        report.TStaged = report.TReceived;
        ScriptRuntime.Enqueue(new DeltaUpdate
        {
            Slot = slot,
            Report = report,
            Apply = () =>
            {
                Assembly asm;
                lock (_lock)
                {
                    if (m.Asm is null || m.Generation != generation)
                        throw new InvalidOperationException($"delta for generation {generation}, but {m.Generation} is running");
                    asm = m.Asm;
                }
                MetadataUpdater.ApplyUpdate(asm, md, il, pdb);
            },
            Failed = _ => _client?.Rebuild(slot, AbiConst.EscRequested),
        });
    }

    internal void OnDiagnostics(uint slot, uint errors, string json, ulong serverUs)
    {
        using (var doc = JsonDocument.Parse(json))
            foreach (var d in doc.RootElement.EnumerateArray())
                Host.Diagnostic(System.Text.Encoding.UTF8.GetBytes(d.GetRawText()));
        if (errors == 0 || Find(slot) is not { } m) return;
        if (!m.Staged.IsSet)
        {
            m.Failed = true;
            m.Staged.Set();
            return;
        }
        ScriptRuntime.Enqueue(new ErrorUpdate
        {
            Slot = slot,
            Report = new ReloadReport { Kind = AbiConst.ReloadError, ErrorCount = errors, ServerUs = serverUs, TReceived = Host.Now(), TStaged = Host.Now() },
        });
    }

    /// <inheritdoc/>
    public int SetHeat(uint slot, bool hot)
    {
        if (_client is null || Find(slot) is null) return -1;
        _client.SetHeat(slot, hot);
        return 0;
    }

    /// <inheritdoc/>
    public int Shutdown()
    {
        _client?.Dispose();
        _client = null;
        lock (_lock)
        {
            foreach (var m in _modules)
            {
                if (m.Alc is { } alc)
                {
                    alc.Unload();
                    _retired.Add(new WeakReference(alc));
                }
                (m.Alc, m.Asm) = (null, null);
            }
            _modules.Clear();
        }
        // Unload can lag behind finalizers and JIT bookkeeping; retry briefly (S2).
        for (int i = 0; i < 40 && AlcsAlive > 0; i++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
            if (AlcsAlive > 0) Thread.Sleep(50);
        }
        return AlcsAlive;
    }
}

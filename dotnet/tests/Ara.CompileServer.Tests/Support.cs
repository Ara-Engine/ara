using System.Collections.Concurrent;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

/// <summary>One module entry of a <see cref="MessageKind.Reload"/>.</summary>
sealed record ReloadEntry(uint Slot, uint Generation, bool Hot, uint Kind, Escalation Escalation, uint Origin, CompileTiming Timing, byte[] Pe);

/// <summary>A message the coordinator sent, decoded.</summary>
sealed record Sent(MessageKind Kind, byte[] Payload)
{
    public ReloadEntry[] Reload()
    {
        Assert.Equal(MessageKind.Reload, Kind);
        var r = new Protocol.Reader(Payload);
        r.U64();
        var entries = new ReloadEntry[r.U32()];
        for (int i = 0; i < entries.Length; i++)
        {
            var e = new ReloadEntry(r.U32(), r.U32(), r.Bool(), r.U32(), (Escalation)r.U32(), r.U32(), r.Timing(), r.Bytes());
            r.Bytes();
            entries[i] = e;
        }
        return entries;
    }

    /// <summary>(slot, generation, kind, changed members, origin) of a delta.</summary>
    public (uint Slot, uint Generation, uint Kind, uint Changed, uint Origin) Delta()
    {
        Assert.Equal(MessageKind.Delta, Kind);
        var r = new Protocol.Reader(Payload);
        r.U64();
        return (r.U32(), r.U32(), r.U32(), r.U32(), r.U32());
    }

    /// <summary>(slot, errors, diagnostics JSON).</summary>
    public (uint Slot, uint Errors, string Json) Diagnostics()
    {
        Assert.Equal(MessageKind.Diagnostics, Kind);
        var r = new Protocol.Reader(Payload);
        return (r.U32(), r.U32(), r.Str());
    }

    public uint Slot => Kind switch
    {
        MessageKind.Reload => Reload()[0].Slot,
        MessageKind.Delta => Delta().Slot,
        MessageKind.Diagnostics => Diagnostics().Slot,
        _ => uint.MaxValue,
    };
}

/// <summary>Captures what the coordinator sends to the host (logs and compile state are dropped).</summary>
sealed class Capture : IMessageSink
{
    readonly BlockingCollection<Sent> _messages = [];

    public void Send(MessageKind kind, byte[] payload)
    {
        if (kind == MessageKind.Log)
        {
            var r = new Protocol.Reader(payload);
            r.U32();
            lock (Logs) Logs.Add(r.Str());
        }
        else if (kind != MessageKind.CompileState) _messages.Add(new Sent(kind, payload));
    }

    /// <summary>The next message; diagnostics without errors (warnings, infos) are collected in <see cref="Warnings"/> instead.</summary>
    public Sent Next()
    {
        while (true)
        {
            Assert.True(_messages.TryTake(out var m, TimeSpan.FromSeconds(60)),
                $"no message from the compile server; log:\n{string.Join("\n", Logs)}\nwarnings:\n{string.Join("\n", Warnings)}");
            if (m.Kind != MessageKind.Diagnostics || m.Diagnostics().Errors > 0) return m;
            Warnings.Add(m.Diagnostics().Json);
        }
    }

    public readonly List<string> Warnings = [];
    public readonly List<string> Logs = [];

    public bool Empty => _messages.All(m => m.Kind == MessageKind.Diagnostics && m.Diagnostics().Errors == 0);
}

/// <summary>Temporary module folders and a coordinator over them.</summary>
sealed class Modules : IDisposable
{
    public readonly string Root = Path.Combine(Path.GetTempPath(), "ara-cs-tests-" + Guid.NewGuid().ToString("N"));
    public readonly Capture Sink = new();
    public readonly Coordinator Server;

    public Modules(string capabilities = "Baseline AddMethodToExistingType AddStaticFieldToExistingType AddInstanceFieldToExistingType NewTypeDefinition")
    {
        Directory.CreateDirectory(Root);
        Server = new Coordinator(Sink, new AnalyzerSet());
        Server.Hello(true, capabilities);
    }

    public string File(string module, string name) => Path.Combine(Root, module, name);

    /// <summary>Writes a module's source and watches it (no file watcher: tests call <see cref="Edit"/>).</summary>
    public void Add(uint slot, string name, string source, bool hot = true, string[]? references = null, string? runtime = null)
    {
        Directory.CreateDirectory(Path.Combine(Root, name));
        System.IO.File.WriteAllText(File(name, "Code.cs"), source);
        Server.Watch(slot, name, Path.Combine(Root, name), hot, [runtime ?? typeof(Behaviour).Assembly.Location], references ?? [], "gameplay", watch: false);
    }

    /// <summary>Replaces text in a module's source and reports the change.</summary>
    public void Edit(uint slot, string module, string from, string to, uint origin = AbiConst.OriginHuman, bool wait = true)
    {
        var path = File(module, "Code.cs");
        var text = System.IO.File.ReadAllText(path);
        Assert.Contains(from, text);
        System.IO.File.WriteAllText(path, text.Replace(from, to));
        Server.Change(slot, [path], origin, System.Diagnostics.Stopwatch.GetTimestamp());
        if (wait) Idle();
    }

    public void Idle() => Assert.True(Server.WaitIdle(TimeSpan.FromSeconds(120)), "the compile server did not go idle");

    public void Dispose()
    {
        Server.Dispose();
        try { Directory.Delete(Root, recursive: true); } catch (IOException) { }
    }
}

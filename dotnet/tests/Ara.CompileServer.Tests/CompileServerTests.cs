using System.Collections.Concurrent;
using Ara.Hosting;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer.Tests;

public class ClassifierTests
{
    const string Before = """
        public static class S
        {
            static int field = 1;
            public static int F(int x) { return x + 1; }
            public static int G(int x) => x * 2;
            public static System.Collections.Generic.IEnumerable<int> Seq() { yield return 1; }
            public static int L(int x) { System.Func<int, int> f = static y => y; return f(x); }
        }
        public class C { public int V; public C() { V = 1; } }
        """;

    static (Escalation, int) Classify(string after, string before = Before)
    {
        var a = ModuleCompiler.Parse("s.cs", before);
        var b = ModuleCompiler.Parse("s.cs", after);
        var (esc, edits) = EditClassifier.Classify([new FileChange("s.cs", a, b)]);
        return (esc, edits.Count);
    }

    [Fact] public void Body_edits_take_l1() => Assert.Equal((Escalation.None, 2), Classify(Before.Replace("x + 1", "x + 2").Replace("x * 2", "x * 3")));
    [Fact] public void Trivia_edits_change_nothing() => Assert.Equal((Escalation.None, 0), Classify(Before.Replace("return x + 1;", "return x + 1; /* note */")));
    [Fact] public void Signatures_are_structural() => Assert.Equal(Escalation.Structural, Classify(Before.Replace("F(int x)", "F(long x)")).Item1);
    [Fact] public void New_members_are_structural() => Assert.Equal(Escalation.Structural, Classify(Before.Replace("public class C {", "public class C { public void H() { }")).Item1);
    [Fact] public void Initializers_escalate() => Assert.Equal(Escalation.Constructor, Classify(Before.Replace("field = 1", "field = 2")).Item1);
    [Fact] public void Constructors_escalate() => Assert.Equal(Escalation.Constructor, Classify(Before.Replace("V = 1;", "V = 2;")).Item1);
    [Fact] public void State_machines_escalate() => Assert.Equal(Escalation.StateMachine, Classify(Before.Replace("yield return 1;", "yield return 2;")).Item1);
    [Fact] public void Closures_escalate() => Assert.Equal(Escalation.Closure, Classify(Before.Replace("return f(x);", "return f(x) + 1;")).Item1);
    [Fact]
    public void Body_edits_in_primary_constructor_types_escalate()
    {
        const string before = "public class P(int seed) { public int F() { return 1; } }";
        Assert.Equal(Escalation.Constructor, Classify(before.Replace("return 1;", "return seed;"), before).Item1);
    }

    [Fact] public void Adding_a_lambda_escalates() => Assert.Equal(Escalation.Closure, Classify(Before.Replace("return x + 1;", "System.Func<int> g = () => x; return g();")).Item1);

    [Fact]
    public void New_files_escalate()
    {
        var (esc, _) = EditClassifier.Classify([new FileChange("t.cs", null, ModuleCompiler.Parse("t.cs", "class T { }"))]);
        Assert.Equal(Escalation.FileSet, esc);
    }
}

/// <summary>Captures what a session sends to the host.</summary>
sealed class Capture : IMessageSink
{
    public readonly BlockingCollection<(MessageKind Kind, byte[] Payload)> Messages = [];

    public void Send(MessageKind kind, byte[] payload)
    {
        if (kind is not (MessageKind.Log or MessageKind.CompileState)) Messages.Add((kind, payload));
    }

    public (MessageKind Kind, Protocol.Reader Reader) Next()
    {
        Assert.True(Messages.TryTake(out var m, TimeSpan.FromSeconds(60)), "no message from the session");
        return (m.Kind, new Protocol.Reader(m.Payload));
    }
}

public sealed class SessionTests : IDisposable
{
    readonly string _dir = Path.Combine(Path.GetTempPath(), "ara-cs-tests-" + Guid.NewGuid().ToString("N"));

    const string Source = """
        namespace Game;

        /// <summary>A counter.</summary>
        [Component] public partial struct Count { /// <summary>Ticks.</summary>
            public int Value; }

        public static partial class Counting
        {
            [Query]
            static void Run(ref Count c)
            {
                c.Value += 1;
            }
        }
        """;

    public SessionTests() => Directory.CreateDirectory(_dir);

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch (IOException) { }
    }

    ModuleSession Start(Capture sink, bool hot)
    {
        File.WriteAllText(Path.Combine(_dir, "Game.cs"), Source);
        var session = new ModuleSession(sink, 0, "Game", _dir, hot, enc: true, [typeof(Ara.Behaviour).Assembly.Location], new AnalyzerSet());
        session.Start(watch: false);
        return session;
    }

    void Edit(ModuleSession s, string from, string to)
    {
        var path = Path.Combine(_dir, "Game.cs");
        File.WriteAllText(path, File.ReadAllText(path).Replace(from, to));
        s.Change([path]);
        s.WaitIdle();
    }

    [Fact]
    public void Cold_module_goes_hot_then_takes_l1()
    {
        var sink = new Capture();
        using var s = Start(sink, hot: false);
        var (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Image, kind);
        Assert.Equal((0u, 0u, false, AbiConst.ReloadInitial), (r.U32(), r.U32(), r.Bool(), r.U32()));

        Edit(s, "c.Value += 1;", "c.Value += 2;");
        (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Image, kind);
        Assert.Equal((0u, 1u, true, AbiConst.ReloadL3, (uint)Escalation.Cold), (r.U32(), r.U32(), r.Bool(), r.U32(), r.U32()));

        Edit(s, "c.Value += 2;", "c.Value += 3;");
        (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Delta, kind);
        Assert.Equal((0u, 1u, 1u), (r.U32(), r.U32(), r.U32()));
    }

    [Fact]
    public void Errors_keep_the_last_good_state()
    {
        var sink = new Capture();
        using var s = Start(sink, hot: true);
        Assert.Equal(MessageKind.Image, sink.Next().Kind);

        Edit(s, "c.Value += 1;", "c.Value += ;");
        var (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Diagnostics, kind);
        r.U32();
        Assert.True(r.U32() > 0);
        var json = r.Str();
        Assert.Contains("\"code\":\"ARA0900\"", json);
        Assert.Contains("Game.cs", json);

        // The fix is diffed against the last good code: a body-only edit, so L1.
        Edit(s, "c.Value += ;", "c.Value += 5;");
        Assert.Equal(MessageKind.Delta, sink.Next().Kind);

        // A new component field is structural: L3.
        Edit(s, "public int Value; }", "public int Value; /// <summary>More.</summary>\n public int More; }");
        (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Image, kind);
        r.U32(); r.U32(); r.Bool(); r.U32();
        Assert.Equal((uint)Escalation.Structural, r.U32());

        // A doc-comment edit changes the generated schema docs, so it takes L3 too.
        Edit(s, "/// <summary>Ticks.</summary>", "/// <summary>Ticks seen.</summary>");
        (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Image, kind);
        r.U32(); r.U32(); r.Bool(); r.U32();
        Assert.Equal((uint)Escalation.Generated, r.U32());

        // Analyzer errors block a reload like compile errors.
        Edit(s, "c.Value += 5;", "c.Value += 5; Hits++; } static int Hits; static void X() {");
        (kind, r) = sink.Next();
        Assert.Equal(MessageKind.Diagnostics, kind);
        r.U32(); r.U32();
        Assert.Contains("ARA0100", r.Str());
    }
}

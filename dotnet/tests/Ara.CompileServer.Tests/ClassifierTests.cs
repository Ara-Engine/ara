namespace Ara.CompileServer.Tests;

/// <summary>
/// The L1/L2/L3 edit matrix: each edit of <see cref="Before"/> must take exactly the
/// expected path. L4 (a change the process cannot apply) is not a source edit; the
/// coordinator tests cover it. The Rust reload tests apply the same kinds of edit to a
/// running game and check its behaviour afterwards.
/// </summary>
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
            public virtual int V() => 0;
        }
        public class C { public int V; public C() { V = 1; } public int P => V; }
        public struct D { public int A; public int Get() => A; }
        public interface I { int M(); }
        public enum E { One, Two }
        """;

    enum Level { L1, L2, L3 }

    static (Level Level, Escalation Escalation) Classify(string after, string before = Before, EncCapabilities? caps = null)
    {
        var a = ModuleCompiler.Parse("s.cs", before);
        var b = ModuleCompiler.Parse("s.cs", after);
        var c = EditClassifier.Classify([new FileChange("s.cs", a, b)], caps ?? EncCapabilities.Net10);
        return (c.Escalation != Escalation.None ? Level.L3 : c.Additive ? Level.L2 : Level.L1, c.Escalation);
    }

    static string Edit(string from, string to)
    {
        Assert.Contains(from, Before);
        return Before.Replace(from, to);
    }

    public static TheoryData<string, string, string, uint> Matrix => new()
    {
        // L1: body-only.
        { "method body", "x + 1", "x + 2", (uint)Escalation.None },
        { "expression body", "x * 2", "x * 3", (uint)Escalation.None },
        { "property body", "public int P => V;", "public int P => V + 1;", (uint)Escalation.None },
        // L2: additive.
        { "new static method", "public static int G(int x)", "public static int H(int x) => x;\n    public static int G(int x)", (uint)Escalation.None },
        { "new method in a struct", "public int Get() => A;", "public int Get() => A; public int Twice() => A * 2;", (uint)Escalation.None },
        { "new instance field on a class", "public class C { public int V;", "public class C { public int V; public int W;", (uint)Escalation.None },
        { "new static field", "static int field = 1;", "static int field = 1; static int other;", (uint)Escalation.None },
        { "new const", "static int field = 1;", "static int field = 1; const int K = 5;", (uint)Escalation.None },
        { "new auto-property on a class", "public int P => V;", "public int P => V; public int Q { get; set; }", (uint)Escalation.None },
        { "new computed property in a struct", "public int Get() => A;", "public int Get() => A; public int B => A + 1;", (uint)Escalation.None },
        { "new type", "public enum E", "public class N { public int X; public N() { X = 3; } }\npublic enum E", (uint)Escalation.None },
        { "new nested type", "public int P => V;", "public int P => V; public sealed class Inner { public int Y = 2; }", (uint)Escalation.None },
        { "lambda in a new method", "public static int G(int x)", "public static int H(int x) { System.Func<int, int> f = y => y + x; return f(1); }\n    public static int G(int x)", (uint)Escalation.None },
        // L3: rude or unproven edits.
        { "signature", "F(int x)", "F(long x)", (uint)Escalation.Structural },
        { "removed member", "public static int G(int x) => x * 2;", "", (uint)Escalation.Structural },
        { "reordered members", "    public static int F(int x) { return x + 1; }\n    public static int G(int x) => x * 2;", "    public static int G(int x) => x * 2;\n    public static int F(int x) { return x + 1; }", (uint)Escalation.Structural },
        { "new struct field", "public struct D { public int A;", "public struct D { public int A; public int B2;", (uint)Escalation.Structural },
        { "new auto-property in a struct", "public int Get() => A;", "public int Get() => A; public int Q { get; set; }", (uint)Escalation.Structural },
        { "new virtual method", "public virtual int V() => 0;", "public virtual int V() => 0; public virtual int W() => 1;", (uint)Escalation.Structural },
        { "new interface member", "int M();", "int M(); int N();", (uint)Escalation.Structural },
        { "new generic method", "public static int G(int x)", "public static T Id<T>(T t) => t;\n    public static int G(int x)", (uint)Escalation.Structural },
        { "new partial type", "public enum E", "public partial class Q { }\npublic enum E", (uint)Escalation.Structural },
        { "enum member", "One, Two", "One, Two, Three", (uint)Escalation.Structural },
        { "attribute", "public static int F(int x)", "[System.Obsolete] public static int F(int x)", (uint)Escalation.Structural },
        { "field initializer", "field = 1", "field = 2", (uint)Escalation.Constructor },
        { "new field with an initializer", "static int field = 1;", "static int field = 1; static int other = 4;", (uint)Escalation.Constructor },
        { "constructor body", "V = 1;", "V = 2;", (uint)Escalation.Constructor },
        { "new constructor", "public C() { V = 1; }", "public C() { V = 1; } public C(int v) { V = v; }", (uint)Escalation.Constructor },
        { "iterator body", "yield return 1;", "yield return 2;", (uint)Escalation.StateMachine },
        { "new async method", "public static int G(int x)", "public static async System.Threading.Tasks.Task<int> A() { await System.Threading.Tasks.Task.Yield(); return 1; }\n    public static int G(int x)", (uint)Escalation.StateMachine },
        { "lambda body", "return f(x);", "return f(x) + 1;", (uint)Escalation.Closure },
        { "lambda added to a body", "return x + 1;", "System.Func<int> g = () => x; return g();", (uint)Escalation.Closure },
    };

    [Theory]
    [MemberData(nameof(Matrix))]
    public void Edit_matrix(string what, string from, string to, uint code)
    {
        var escalation = (Escalation)code;
        var (level, esc) = Classify(Edit(from, to));
        var expected = escalation != Escalation.None ? Level.L3 : what.StartsWith("new", StringComparison.Ordinal) || what.StartsWith("lambda in", StringComparison.Ordinal) ? Level.L2 : Level.L1;
        Assert.Equal((expected, escalation), (level, esc));
    }

    [Fact]
    public void Trivia_edits_change_nothing()
    {
        var c = EditClassifier.Classify([new FileChange("s.cs", ModuleCompiler.Parse("s.cs", Before), ModuleCompiler.Parse("s.cs", Edit("return x + 1;", "return x + 1; /* note */")))], EncCapabilities.Net10);
        Assert.Equal((Escalation.None, 0, 0), (c.Escalation, c.Bodies.Count, c.Inserts.Count));
    }

    [Fact]
    public void Additive_edits_need_the_runtime_capability() =>
        Assert.Equal((Level.L3, Escalation.Capability), Classify(Edit("public static int G(int x)", "public static int H() => 1;\n    public static int G(int x)"), caps: EncCapabilities.BodiesOnly));

    [Fact]
    public void Edits_in_primary_constructor_types_escalate()
    {
        const string before = "public class P(int seed) { public int F() { return 1; } }";
        Assert.Equal(Escalation.Constructor, Classify(before.Replace("return 1;", "return seed;"), before).Escalation);
        Assert.Equal(Escalation.Constructor, Classify(before.Replace("return 1; }", "return 1; } public int G() => seed;"), before).Escalation);
    }

    [Fact]
    public void New_files_with_new_types_take_l2_and_removed_files_l3()
    {
        var added = EditClassifier.Classify([new FileChange("t.cs", null, ModuleCompiler.Parse("t.cs", "namespace X; public class T { public int A() => 1; }"))], EncCapabilities.Net10);
        Assert.Equal((Escalation.None, true), (added.Escalation, added.Additive));
        var partial = EditClassifier.Classify([new FileChange("t.cs", null, ModuleCompiler.Parse("t.cs", "public partial class S { }"))], EncCapabilities.Net10);
        Assert.Equal(Escalation.Structural, partial.Escalation);
        var usings = EditClassifier.Classify([new FileChange("t.cs", null, ModuleCompiler.Parse("t.cs", "global using System.Text;"))], EncCapabilities.Net10);
        Assert.Equal(Escalation.Structural, usings.Escalation);
        var removed = EditClassifier.Classify([new FileChange("t.cs", ModuleCompiler.Parse("t.cs", "class T { }"), null)], EncCapabilities.Net10);
        Assert.Equal(Escalation.FileSet, removed.Escalation);
    }
}

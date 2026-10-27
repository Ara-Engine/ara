using Ara.Analyzers.CodeFixes;

namespace Ara.Analyzers.Tests;

public class UnityIsmTests
{
    const string UnityDoor = """
        using UnityEngine;

        public class Door : MonoBehaviour
        {
            float speed = 90f;

            void Start()
            {
                speed = 90f;
            }

            void Update()
            {
                var tr = GetComponent<Transform>();
                tr.Rotation.y += speed * Time.deltaTime;
                Debug.Log("tick");
            }
        }
        """;

    // `tr.Rotation.y` is found once `tr` resolves, after the GetComponent fix (see the next test).
    [Fact]
    public void Reports_each_unity_ism_where_it_is_written()
    {
        Verify.Diagnostics("""
            [|using UnityEngine;|]

            public class Door : [|MonoBehaviour|]
            {
                [Transient] float speed;
                void [|Start|]() { }
                void [|Update|]()
                {
                    var tr = [|GetComponent<Transform>|]();
                    tr.Rotation.y += speed * [|Time.deltaTime|];
                    [|Debug.Log|]("tick");
                }
            }
            """, new UnityAnalyzer(), "ARA0001");
    }

    [Fact]
    public void Fixes_a_unity_behaviour_into_code_that_compiles()
    {
        var fixedCode = Verify.Fix(UnityDoor, new UnityAnalyzer(), new UnityFix(), "ARA0001");
        Assert.Equal("""
            public partial class Door : Behaviour
            {
                float speed = 90f;

                protected override void OnSpawn()
                {
                    speed = 90f;
                }

                protected override void OnTick(in Tick tick)
                {
                    ref var tr = ref Get<Transform>();
                    tr.Rotation.Y += speed * tick.Dt;
                    Log.Info("tick");
                }
            }
            """, fixedCode.Trim());
        Assert.Empty(Verify.Errors(fixedCode));
    }

    [Fact]
    public void Lowercase_vector_members_are_fixed_even_outside_behaviours()
    {
        var src = "static class M { static float F(Vector3 v) => v.x + v.z; }";
        Assert.Equal("static class M { static float F(Vector3 v) => v.X + v.Z; }", Verify.Fix(src, new UnityAnalyzer(), new UnityFix(), "ARA0001"));
    }

    [Fact]
    public void Ara_code_is_left_alone()
    {
        Assert.DoesNotContain(Verify.Analyze("""
            public sealed partial class Door : Behaviour
            {
                protected override void OnTick(in Tick t) { ref var tr = ref Get<Transform>(); tr.Position.X += t.Dt; }
            }
            """, new UnityAnalyzer()), d => d.Id == "ARA0001");
    }
}

public class StateTests
{
    [Fact]
    public void Mutable_statics_are_errors()
    {
        Verify.Diagnostics("""
            static class Score
            {
                static int [|total|];
                static readonly int Max = 10;
                const int Min = 0;
                public static int [|Best|] { get; set; }
                public static int Calculated => Max;
            }
            """, new StateAnalyzer(), "ARA0100");
    }

    [Fact]
    public void Behaviour_fields_need_transient_and_the_fix_adds_it()
    {
        const string src = """
            public sealed partial class Lamp : Behaviour
            {
                float flicker;
                [Transient] float cache;
            }
            """;
        Verify.Diagnostics(src.Replace("float flicker", "float [|flicker|]"), new StateAnalyzer(), "ARA0101");
        var fixedCode = Verify.Fix(src, new StateAnalyzer(), new TransientFix(), "ARA0101");
        Assert.Equal(2, fixedCode.Split("[Transient]").Length - 1);
        Assert.DoesNotContain(Verify.Analyze(fixedCode, new StateAnalyzer()), d => d.Id == "ARA0101");
    }
}

public class DeterminismTests
{
    [Fact]
    public void Simulation_code_rejects_ambient_state()
    {
        Verify.Diagnostics("""
            using System.Threading.Tasks;
            [Simulation]
            static class Sim
            {
                static double Now() => [|DateTime.Now|].Second;
                static int Roll() => [|new Random()|].Next();
                static [|async|] Task Wait() => [|await [|Task.Delay(1)|]|];
                static string Read() => [|System.IO.File.ReadAllText("x")|];
            }
            static class NotSim
            {
                static double Now() => DateTime.Now.Second;
            }
            """, new DeterminismAnalyzer(), "ARA0200");
    }

    [Fact]
    public void Assembly_level_simulation_covers_every_type()
    {
        var d = Verify.Analyze("[assembly: Simulation]\nstatic class A { static long T() => System.Environment.TickCount64; }", new DeterminismAnalyzer());
        Assert.Single(d, x => x.Id == "ARA0200");
    }
}

public class AllocationTests
{
    [Fact]
    public void Tick_path_allocations_warn()
    {
        Verify.Diagnostics("""
            [Component] public partial struct Hp { public int Value; }
            public static partial class Sys
            {
                [Query]
                static void Run(ref Hp h)
                {
                    var list = [|new List<int>()|];
                    var arr = [|new int[4]|];
                    var any = [|list.Any()|];
                    object boxed = [|h.Value|];
                    Span<int> fine = stackalloc int[4];
                    h.Value += fine.Length;
                    if (h.Value < 0) throw new InvalidOperationException("exceptional paths may allocate");
                }
                static void NotTick() { var list = new List<int>(); }
            }
            """, new AllocationAnalyzer(), "ARA0300");
    }
}

public class AotTests
{
    [Fact]
    public void Aot_unsafe_apis_are_errors()
    {
        Verify.Diagnostics("""
            static class Loader
            {
                static object A() => [|System.Reflection.Assembly.Load("x")|];
                static object B(Type t) => [|Activator.CreateInstance(t)|]!;
                static object C() => [|Type.GetType("Foo")|]!;
                static T D<T>() where T : new() => Activator.CreateInstance<T>();
            }
            """, new AotAnalyzer(), "ARA0400");
    }
}

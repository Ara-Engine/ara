using System.Collections.Generic;
using System.Linq;

namespace Ara.Generators;

public static partial class Emit
{
    // Batch views borrow only this call's native ranges. No per-row managed method
    // invocation is generated; the gameplay method owns its loop and SIMD choices.
    static string QueryBatch(SystemModel m) => InShell(m.Shell, null, c =>
    {
        c.Line("/// <summary>Runs <c>" + m.Method + "</c> once over one contiguous run of borrowed ECS rows.</summary>");
        c.Line(Hidden);
        c.Open($"internal static unsafe void __AraRun_{m.Method}(global::Ara.Hosting.SystemCall* call)");
        c.Line("int n = (int)call->Len;");
        int col = 0, res = 0;
        var args = new List<string>();
        foreach (var p in m.Params)
        {
            switch (p.Kind)
            {
                case "span":
                    var span = p.RefKind == "ref" ? "Span" : "ReadOnlySpan";
                    args.Add($"new global::System.{span}<{p.CsType}>(call->Columns[{col}], n)");
                    col++;
                    break;
                case "entity-span":
                    args.Add("new global::System.ReadOnlySpan<global::Ara.Entity>(call->Entities, n)");
                    break;
                case "resource":
                    c.Line($"ref var r{res} = ref {Unsafe}.AsRef<{p.CsType}>(call->Resources[{res}]);");
                    args.Add($"{Pass(p.RefKind)}r{res}");
                    res++;
                    break;
                case "tick": args.Add(Pass(p.RefKind) + "*call->Tick"); break;
                case "commands": args.Add(Pass(p.RefKind) + "cmds"); break;
            }
        }
        if (m.Params.Items.Any(p => p.Kind == "commands")) c.Line("var cmds = global::Ara.Commands.FromCall(call->Commands);");
        c.Line($"{m.Method}({string.Join(", ", args)});");
        c.Close();
    });
}

using System.Text.Json.Nodes;

namespace Ara.Hosting;

/// <summary>Private diagnostics trust boundary: the live client and slot own module identity.</summary>
internal static class CompileDiagnosticScope
{
    internal static bool Bind(JsonObject row, string module, string profile, string instance, ulong binding)
    {
        if (row["data"] is not JsonObject data || data["compile"] is not JsonObject compile
            || (int?)compile["schema"] != 1 || (string?)compile["producer"] != "warm"
            || compile["attempt"] is not JsonValue attempt || !attempt.TryGetValue<ulong>(out var sequence)) return false;
        data["module"] = module;
        compile["profile"] = profile;
        compile["instance"] = instance;
        compile["binding"] = binding;
        var subjects = row["subjects"] as JsonArray ?? [];
        row["subjects"] = null;
        for (var i = subjects.Count - 1; i >= 0; i--)
        {
            var subject = (string?)subjects[i];
            if (subject?.StartsWith("module:", StringComparison.Ordinal) == true
                || subject?.StartsWith("system:csharp.compile/", StringComparison.Ordinal) == true) subjects.RemoveAt(i);
        }
        subjects.Add("module:" + module);
        subjects.Add($"system:csharp.compile/{instance}/{sequence}");
        row["subjects"] = subjects;
        return true;
    }

    internal static JsonObject Binding(JsonObject tagged)
    {
        var row = tagged.DeepClone().AsObject();
        row.Remove("location");
        row["code"] = AraCodes.CompileResult;
        row["severity"] = "info";
        row["message"] = (string?)row["data"]?["module"] + " compilation bound";
        var compile = row["data"]!["compile"]!.AsObject();
        foreach (var key in new[] { "result", "validation", "coverage", "errorCount", "total", "emitted", "truncated" }) compile.Remove(key);
        compile["attempt"] = 0UL;
        compile["phase"] = "bound";
        var subjects = row["subjects"]!.AsArray();
        for (var i = subjects.Count - 1; i >= 0; i--)
            if (((string?)subjects[i])?.StartsWith("system:csharp.compile/", StringComparison.Ordinal) == true) subjects.RemoveAt(i);
        subjects.Add($"system:csharp.compile/{(string?)compile["instance"]}/0");
        return row;
    }
}

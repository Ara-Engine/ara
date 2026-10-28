using System.Text.Json.Nodes;
using Ara.Hosting;

namespace Ara.CompileServer;

/// <summary>Private caller-owned identity, independent of helper processes and ALC generations.</summary>
internal sealed record CompileRequestContext(string Producer, string Instance, ulong Binding, ulong Attempt);

/// <summary>Immutable identity for one evaluated module input snapshot.</summary>
internal sealed record CompileAttempt(string Module, string Profile, CompileRequestContext Request)
{
    internal JsonObject Tag(JsonObject row, string phase = "diagnostic", string? channel = null)
    {
        var data = row["data"] as JsonObject;
        if (data is null)
        {
            var original = row["data"]?.DeepClone();
            row["data"] = data = new JsonObject();
            if (original is not null) data["original"] = original;
        }
        data["module"] = Module;
        var compile = data["compile"] as JsonObject;
        if (compile is null) data["compile"] = compile = new JsonObject();
        compile["schema"] = 1;
        compile["producer"] = Request.Producer;
        compile["instance"] = Request.Instance;
        compile["binding"] = Request.Binding;
        compile["attempt"] = Request.Attempt;
        compile["profile"] = Profile;
        compile["phase"] = phase;
        compile["channel"] = channel ?? Channel((string?)row["code"] ?? "");
        var subjects = row["subjects"] as JsonArray;
        if (subjects is null) row["subjects"] = subjects = [];
        foreach (var subject in new[] { "module:" + Module, $"system:csharp.compile/{Request.Instance}/{Request.Attempt}" })
            if (!subjects.Any(s => (string?)s == subject)) subjects.Add(subject);
        return row;
    }

    internal JsonObject Marker(string phase, string? result = null, bool validated = false,
        int errors = 0, int total = 0, int emitted = 0)
    {
        var row = Tag(new JsonObject { ["code"] = AraCodes.CompileResult, ["severity"] = "info",
            ["message"] = $"{Module} compilation {phase}" }, phase, "compiler");
        if (result is not null)
        {
            var compile = row["data"]!["compile"]!.AsObject();
            compile["result"] = result;
            compile["validation"] = validated ? "current-module-inputs" : "not-evaluated";
            compile["errorCount"] = errors;
            compile["total"] = total;
            compile["emitted"] = emitted;
            compile["truncated"] = emitted < total;
            // Warm analyzers inspect changed trees. A completion covers errors,
            // but never claims to replace warnings from untouched contributions.
            compile["coverage"] = new JsonObject { ["errors"] = validated ? "module" : "none", ["warnings"] = "partial" };
        }
        return row;
    }

    internal JsonObject[] Complete(IEnumerable<JsonObject> diagnostics, string result, bool validated)
    {
        var all = diagnostics.ToArray();
        var emitted = all.Take(50).Select(row => Tag(row)).ToArray();
        return [.. emitted, Marker("completed", result, validated, all.Count(d => (string?)d["severity"] == "error"), all.Length, emitted.Length)];
    }

    static string Channel(string code) => code switch
    {
        "ARA0923" or "ARA0924" or "ARA0925" => "dependency",
        _ when code.StartsWith("ARA0", StringComparison.Ordinal) && int.TryParse(code.AsSpan(3), out var number) && number < 700 => "analyzer",
        _ when code.StartsWith("ARA0", StringComparison.Ordinal) && int.TryParse(code.AsSpan(3), out var number) && number is >= 700 and < 900 => "generator",
        _ => "compiler",
    };
}

/// <summary>One authoritative producer lifetime; attempts are allocated before evaluation.</summary>
internal sealed class CompileProducer(string producer)
{
    readonly string _instance = Guid.NewGuid().ToString("N");
    long _attempt;
    internal CompileAttempt Next(string module, string profile) =>
        new(module, profile, new(producer, _instance, 1, checked((ulong)Interlocked.Increment(ref _attempt))));
}

using System.Text;

namespace Ara.Hosting;

/// <summary>The host function table, captured once at <see cref="Exports.Init"/>. Runtime infrastructure, not gameplay state.</summary>
internal static unsafe class Host
{
    static HostInit s_init;

    internal static void Attach(in HostInit init) => s_init = init;

    internal static void Log(uint level, string message)
    {
        var bytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* p = bytes) s_init.Api.Log(s_init.Ctx, level, p, (uint)bytes.Length);
    }

    /// <summary>Host monotonic clock in ns (diagnostics only).</summary>
    internal static ulong Now() => s_init.Api.NowNs(s_init.Ctx);

    internal static void Stamp(uint value) => s_init.Api.Stamp(s_init.Ctx, value);

    internal static void ReportReload(in ReloadReport report)
    {
        fixed (ReloadReport* p = &report) s_init.Api.ReportReload(s_init.Ctx, p);
    }

    internal static void ReportFault(uint system, string message)
    {
        var bytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* p = bytes) s_init.Api.ReportFault(s_init.Ctx, system, p, (uint)bytes.Length);
    }

    /// <summary>Milliseconds between two host-clock readings.</summary>
    internal static double Ms(ulong fromNs, ulong toNs) => (toNs - fromNs) / 1e6;
}

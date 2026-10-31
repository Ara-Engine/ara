namespace Ara.Hosting;

// Callback and registration infrastructure only. Authoritative gameplay stays in ECS.
internal sealed unsafe class HostContext(in HostInit init)
{
    internal readonly HostInit Init = init;
    internal readonly Dictionary<string, uint> Indexes = new(StringComparer.Ordinal);
    internal volatile bool Detached;
    internal uint GraphicsModule;
    internal ulong GraphicsRevision;
    internal readonly List<(uint Module, Action<GraphicsState> Handler)> GraphicsHandlers = [];
}

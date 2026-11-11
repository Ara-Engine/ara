using System.Collections.Concurrent;
using System.Runtime.InteropServices;

namespace Ara.CompileServer;

/// <summary>
/// macOS only (the caller checks; every libc import is reached through here): one kqueue vnode watch per source file. <c>FileSystemWatcher</c> there rides on FSEvents, which
/// delivers an in-place save about 11 ms late (median, measured); a vnode event arrives in ~0.1 ms. Only in-place
/// writes (truncate and write, as most editors save) are reported here. A file that is deleted or renamed away
/// (an atomic save) just drops its watch: the directory watcher reports those, and re-arms the new file through
/// <see cref="Add"/>, so a half-replaced file is never compiled early.
/// </summary>
internal sealed class VnodeEvents : IDisposable
{
    /// <summary>One descriptor per watched file; beyond this the directory watcher alone covers the rest.</summary>
    const int DefaultMaxFiles = 1024;
    const int RlimitNofile = 8;
    const short EvfiltVnode = -4;
    const ushort EvAdd = 0x1, EvClear = 0x20;
    const uint NoteDelete = 0x1, NoteWrite = 0x2, NoteExtend = 0x4, NoteRename = 0x20, NoteRevoke = 0x40;
    const int OEvtOnly = 0x8000, OCloexec = 0x1000000;

    readonly Action<uint, string> _changed;
    readonly int _queue;
    readonly ConcurrentDictionary<int, (uint Slot, string Path)> _byDescriptor = new();
    readonly ConcurrentDictionary<string, int> _byPath = new(StringComparer.Ordinal);
    readonly Thread _thread;
    readonly int _maxFiles;
    int _held;
    volatile bool _stopped;

    /// <summary><paramref name="changed"/> receives (slot, path) on the watcher thread for each in-place write.</summary>
    internal VnodeEvents(Action<uint, string> changed, int maxFiles = DefaultMaxFiles)
    {
        _changed = changed;
        // Never take more than a quarter of the process's descriptor limit: the server also needs descriptors
        // for source reads, memory-mapped references, pipes and the other watchers.
        _maxFiles = Math.Min(maxFiles, (int)Math.Min(DescriptorLimit() / 4, int.MaxValue));
        _queue = kqueue();
        if (_queue < 0) throw new InvalidOperationException("kqueue is unavailable");
        _thread = new Thread(Pump) { Name = "ara-vnode", IsBackground = true };
        _thread.Start();
    }

    /// <summary>True when <paramref name="path"/> is watched, so its in-place writes arrive from here first.</summary>
    internal bool Watching(string path) => _byPath.ContainsKey(path);

    /// <summary>Starts watching a file. Failures (a missing file, no descriptors left) leave it to the directory watcher.</summary>
    internal void Add(uint slot, string path)
    {
        if (_stopped || _byPath.ContainsKey(path)) return;
        // Reserve a slot first so concurrent adds cannot overshoot the budget.
        if (Interlocked.Increment(ref _held) > _maxFiles)
        {
            Interlocked.Decrement(ref _held);
            return;
        }
        var fd = open(path, OEvtOnly | OCloexec);
        if (fd < 0 || !_byPath.TryAdd(path, fd))
        {
            if (fd >= 0) _ = close(fd);
            Interlocked.Decrement(ref _held);
            return;
        }
        // Both maps are complete before the kernel can deliver the first event for this descriptor, so a delete or
        // rename that lands right after registration finds (and removes) both entries.
        _byDescriptor[fd] = (slot, path);
        var change = new KEvent { Ident = (nuint)fd, Filter = EvfiltVnode, Flags = (ushort)(EvAdd | EvClear), FFlags = NoteWrite | NoteExtend | NoteDelete | NoteRename | NoteRevoke };
        if (_stopped || Register(_queue, ref change, 1, IntPtr.Zero, 0, IntPtr.Zero) < 0) Remove(fd);
    }

    void Remove(int fd)
    {
        if (!_byDescriptor.TryRemove(fd, out var entry)) return;
        _byPath.TryRemove(new KeyValuePair<string, int>(entry.Path, fd));
        _ = close(fd); // closing the descriptor removes its kqueue registration
        Interlocked.Decrement(ref _held);
    }

    void Pump()
    {
        var events = new KEvent[16];
        while (!_stopped)
        {
            // A timeout bounds shutdown; an idle watcher wakes four times a second.
            var timeout = new TimeSpec { Seconds = 0, Nanoseconds = 250_000_000 };
            var count = Wait(_queue, IntPtr.Zero, 0, events, events.Length, ref timeout);
            for (var i = 0; i < count; i++)
            {
                var fd = (int)events[i].Ident;
                if (!_byDescriptor.TryGetValue(fd, out var entry)) continue;
                if ((events[i].FFlags & (NoteDelete | NoteRename | NoteRevoke)) != 0) Remove(fd);
                else
                {
                    // A failing callback must not end the watcher thread (and with it every later fast save).
                    try { _changed(entry.Slot, entry.Path); }
                    catch (Exception) { /* the directory watcher still reports the save */ }
                }
            }
        }
    }

    public void Dispose()
    {
        _stopped = true;
        _thread.Join(1000);
        foreach (var fd in _byDescriptor.Keys.ToArray()) Remove(fd);
        _ = close(_queue);
    }

    /// <summary>The soft limit on open descriptors; a conservative 256 when it cannot be read.</summary>
    static ulong DescriptorLimit()
    {
        var limit = new RLimit();
        return getrlimit(RlimitNofile, ref limit) == 0 && limit.Current > 0 ? limit.Current : 256;
    }

    /// <summary>How many files are watched right now (open descriptors held by this class).</summary>
    internal int WatchedCount => _byPath.Count;

    /// <summary>Descriptors this instance still owns; equals <see cref="WatchedCount"/> unless one leaked.</summary>
    internal int DescriptorCount => _byDescriptor.Count;

    [StructLayout(LayoutKind.Sequential)]
    struct RLimit
    {
        public ulong Current;
        public ulong Maximum;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct KEvent
    {
        public nuint Ident;
        public short Filter;
        public ushort Flags;
        public uint FFlags;
        public nint Data;
        public nint UserData;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct TimeSpec
    {
        public long Seconds;
        public long Nanoseconds;
    }

    [DllImport("libc")] static extern int kqueue();
    [DllImport("libc", EntryPoint = "kevent")] static extern int Register(int queue, ref KEvent change, int changeCount, IntPtr events, int eventCount, IntPtr timeout);
    [DllImport("libc", EntryPoint = "kevent")] static extern int Wait(int queue, IntPtr changes, int changeCount, [Out] KEvent[] events, int eventCount, ref TimeSpec timeout);
    [DllImport("libc", SetLastError = true)] static extern int open([MarshalAs(UnmanagedType.LPUTF8Str)] string path, int flags);
    [DllImport("libc")] static extern int close(int descriptor);
    [DllImport("libc")] static extern int getrlimit(int resource, ref RLimit limit);
}

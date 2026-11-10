using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Ara.CompileServer;

/// <summary>Gives human compiles the helper's launch budget and backgrounds agent/idle work.</summary>
internal static class CompilePriority
{
    const int DarwinProcess = 4;
    const int DarwinBackground = 0x1000;
    static readonly object Gate = new();
    static readonly Process Self = Process.GetCurrentProcess();
    static ProcessPriorityClass? _launchClass;
    static int? _launchBackground;
    static int _humans;
    [ThreadStatic] static bool _human;

    /// <summary>Applies a scheduling hint; failures are returned for the coordinator's log.</summary>
    internal static string? Apply(bool? agent)
    {
        string? failure = null;
        lock (Gate)
        {
            var human = agent == false;
            if (human != _human)
            {
                _human = human;
                _humans += human ? 1 : -1;
            }
            try
            {
                if (OperatingSystem.IsMacOS())
                {
                    // BSD nice cannot be raised back without privilege. Darwin's
                    // self-process background policy is reversible and covers Roslyn workers.
                    _launchBackground ??= Background();
                    var background = _humans > 0 ? _launchBackground.Value : 1;
                    if (Native.SetPriority(DarwinProcess, 0, background == 1 ? DarwinBackground : 0) != 0)
                        throw new Win32Exception(Marshal.GetLastPInvokeError(), "Cannot set compiler background policy");
                }
                else
                {
                    _launchClass ??= Self.PriorityClass;
                    var priority = _humans > 0 ? _launchClass.Value
                        : _launchClass == ProcessPriorityClass.Idle ? ProcessPriorityClass.Idle : ProcessPriorityClass.BelowNormal;
                    if (Self.PriorityClass != priority) Self.PriorityClass = priority;
                }
            }
            catch (Exception error)
            {
                failure = "Compiler process priority hint failed: " + error.Message;
            }
        }

        // A process-policy failure must not prevent restoring the worker-thread hint.
        try { Thread.CurrentThread.Priority = agent == true ? ThreadPriority.BelowNormal : ThreadPriority.Normal; }
        catch (Exception error) { failure ??= "Compiler thread priority hint failed: " + error.Message; }
        return failure;
    }

    static int Background()
    {
        var state = Native.GetPriority(DarwinProcess, 0);
        if (state is not (0 or 1))
            throw new Win32Exception(Marshal.GetLastPInvokeError(), "Cannot read compiler background policy");
        return state;
    }

    static class Native
    {
        [DllImport("/usr/lib/libSystem.B.dylib", EntryPoint = "getpriority", SetLastError = true)]
        internal static extern int GetPriority(int which, int who);

        [DllImport("/usr/lib/libSystem.B.dylib", EntryPoint = "setpriority", SetLastError = true)]
        internal static extern int SetPriority(int which, int who, int priority);
    }
}

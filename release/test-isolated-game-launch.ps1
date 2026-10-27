[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$Exe,
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$WorkingDirectory,
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$Report,
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$ExpectedTitle
)

# Real no-argument GUI launch on an owned desktop that never receives input.
# The caller supplies its disposable profile and temp environment before invocation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Isolated desktop launch requires Windows; no default-desktop fallback is permitted.' }

function Get-AbsoluteLaunchPath([string]$Value, [string]$Label) {
    if (-not [IO.Path]::IsPathRooted($Value) -or $Value.Contains('"')) { throw "$Label must be an absolute literal path." }
    $full = [IO.Path]::GetFullPath($Value)
    if ($full.StartsWith('\\') -or $full.Length -lt 3 -or $full[1] -ne ':') { throw "$Label must be an absolute local drive path." }
    return $full
}

function Assert-NoReparseAncestors([string]$Value, [string]$Label) {
    $current = $Value
    while (-not [string]::IsNullOrEmpty($current)) {
        if (([IO.File]::GetAttributes($current) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "$Label contains a reparse point; isolated launch refused: $current" }
        $parent = [IO.Path]::GetDirectoryName($current.TrimEnd('\'))
        if ([string]::IsNullOrEmpty($parent) -or $parent -eq $current) { break }
        $current = $parent
    }
}

$exePath = Get-AbsoluteLaunchPath $Exe 'Exe'
$cwdPath = Get-AbsoluteLaunchPath $WorkingDirectory 'WorkingDirectory'
$reportPath = Get-AbsoluteLaunchPath $Report 'Report'
if (-not [IO.File]::Exists($exePath) -or [IO.Path]::GetExtension($exePath) -ine '.exe') { throw "Exe is not an existing executable: $exePath" }
if (-not [IO.Directory]::Exists($cwdPath)) { throw "WorkingDirectory does not exist: $cwdPath" }
if (-not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($reportPath))) { throw 'Report parent must already exist in the caller-owned evidence directory.' }
if ([IO.File]::Exists($reportPath) -or [IO.Directory]::Exists($reportPath)) { throw "Report must be a fresh target: $reportPath" }
Assert-NoReparseAncestors $exePath 'Exe'
Assert-NoReparseAncestors $cwdPath 'WorkingDirectory'
Assert-NoReparseAncestors ([IO.Path]::GetDirectoryName($reportPath)) 'Report parent'

foreach ($name in @('ARA_CONFIG_DIR', 'ARA_CRASH_DIR', 'TEMP', 'TMP')) {
    $value = [Environment]::GetEnvironmentVariable($name, [EnvironmentVariableTarget]::Process)
    if ([string]::IsNullOrWhiteSpace($value)) { throw "Caller must provide isolated $name before launching." }
    $isolated = Get-AbsoluteLaunchPath $value $name
    if (-not [IO.Directory]::Exists($isolated)) { throw "Caller-owned $name directory does not exist: $isolated" }
    Assert-NoReparseAncestors $isolated $name
    foreach ($special in @([Environment+SpecialFolder]::ApplicationData, [Environment+SpecialFolder]::LocalApplicationData)) {
        $realAra = [IO.Path]::Combine([Environment]::GetFolderPath($special), 'Ara').TrimEnd('\')
        if ($isolated.Equals($realAra, [StringComparison]::OrdinalIgnoreCase) -or $isolated.StartsWith($realAra + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "$name points at the real Ara profile; isolated launch refused."
        }
    }
}

if (-not ('AraIsolatedLaunch.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace AraIsolatedLaunch {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct StartupInfo {
        public int cb;
        public string lpReserved, lpDesktop, lpTitle;
        public uint dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public ushort wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct ProcessInformation {
        public IntPtr hProcess, hThread;
        public uint dwProcessId, dwThreadId;
    }
    public sealed class WindowProbe {
        public string Title;
        public bool Responsive;
        public int MessageError;
        public int Messages;
        public double MessageLatencyMs;
        public System.Collections.Generic.List<string> OwnedTitles = new System.Collections.Generic.List<string>();
    }
    public static class Native {
        public const uint WaitTimeout = 0x102;
        public const uint WaitObject = 0;
        public const uint WindowMessageBoundMs = 1500;
        public const int ProbeGuardMs = 5000;
        public static volatile string ProbeStage = "not started";
        public static volatile uint ProbeThreadId;
        public static volatile int ProbeWindows;
        private static volatile System.Diagnostics.Stopwatch messageClock;
        public static double CurrentMessageElapsedMs { get { return messageClock == null ? 0 : messageClock.Elapsed.TotalMilliseconds; } }
        private delegate bool EnumWindowCallback(IntPtr window, IntPtr parameter);
        [DllImport("user32.dll", EntryPoint = "CreateDesktopW", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr CreateDesktop(string name, IntPtr device, IntPtr mode, uint flags, uint access, IntPtr attributes);
        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool CloseDesktop(IntPtr desktop);
        [DllImport("kernel32.dll", EntryPoint = "CreateProcessW", CharSet = CharSet.Unicode, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool CreateProcess(string application, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes,
            [MarshalAs(UnmanagedType.Bool)] bool inheritHandles, uint flags, IntPtr environment, string directory,
            ref StartupInfo startup, out ProcessInformation process);
        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll")]
        private static extern void SetLastError(uint code);
        [DllImport("kernel32.dll")]
        private static extern uint GetCurrentThreadId();
        [DllImport("dbghelp.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool MiniDumpWriteDump(IntPtr process, uint processId, IntPtr file, uint type,
            IntPtr exception, IntPtr streams, IntPtr callback);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern IntPtr GetThreadDesktop(uint thread);
        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetThreadDesktop(IntPtr desktop);
        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool GetExitCodeProcess(IntPtr process, out uint code);
        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool TerminateProcess(IntPtr process, uint code);
        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool CloseHandle(IntPtr handle);
        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool EnumDesktopWindows(IntPtr desktop, EnumWindowCallback callback, IntPtr parameter);
        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool IsWindowVisible(IntPtr window);
        [DllImport("user32.dll", EntryPoint = "GetWindowTextW", CharSet = CharSet.Unicode)]
        private static extern int GetWindowText(IntPtr window, StringBuilder title, int count);
        [DllImport("user32.dll", EntryPoint = "SendMessageTimeoutW", SetLastError = true)]
        private static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);

        public static void WriteFullDump(IntPtr process, uint processId, string path) {
            using (FileStream file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) {
                // Full accessible memory, unloaded modules, memory map and thread info.
                if (!MiniDumpWriteDump(process, processId, file.SafeFileHandle.DangerousGetHandle(), 0x1822,
                    IntPtr.Zero, IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot capture the exact owned game process.");
                file.Flush(true);
            }
        }

        public static WindowProbe Probe(IntPtr desktop, uint ownedProcess, string expectedTitle) {
            WindowProbe result = null;
            Exception failure = null;
            ProbeStage = "starting";
            ProbeWindows = 0;
            messageClock = null;
            // Window messages belong to a desktop. Only this fresh worker thread
            // attaches to ours; the input desktop is never switched or activated.
            Thread worker = new Thread(delegate() {
                ProbeThreadId = GetCurrentThreadId();
                ProbeStage = "get thread desktop";
                IntPtr previous = GetThreadDesktop(GetCurrentThreadId());
                try {
                    ProbeStage = "attach desktop";
                    if (!SetThreadDesktop(desktop)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot attach the isolated probe thread.");
                    result = ProbeAttached(desktop, ownedProcess, expectedTitle);
                } catch (Exception error) { failure = error; }
                finally {
                    ProbeStage = "detach desktop";
                    if (previous != IntPtr.Zero && !SetThreadDesktop(previous)) failure = new Win32Exception(Marshal.GetLastWin32Error(), "Cannot detach the isolated probe thread.");
                    ProbeStage = "complete";
                }
            });
            worker.IsBackground = true;
            worker.Start();
            if (!worker.Join(ProbeGuardMs)) throw new TimeoutException("Isolated desktop probe exceeded its bounded wait at " + ProbeStage + " (probe TID " + ProbeThreadId + ", windows " + ProbeWindows + ").");
            if (failure != null) throw failure;
            return result;
        }

        private static WindowProbe ProbeAttached(IntPtr desktop, uint ownedProcess, string expectedTitle) {
            WindowProbe probe = new WindowProbe();
            EnumWindowCallback callback = delegate(IntPtr window, IntPtr parameter) {
                ProbeWindows++;
                uint process;
                ProbeStage = "get window owner";
                GetWindowThreadProcessId(window, out process);
                if (process != ownedProcess) return true;
                ProbeStage = "get window visibility";
                if (!IsWindowVisible(window)) return true;
                StringBuilder title = new StringBuilder(4096);
                ProbeStage = "get window title";
                GetWindowText(window, title, title.Capacity);
                probe.OwnedTitles.Add(title.ToString());
                if (!String.Equals(title.ToString(), expectedTitle, StringComparison.Ordinal)) return true;
                UIntPtr reply;
                probe.Title = title.ToString();
                SetLastError(0);
                ProbeStage = "send WM_NULL";
                messageClock = System.Diagnostics.Stopwatch.StartNew();
                probe.Responsive = SendMessageTimeout(window, 0, UIntPtr.Zero, IntPtr.Zero, 0x3, WindowMessageBoundMs, out reply) != IntPtr.Zero;
                probe.MessageError = Marshal.GetLastWin32Error();
                messageClock.Stop();
                probe.Messages++;
                probe.MessageLatencyMs = Math.Max(probe.MessageLatencyMs, messageClock.Elapsed.TotalMilliseconds);
                ProbeStage = "WM_NULL returned";
                return true;
            };
            SetLastError(0);
            ProbeStage = "enumerate desktop windows";
            if (!EnumDesktopWindows(desktop, callback, IntPtr.Zero)) {
                int error = Marshal.GetLastWin32Error();
                // A newly created desktop may not have any windows yet.
                if (error != 0) throw new Win32Exception(error, "Cannot enumerate the isolated desktop (" + error + ").");
            }
            ProbeStage = "enumeration returned";
            GC.KeepAlive(callback);
            return probe;
        }
    }
}
'@
}

$desktopName = 'ara-export-' + [Guid]::NewGuid().ToString('N')
$desktopHandle = [IntPtr]::Zero
$processInfo = New-Object AraIsolatedLaunch.ProcessInformation
$passed = $false
$failure = $null
$title = $null
$ownedPid = $null
$messageError = 0
$dump = $null
$probeState = $null
$samples = New-Object 'System.Collections.Generic.List[object]'
$maxPumpLatencyMs = $null
$maxProbeDurationMs = 0.0
$probeWatch = $null
$reportStream = [IO.File]::Open($reportPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
try {
    # No switch-desktop access is requested. Only the child sees this desktop.
    $desktopHandle = [AraIsolatedLaunch.Native]::CreateDesktop($desktopName, [IntPtr]::Zero, [IntPtr]::Zero, 0, 0xC7, [IntPtr]::Zero)
    if ($desktopHandle -eq [IntPtr]::Zero) { throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error(), 'Cannot create an isolated desktop; launch refused.') }
    $startup = New-Object AraIsolatedLaunch.StartupInfo
    $startup.cb = [Runtime.InteropServices.Marshal]::SizeOf($startup)
    $startup.lpDesktop = $desktopName
    $startup.dwFlags = 1
    $startup.wShowWindow = 1
    $commandLine = [Text.StringBuilder]::new('"' + $exePath + '"')
    # Null environment inherits exactly the caller's already isolated environment.
    if (-not [AraIsolatedLaunch.Native]::CreateProcess($exePath, $commandLine, [IntPtr]::Zero, [IntPtr]::Zero, $false, 0, [IntPtr]::Zero, $cwdPath, [ref]$startup, [ref]$processInfo)) {
        throw [ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error(), 'Cannot launch the game on its isolated desktop.')
    }
    $ownedPid = $processInfo.dwProcessId
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    $responsiveFor = New-Object Diagnostics.Stopwatch
    while ($deadline.Elapsed.TotalSeconds -lt 30) {
        $state = [AraIsolatedLaunch.Native]::WaitForSingleObject($processInfo.hProcess, 0)
        if ($state -ne [AraIsolatedLaunch.Native]::WaitTimeout) {
            [uint32]$exitCode = 0
            if (-not [AraIsolatedLaunch.Native]::GetExitCodeProcess($processInfo.hProcess, [ref]$exitCode)) { throw 'The owned game process handle became unreadable.' }
            throw "Game exited before the launch proof completed (exit code $exitCode)."
        }
        $probeWatch = [Diagnostics.Stopwatch]::StartNew()
        $probe = [AraIsolatedLaunch.Native]::Probe($desktopHandle, $ownedPid, $ExpectedTitle)
        $probeWatch.Stop()
        $maxProbeDurationMs = [Math]::Max($maxProbeDurationMs, $probeWatch.Elapsed.TotalMilliseconds)
        $latency = if ($probe.Messages -gt 0) { $probe.MessageLatencyMs } else { $null }
        if ($null -ne $latency) { $maxPumpLatencyMs = [Math]::Max([double]$maxPumpLatencyMs, $latency) }
        $samples.Add([ordered]@{ elapsedMs = $deadline.Elapsed.TotalMilliseconds; responsive = $probe.Responsive; pumpLatencyMs = $latency; probeDurationMs = $probeWatch.Elapsed.TotalMilliseconds; messages = $probe.Messages; messageError = $probe.MessageError; windowsVisited = [AraIsolatedLaunch.Native]::ProbeWindows; windowTitles = [object[]]$probe.OwnedTitles.ToArray() })
        $messageError = $probe.MessageError
        if ($deadline.Elapsed.TotalSeconds -ge 30) { break }
        if ($probe.Responsive) {
            $title = $probe.Title
            if (-not $responsiveFor.IsRunning) { $responsiveFor.Start() }
            if ($responsiveFor.Elapsed.TotalSeconds -ge 3) { $passed = $true; break }
        } else { $responsiveFor.Reset() }
        Start-Sleep -Milliseconds 100
    }
    if (-not $passed) { throw "No responsive '$ExpectedTitle' window stayed alive for three seconds within the thirty-second deadline (window message error $messageError)." }
} catch {
    $failure = $_.Exception.Message
    if ($null -ne $probeWatch -and $probeWatch.IsRunning) {
        $probeWatch.Stop()
        $maxProbeDurationMs = [Math]::Max($maxProbeDurationMs, $probeWatch.Elapsed.TotalMilliseconds)
        $samples.Add([ordered]@{ elapsedMs = $deadline.Elapsed.TotalMilliseconds; responsive = $false; pumpLatencyMs = $null; probeDurationMs = $probeWatch.Elapsed.TotalMilliseconds; stage = [AraIsolatedLaunch.Native]::ProbeStage; incomplete = $true })
    }
    $probeState = [ordered]@{ stage = [AraIsolatedLaunch.Native]::ProbeStage; threadId = [AraIsolatedLaunch.Native]::ProbeThreadId; windowsVisited = [AraIsolatedLaunch.Native]::ProbeWindows; currentMessageElapsedMs = [AraIsolatedLaunch.Native]::CurrentMessageElapsedMs }
} finally {
    if ($processInfo.hProcess -ne [IntPtr]::Zero) {
        if (-not $passed -and [AraIsolatedLaunch.Native]::WaitForSingleObject($processInfo.hProcess, 0) -eq [AraIsolatedLaunch.Native]::WaitTimeout) {
            $dumpPath = [IO.Path]::ChangeExtension($reportPath, '.dmp')
            try {
                [AraIsolatedLaunch.Native]::WriteFullDump($processInfo.hProcess, $processInfo.dwProcessId, $dumpPath)
                $dump = [ordered]@{ path = $dumpPath; type = 'full'; captured = $true; error = $null }
            } catch { $dump = [ordered]@{ path = $dumpPath; type = 'full'; captured = $false; error = $_.Exception.Message } }
        }
        if ([AraIsolatedLaunch.Native]::WaitForSingleObject($processInfo.hProcess, 0) -eq [AraIsolatedLaunch.Native]::WaitTimeout) {
            if (-not [AraIsolatedLaunch.Native]::TerminateProcess($processInfo.hProcess, 0xA11)) { $passed = $false; $failure = 'Could not terminate the exact owned game process handle.' }
        }
        if ([AraIsolatedLaunch.Native]::WaitForSingleObject($processInfo.hProcess, 5000) -ne [AraIsolatedLaunch.Native]::WaitObject) { $passed = $false; $failure = 'The owned game process did not exit during bounded cleanup.' }
    }
    if ($processInfo.hThread -ne [IntPtr]::Zero) { [void][AraIsolatedLaunch.Native]::CloseHandle($processInfo.hThread) }
    if ($processInfo.hProcess -ne [IntPtr]::Zero) { [void][AraIsolatedLaunch.Native]::CloseHandle($processInfo.hProcess) }
    if ($desktopHandle -ne [IntPtr]::Zero -and -not [AraIsolatedLaunch.Native]::CloseDesktop($desktopHandle)) { $passed = $false; $failure = 'Could not close the exact owned isolated desktop.' }
    if ($null -eq $probeState) { $probeState = [ordered]@{ stage = [AraIsolatedLaunch.Native]::ProbeStage; threadId = [AraIsolatedLaunch.Native]::ProbeThreadId; windowsVisited = [AraIsolatedLaunch.Native]::ProbeWindows } }
    $result = [ordered]@{ passed = $passed; exe = $exePath; cwd = $cwdPath; desktop = $desktopName; pid = $ownedPid; mainThreadId = $processInfo.dwThreadId; title = $title; arguments = [object[]]@(); visibleDesktopUsed = $false; doubleClickVerified = $false; error = $failure; dump = $dump; probe = $probeState; windowMessageBoundMs = [AraIsolatedLaunch.Native]::WindowMessageBoundMs; probeGuardMs = [AraIsolatedLaunch.Native]::ProbeGuardMs; launchDeadlineMs = 30000; responsivePeriodMs = 3000; latencyMeasure = 'WM_NULL round trip per probe, not internal frame duration'; maxPumpLatencyMs = $maxPumpLatencyMs; maxProbeDurationMs = $maxProbeDurationMs; iterations = $samples.ToArray() }
    try {
        $json = ($result | ConvertTo-Json -Depth 4) + "`n"
        $encoding = [Text.UTF8Encoding]::new($false)
        $bytes = $encoding.GetBytes($json)
        $reportStream.Write($bytes, 0, $bytes.Length)
        $reportStream.Flush($true)
    } finally { $reportStream.Dispose() }
}
if (-not $passed) { throw "Isolated game launch failed: $failure Report: $reportPath" }
Write-Output $reportPath

[CmdletBinding()]
param([string]$Distribution, [string]$Output)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Distribution) { $Distribution = Join-Path $repo 'target/release-dist/ara-0.0.1-alpha-windows-x64' }
if (-not $Output) { $Output = Join-Path $repo ('target/release-dist/crash-recovery-' + $PID) }
$Distribution = [IO.Path]::GetFullPath($Distribution)
$Output = [IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $Output) { throw 'Use a new evidence directory' }
New-Item -ItemType Directory -Path $Output | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class AraRecoveryWindows {
    public delegate bool Visitor(IntPtr window, IntPtr state);
    [DllImport("user32.dll")] public static extern bool EnumWindows(Visitor visitor, IntPtr state);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    public static IntPtr Find(uint pid, string title, bool prefix) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, state) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner != pid) return true;
            var text = new StringBuilder(512); GetWindowText(window, text, text.Capacity);
            if (prefix ? text.ToString().StartsWith(title) : text.ToString() == title) { found = window; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
function Start-TestProcess([string]$Executable, [string]$Arguments) {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $Executable
    $start.Arguments = $Arguments
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['ARA_CONFIG_DIR'] = Join-Path $Output 'config'
    $start.EnvironmentVariables['ARA_CRASH_DIR'] = Join-Path $Output 'crashes'
    $start.EnvironmentVariables['APPDATA'] = Join-Path $Output 'appdata'
    $start.EnvironmentVariables['GIT_CEILING_DIRECTORIES'] = $Output
    $start.EnvironmentVariables['RUST_BACKTRACE'] = '0'
    $start.EnvironmentVariables['VK_IMPLICIT_LAYER_PATH'] = Join-Path $Output 'empty-layers'
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    if (-not $process.Start()) { throw 'Could not start verification process' }
    return $process
}
New-Item -ItemType Directory -Path (Join-Path $Output 'empty-layers') | Out-Null
$probe = Start-TestProcess (Join-Path $repo 'target/debug/ara-crash.exe') ('probe panic "' + (Join-Path $Output 'crashes') + '"')
if (-not $probe.WaitForExit(15000)) { $probe.Kill(); throw 'Crash probe timed out' }
[IO.File]::WriteAllText((Join-Path $Output 'probe.log'), $probe.StandardError.ReadToEnd())
if ($probe.ExitCode -eq 0 -or -not (Get-ChildItem -LiteralPath (Join-Path $Output 'crashes') -Filter '*.dmp')) { throw 'Probe did not produce a native dump' }
$project = Join-Path $Output 'project'
$init = Start-TestProcess (Join-Path $Distribution 'ara.exe') ('init "' + $project + '" --name "Crash recovery verification"')
if (-not $init.WaitForExit(15000)) { $init.Kill(); throw 'Project creation timed out' }
if ($init.ExitCode -ne 0) { throw $init.StandardError.ReadToEnd() }
$editor = Start-TestProcess (Join-Path $Distribution 'ara-editor.exe') ('"' + $project + '"')
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do { $dialog = [AraRecoveryWindows]::Find($editor.Id, 'Ara crash report', $false); if ($dialog -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 } } while ($dialog -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline -and -not $editor.HasExited)
    if ($dialog -eq [IntPtr]::Zero) { throw 'Next launch did not offer to open the crash folder' }
    $rect = New-Object AraRecoveryWindows+Rect
    if (-not [AraRecoveryWindows]::GetWindowRect($dialog, [ref]$rect)) { throw 'Cannot measure the recovery dialog' }
    $bitmap = New-Object Drawing.Bitmap ($rect.Right - $rect.Left),($rect.Bottom - $rect.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    try { if (-not [AraRecoveryWindows]::PrintWindow($dialog, $hdc, 0)) { throw 'Cannot capture the recovery dialog' } }
    finally { $graphics.ReleaseHdc($hdc); $graphics.Dispose() }
    $bitmap.Save((Join-Path $Output 'crash-folder-offer.png'), [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    # IDNO closes only this test process's message box, without opening Explorer.
    if (-not [AraRecoveryWindows]::PostMessage($dialog, 0x0111, [IntPtr]7, [IntPtr]::Zero)) { throw 'Cannot decline the folder offer' }
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do { $window = [AraRecoveryWindows]::Find($editor.Id, 'Ara | ', $true); if ($window -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 } } while ($window -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline -and -not $editor.HasExited)
    if ($window -eq [IntPtr]::Zero) { throw 'Editor did not launch after declining recovery' }
    if (-not [AraRecoveryWindows]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'Cannot close the test editor' }
    if (-not $editor.WaitForExit(20000) -or $editor.ExitCode -ne 0) { throw 'Editor did not close cleanly' }
    if (-not (Test-Path -LiteralPath (Join-Path $Output 'crashes/.ara-editor-seen'))) { throw 'Recovery notification was not marked seen' }
    [IO.File]::WriteAllText((Join-Path $Output 'RESULT.txt'), "PASS: real panic and minidump captured locally; next interactive editor launch offered the crash folder; declining continued into the native editor; normal close succeeded.`n")
    Write-Output ('Crash recovery PASS: ' + $Output)
} finally {
    if (-not $editor.HasExited) { $editor.Kill(); $editor.WaitForExit() }
    [IO.File]::WriteAllText((Join-Path $Output 'editor.log'), $editor.StandardError.ReadToEnd())
}

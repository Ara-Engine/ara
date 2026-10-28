# Manual opt-in. Uses the editor's production protocol/capture jobs; no probe frame hook.
# Launch editor on this scratch Project with --panel console --screenshot --frames 10000000.
param([Parameter(Mandatory=$true)][string]$Project, [Parameter(Mandatory=$true)][string]$Root, [int]$StartupTimeoutSeconds = 2700)
$ErrorActionPreference = 'Stop'
$source = Join-Path $Root 'crates/ara-editor/src/shell/panels.rs'
$original = [IO.File]::ReadAllText($source)
$old = '"console" => self.console_panel(editor, out),'
$new = '"console" => self.empty(out, "rust-live", "terminal", "RUST PATCH LIVE", "The editor process stayed alive.", &[]),'
if (($original.Split(@($old), [StringSplitOptions]::None)).Length -ne 2) { throw 'editor panel anchor is not unique' }
$encoding = [Text.UTF8Encoding]::new($false)
$socket = [Net.WebSockets.ClientWebSocket]::new()
$script:nextRpc = 0
function Rpc([string]$Method, $Arguments) {
    $script:nextRpc += 1
    $id = $script:nextRpc
    $bytes = [Text.Encoding]::UTF8.GetBytes((@{jsonrpc='2.0';id=$id;method=$Method;params=$Arguments} | ConvertTo-Json -Compress -Depth 12))
    $cancel = [Threading.CancellationTokenSource]::new(30000)
    try {
        $socket.SendAsync([ArraySegment[byte]]::new($bytes), [Net.WebSockets.WebSocketMessageType]::Text, $true, $cancel.Token).GetAwaiter().GetResult()
        while ($true) {
            $message = [Text.StringBuilder]::new()
            do {
                $buffer = [byte[]]::new(65536)
                $received = $socket.ReceiveAsync([ArraySegment[byte]]::new($buffer), $cancel.Token).GetAwaiter().GetResult()
                if ($received.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Close) { throw 'editor protocol closed' }
                [void]$message.Append([Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count))
            } until ($received.EndOfMessage)
            $response = $message.ToString() | ConvertFrom-Json
            if ($response.id -ne $id) { continue }
            if ($null -ne $response.error) { throw ($response.error | ConvertTo-Json -Compress -Depth 12) }
            return $response.result
        }
    } finally { $cancel.Dispose() }
}
function Capture([string]$Target, [string]$Path) {
    $job = (Rpc 'capture' @{session=$session; target=$Target; path=$Path}).job
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        $status = Rpc 'jobs.status' @{session=$session; job=$job}
        if ($status.status -eq 'done') { return $status.result }
        if ($status.status -ne 'running') { throw ($status | ConvertTo-Json -Compress -Depth 12) }
        Start-Sleep -Milliseconds 20
    } while ($timer.Elapsed.TotalSeconds -lt 60)
    throw 'editor capture job timeout'
}
$results = [Collections.Generic.List[object]]::new()
$session = $null
try {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $daemonPath = Join-Path $Project '.ara/daemon.json'
    while (-not (Test-Path -LiteralPath $daemonPath)) {
        if ($timer.Elapsed.TotalSeconds -gt $StartupTimeoutSeconds) { throw 'editor startup timeout' }
        Start-Sleep -Milliseconds 100
    }
    $info = Get-Content -LiteralPath $daemonPath -Raw | ConvertFrom-Json
    $hostPid = $info.pid
    $socket.Options.SetRequestHeader('Authorization', "Bearer $($info.token)")
    $socket.ConnectAsync([Uri]"ws://127.0.0.1:$($info.port)/", [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $session = (Rpc 'session.begin' @{client='hotpatch-proof';author='agent:hotpatch-proof'}).session
    $baseline = Capture 'editor' '.ara/hotpatch-proof/baseline.png'
    $baseline | ConvertTo-Json -Compress
    Start-Sleep -Milliseconds 500
    $poll = 0
    foreach ($phase in @('body', 'recovery', 'body-repeat', 'restore')) {
        $changed = $phase -in @('body', 'body-repeat')
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $text = if ($changed) { $original.Replace($old, $new) } else { $original }
        [IO.File]::WriteAllText($source, $text, $encoding)
        do {
            $poll += 1
            $layout = Capture 'layout' ".ara/hotpatch-proof/poll-$poll.json"
            $report = [IO.File]::ReadAllText($layout.path)
            if ($report.Contains('RUST PATCH LIVE') -eq $changed) { break }
            if ($timer.Elapsed.TotalSeconds -gt 90) { throw "editor panel patch timeout ($phase)" }
            Start-Sleep -Milliseconds 40
        } while ($true)
        $capture = Capture 'editor' ".ara/hotpatch-proof/$phase.png"
        $observed = $timer.Elapsed.TotalMilliseconds
        $now = Get-Content -LiteralPath $daemonPath -Raw | ConvertFrom-Json
        if ($now.pid -ne $hostPid) { throw 'editor restarted during panel patch' }
        $row = [ordered]@{phase=$phase; observed_ms=$observed;pid=$hostPid;session=$session;image=$capture.path;layout=$capture.layout;marker=$changed}
        $results.Add($row)
        $row | ConvertTo-Json -Compress -Depth 8
        $results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Project '.ara/hotpatch-proof/measurements.json')
    }
} finally {
    [IO.File]::WriteAllText($source, $original, $encoding)
    if ($null -ne $session -and $socket.State -eq [Net.WebSockets.WebSocketState]::Open) {
        try { [void](Rpc 'editor.command' @{session=$session;id='file.exit'}) } catch { Write-Warning $_ }
    }
    $socket.Dispose()
}

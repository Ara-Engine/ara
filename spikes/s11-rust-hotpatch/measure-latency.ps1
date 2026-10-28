# Explicit opt-in: edits one owned Rust body and restores it. Never a Cargo test.
param(
    [Parameter(Mandatory=$true)][ValidateSet('sim','fidelity','editor')][string]$Kind,
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$Output,
    [Parameter(Mandatory=$true)][string]$Experiment,
    [string]$Project,
    [string[]]$Modes = @('baseline','cgu16','cgu64','incremental-off','lld-threads4'),
    [int]$Samples = 4,
    [ValidateRange(0,500)][int]$WriteHoldMs = 0
)
$ErrorActionPreference = 'Stop'
$encoding = [Text.UTF8Encoding]::new($false)
$crate = switch ($Kind) { sim { 'ara_hotpatch_sim_probe' } fidelity { 'ara_render_fidelity' } editor { 'ara_editor' } }
$tip = switch ($Kind) { sim { 'ara_hotpatch_sim_probe' } fidelity { 'ara_hotpatch_probe' } editor { 'ara_editor_hotpatch' } }
$relative = switch ($Kind) { sim { 'crates/ara-hotpatch/sim-probe/main.rs' } fidelity { 'crates/ara-render-fidelity/src/render.rs' } editor { 'crates/ara-editor/src/shell/panels.rs' } }
$source = Join-Path $Root $relative
$original = [IO.File]::ReadAllText($source)
$old = switch ($Kind) { sim { 'transform.position[0] += 1.;' } fidelity { 'let (frame, vp, camera) = self.frame(input)?;' } editor { '"console" => self.console_panel(editor, out),' } }
$new = switch ($Kind) {
    sim { 'transform.position[0] += 2.;' }
    fidelity { 'let original = self.settings.post.compensation; self.settings.post.compensation = original + 3.; let recorded = self.frame(input); self.settings.post.compensation = original; let (frame, vp, camera) = recorded?;' }
    editor { '"console" => self.empty(out, "rust-live", "terminal", "RUST PATCH LIVE", "The editor process stayed alive.", &[]),' }
}
if (($original.Split(@($old),[StringSplitOptions]::None)).Length -ne 2) { throw 'body anchor is not unique' }
[IO.Directory]::CreateDirectory($Output) | Out-Null
$socket = [Net.WebSockets.ClientWebSocket]::new()
$session = $null
$script:nextRpc = 0
function Rpc([string]$Method, $Arguments) {
    $script:nextRpc += 1
    $id = $script:nextRpc
    $bytes = [Text.Encoding]::UTF8.GetBytes((@{jsonrpc='2.0';id=$id;method=$Method;params=$Arguments} | ConvertTo-Json -Compress -Depth 12))
    $cancel = [Threading.CancellationTokenSource]::new(30000)
    try {
        [void]$socket.SendAsync([ArraySegment[byte]]::new($bytes),[Net.WebSockets.WebSocketMessageType]::Text,$true,$cancel.Token).GetAwaiter().GetResult()
        while ($true) {
            $message = [Text.StringBuilder]::new()
            do {
                $buffer = [byte[]]::new(65536)
                $received = $socket.ReceiveAsync([ArraySegment[byte]]::new($buffer),$cancel.Token).GetAwaiter().GetResult()
                if ($received.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Close) { throw 'editor protocol closed' }
                [void]$message.Append([Text.Encoding]::UTF8.GetString($buffer,0,$received.Count))
            } until ($received.EndOfMessage)
            $response = $message.ToString() | ConvertFrom-Json
            if ($response.id -ne $id) { continue }
            if ($null -ne $response.error) { throw ($response.error | ConvertTo-Json -Compress -Depth 12) }
            return $response.result
        }
    } finally { $cancel.Dispose() }
}
function Capture([string]$Target,[string]$Path) {
    $job = (Rpc 'capture' @{session=$session;target=$Target;path=$Path}).job
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        $status = Rpc 'jobs.status' @{session=$session;job=$job}
        if ($status.status -eq 'done') { return $status.result }
        if ($status.status -ne 'running') { throw ($status | ConvertTo-Json -Compress -Depth 12) }
        if ($timer.Elapsed.TotalSeconds -gt 60) { throw 'capture timeout' }
        Start-Sleep -Milliseconds 20
    } while ($true)
}
function Read-State {
    try { Get-Content -LiteralPath (Join-Path $Output 'status.json') -Raw | ConvertFrom-Json } catch { $null }
}
function Unix-Us { [int64]([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) * 1000 }
$rows = [Collections.Generic.List[object]]::new()
$changed = $false
try {
    $startup = [Diagnostics.Stopwatch]::StartNew()
    if ($Kind -eq 'editor') {
        $daemon = Join-Path $Project '.ara/daemon.json'
        while (-not (Test-Path -LiteralPath $daemon)) {
            if ($startup.Elapsed.TotalSeconds -gt 2700) { throw 'editor startup timeout' }
            Start-Sleep -Milliseconds 100
        }
        $info = Get-Content -LiteralPath $daemon -Raw | ConvertFrom-Json
        $hostPid = $info.pid
        $socket.Options.SetRequestHeader('Authorization',"Bearer $($info.token)")
        [void]$socket.ConnectAsync([Uri]"ws://127.0.0.1:$($info.port)/",[Threading.CancellationToken]::None).GetAwaiter().GetResult()
        $session = (Rpc 'session.begin' @{client='hotpatch-latency';author='agent:hotpatch-latency'}).session
        [void](Capture 'editor' '.ara/hotpatch-proof/latency-baseline.png')
    } else {
        do {
            $state = Read-State
            if ($startup.Elapsed.TotalSeconds -gt 1800) { throw 'probe startup timeout' }
            Start-Sleep -Milliseconds 100
        } until ($null -ne $state -and $state.frame -ge 3)
        $hostPid = $state.pid
        $entity = $state.entity
        $generation = [uint64]$state.rust.generation
        if ($Kind -eq 'fidelity') { $baselineHash = (Get-FileHash -LiteralPath (Join-Path $Output "generation-$generation.png")).Hash }
    }
    Start-Sleep -Milliseconds 1000
    foreach ($mode in $Modes) {
        $settings = @{}
        switch ($mode) {
            baseline { }
            { $_ -in @('cgu1','cgu4','cgu16','cgu64') } { $settings.crates = @{}; $settings.crates[$crate] = @{codegen_units=[int]$mode.Substring(3)} }
            'tip-cgu16' { $settings.crates = @{}; $settings.crates[$tip] = @{codegen_units=16} }
            'both-cgu16' { $settings.crates = @{}; $settings.crates[$tip] = @{codegen_units=16}; $settings.crates[$crate] = @{codegen_units=16} }
            'incremental-off' { $settings.crates = @{}; $settings.crates[$crate] = @{incremental=$false} }
            'lld-threads4' { $settings.link_threads = 4 }
            'lld-no-debug' { $settings.link_debug = $false }
            default { throw "unknown experiment $mode" }
        }
        [IO.File]::WriteAllText($Experiment,($settings | ConvertTo-Json -Compress -Depth 8),$encoding)
        for ($index=0; $index -lt $Samples; $index++) {
            $changed = -not $changed
            if ($Kind -ne 'editor') { $previous = [uint64](Read-State).rust.generation }
            $saved = Unix-Us
            $timer = [Diagnostics.Stopwatch]::StartNew()
            $contents = if ($changed) { $original.Replace($old,$new) } else { $original }
            if ($WriteHoldMs -gt 0) {
                # Deliberately expose truncate-before-write while the writer owns
                # its handle. The native watcher must wait for a complete save.
                $writer = [IO.FileStream]::new($source,[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::Read)
                try {
                    Start-Sleep -Milliseconds $WriteHoldMs
                    $bytes = $encoding.GetBytes($contents)
                    $writer.Write($bytes,0,$bytes.Length)
                } finally { $writer.Dispose() }
            } else { [IO.File]::WriteAllText($source,$contents,$encoding) }
            do {
                if ($timer.Elapsed.TotalSeconds -gt 180) { throw "patch timeout $mode/$index" }
                if ($Kind -eq 'editor') {
                    $capture = Capture 'layout' ".ara/hotpatch-proof/latency-$mode-$index.json"
                    $matches = ([IO.File]::ReadAllText($capture.path).Contains('RUST PATCH LIVE') -eq $changed)
                } else {
                    $state = Read-State
                    $matches = $null -ne $state -and $state.rust.generation -gt $previous
                    if ($matches -and $Kind -eq 'sim') {
                        $matches = $state.delta -eq $(if ($changed) { 2 } else { 1 })
                        if ($state.entity -ne $entity -or -not $state.guards_verified) { throw 'ECS state/guards lost' }
                    } elseif ($matches) {
                        $image = Join-Path $Output "generation-$($state.rust.generation).png"
                        try { $matches = (((Get-FileHash -LiteralPath $image).Hash -ne $baselineHash) -eq $changed) } catch { $matches=$false }
                    }
                    if ($null -ne $state -and $state.pid -ne $hostPid) { throw 'host restarted' }
                }
                if (-not $matches) { Start-Sleep -Milliseconds 40 }
            } until ($matches)
            if ($Kind -eq 'editor') { [void](Capture 'editor' ".ara/hotpatch-proof/latency-$mode-$index.png") }
            $row = [ordered]@{kind=$Kind;mode=$mode;sample=$index;configuration_first=($index -eq 0);write_hold_ms=$WriteHoldMs;save_us=$saved;observed_us=(Unix-Us);observed_ms=$timer.Elapsed.TotalMilliseconds;pid=$hostPid;changed=$changed;generation=$(if($Kind -eq 'editor'){$null}else{$state.rust.generation})}
            $rows.Add($row)
            $row | ConvertTo-Json -Compress
            $rows | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Output 'latency.json')
        }
    }
} finally {
    if ([IO.File]::ReadAllText($source) -ne $original) { [IO.File]::WriteAllText($source,$original,$encoding) }
    [IO.File]::WriteAllText($Experiment,'{}',$encoding)
    if ($Kind -eq 'editor') {
        if ($null -ne $session -and $socket.State -eq [Net.WebSockets.WebSocketState]::Open) { [void](Rpc 'editor.command' @{session=$session;id='file.exit'}) }
    } else { [IO.File]::WriteAllText((Join-Path $Output 'stop'),'stop',$encoding) }
    $socket.Dispose()
}

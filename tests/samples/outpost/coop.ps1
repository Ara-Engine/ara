param(
    [string]$Bin = $(if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR 'debug' } else { Join-Path $PSScriptRoot '../../../target/debug' }),
    [string]$Output = $(Join-Path $PSScriptRoot "../../../target/outpost-coop/$([guid]::NewGuid().ToString('N'))"),
    [ValidateSet('all','local','listen','dedicated')][string]$Mode = 'all',
    [string]$WarmCache = '',
    [string]$ExportRoot = ''
)
$ErrorActionPreference = 'Stop'
$player = [IO.Path]::GetFullPath($(if ($ExportRoot) { Join-Path $ExportRoot 'Outpost.exe' } else { Join-Path $Bin 'ara-player.exe' }))
if (!(Test-Path -LiteralPath $player)) { throw "Build ara-player and ara-import-worker through ara-dev first: $player" }
$source = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../../samples/outpost'))
$Output = [IO.Path]::GetFullPath($Output)
$project = Join-Path $Output 'project'
New-Item -ItemType Directory -Path $project -Force | Out-Null

# Authoring files and assets are read-only inputs. Every generated cache, script
# assembly, log, profile, capture and replay belongs to this fresh target directory.
function Copy-Project([string]$From, [string]$To) {
    New-Item -ItemType Directory -Path $To -Force | Out-Null
    foreach ($item in Get-ChildItem -LiteralPath $From -Force) {
        if ($item.Name -in @('.ara','.scratch','builds','evidence','tests')) { continue }
        $destination = Join-Path $To $item.Name
        if ($item.PSIsContainer) { Copy-Project $item.FullName $destination }
        else { Copy-Item -LiteralPath $item.FullName -Destination $destination }
    }
}
if (!$ExportRoot) {
    Copy-Project $source $project
    if ($WarmCache) { Copy-Project ([IO.Path]::GetFullPath($WarmCache)) (Join-Path $project '.ara/cache') }
}

function Read-Report([string]$Path) {
    if (!(Test-Path -LiteralPath $Path)) { return $null }
    $reader = $null
    try {
        $stream = [IO.FileStream]::new($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
        $reader = [IO.StreamReader]::new($stream)
        $reader.ReadToEnd() | ConvertFrom-Json
    } catch { $null } finally { if ($reader) { $reader.Dispose() } }
}
function Assert-That([bool]$Condition,[string]$Message) { if (!$Condition) { throw $Message } }

function Run-Crew([string]$Kind) {
    $evidence = Join-Path $Output $Kind
    New-Item -ItemType Directory -Path $evidence -Force | Out-Null
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    $children = [Collections.Generic.List[Diagnostics.Process]]::new()
    $env:ARA_CONFIG_DIR = Join-Path $evidence 'profile'
    $env:ARA_CRASH_DIR = Join-Path $evidence 'crashes'
    $env:ARA_DOTNET_ARTIFACTS = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../../dotnet/artifacts/bin'))
    $env:ARA_IMPORT_WORKER = [IO.Path]::GetFullPath((Join-Path $Bin 'ara-import-worker.exe'))
    $env:VK_IMPLICIT_LAYER_PATH = Join-Path $evidence 'empty-layers'
    New-Item -ItemType Directory -Path $env:VK_IMPLICIT_LAYER_PATH -Force | Out-Null
    if ($env:VULKAN_SDK) { $env:VK_LAYER_PATH = Join-Path $env:VULKAN_SDK 'Bin' }
    function Launch-Bot([string]$Tag,[string]$Extra) {
        $content = if ($ExportRoot) { "--root `"$ExportRoot`"" } else { "--project `"$project`"" }
        $arguments = "--game $content --headless --bot --port $port --evidence `"$evidence`" $Extra"
        $process = Start-Process -FilePath $player -ArgumentList $arguments -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $evidence "$Tag.stdout.log") -RedirectStandardError (Join-Path $evidence "$Tag.stderr.log")
        $null = $process.Handle
        $children.Add($process)
    }
    try {
        Launch-Bot 'authority' $(if ($Kind -eq 'dedicated') { '--dedicated' } elseif ($Kind -eq 'local') { '--bot-solo' } else { '' })
        $authority = if ($Kind -eq 'dedicated') { 0 } else { 1 }
        $deadline = [DateTime]::UtcNow.AddSeconds(600)
        $early = if ($Kind -eq 'dedicated') { 3 } elseif ($Kind -eq 'local') { 0 } else { 2 }
        $launched = $false
        $late = $false
        $joinTick = 0
        while ([DateTime]::UtcNow -lt $deadline) {
            $report = Read-Report (Join-Path $evidence "player-$authority.json")
            if ($report -and !$launched) {
                # Offline verification hashes every tick twice; it is intentionally
                # slower than live play on the full art scene.
                $deadline = [DateTime]::UtcNow.AddSeconds(900)
                for ($i=0; $i -lt $early; $i++) { Launch-Bot "client-$i" "--connect 127.0.0.1:$port" }
                $launched = $true
            }
            if ($Kind -ne 'local' -and $report -and !$late -and $report.doors[0].open) {
                $joinTick = $report.tick
                Launch-Bot 'late-client' "--connect 127.0.0.1:$port"
                $late = $true
            }
            $running = @($children | Where-Object { !$_.HasExited })
            foreach ($child in $children) { if ($child.HasExited) {$child.WaitForExit(); if ($child.ExitCode -ne 0) { throw "Outpost bot PID $($child.Id) exited $($child.ExitCode); see $evidence" }} }
            if ($children.Count -gt 0 -and $running.Count -eq 0) { break }
            Start-Sleep -Milliseconds 100
        }
        Assert-That ($late -or $Kind -eq 'local') 'The fourth bot must join after a door was opened in the running game.'
        Assert-That (@($children | Where-Object { !$_.HasExited }).Count -eq 0) "Co-op timed out; inspect $evidence"
        $players = if ($Kind -eq 'dedicated') { @(0,2,3,4,5) } elseif ($Kind -eq 'local') { @(1) } else { @(1,2,3,4) }
        foreach ($slot in $players) {
            $report = Read-Report (Join-Path $evidence "player-$slot.json")
            Assert-That ($report.completed -and $report.mission.won) "Player $slot did not complete extraction"
            Assert-That ($report.mission.activePlayers -eq $(if ($Kind -eq 'local') {1} else {4})) 'Every player must take part in extraction'
            Assert-That ($report.mission.salvaged -ge $report.mission.goal) 'The real salvage quota must be met'
            Assert-That (@($report.navigationErrors).Count -eq 0) "Player $slot has navigation errors"
            Assert-That ($report.doors[0].open) "Shared door missing on player $slot"
            Assert-That ($report.doors[1].open) "Second shared door missing on player $slot"
            if ($slot -ne 0) {
                Assert-That $report.returnedToMenu "Player $slot did not activate the win screen's return button"
                if ($Kind -ne 'local') { Assert-That ($report.route -contains 'action:ready') "Player $slot did not ready through native UI" }
                Assert-That ($report.route -contains $(if ($Kind -eq 'local') {'action:solo'} elseif ($slot -eq $authority) {'action:host'} else {'action:join'})) "Player $slot bypassed the native menu route"
            }
            if ($slot -eq $authority) {
                Assert-That ($report.replayVerified -and $report.determinismPasses -eq 2 -and $report.replayCheckpointEvery -eq 30) 'Authority must hash every offline tick and verify two replays against live checkpoints'
                if ($Kind -ne 'local') { Assert-That ($report.mission.revives -ge 1) 'The crew must revive the scout after real drone damage' }
            }
            else {
                $privateTotal = 0
                foreach ($entry in $report.survivors.PSObject.Properties) {
                    $bag = $entry.Value.circuits + $entry.Value.cells + $entry.Value.radios
                    if ($entry.Value.player -eq $slot) { $privateTotal += $bag }
                    else { Assert-That ($bag -eq 0) "Player $slot received another player's private inventory" }
                }
                Assert-That ($privateTotal -gt 0) "Player $slot must collect real salvage"
                Assert-That ($report.net.predicted -gt 0) "Player $slot must use owner prediction"
            }
            foreach ($state in @('hud','inventory','door','win')) {
                if ($slot -ne 0) { Assert-That (Test-Path -LiteralPath (Join-Path $evidence "player-$slot-$state.png")) "Missing $state capture for player $slot" }
            }
        }
        if ($Kind -ne 'local') { Assert-That (@(Get-ChildItem -LiteralPath $evidence -Filter 'player-*-revive.png').Count -gt 0) 'Missing the real revive interaction capture' }
        $summary = [ordered]@{mode=$Kind;passed=$true;lateJoinAfterDoorTick=$joinTick;players=$players;latencyMs=$(if($Kind -eq 'local'){0}else{40});jitterMs=$(if($Kind -eq 'local'){0}else{10});lossBasisPoints=$(if($Kind -eq 'local'){0}else{400});evidence=$evidence}
        $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $evidence 'gate.json') -Encoding utf8
        $summary | ConvertTo-Json -Compress
    } finally {
        foreach ($child in $children) {
            # These are the process objects returned by this test's own launches.
            if (!$child.HasExited) { $child.Kill(); $child.WaitForExit() }
            $child.Dispose()
        }
    }
}
if ($Mode -in @('all','local')) { Run-Crew 'local' }
if ($Mode -in @('all','listen')) { Run-Crew 'listen' }
if ($Mode -in @('all','dedicated')) { Run-Crew 'dedicated' }

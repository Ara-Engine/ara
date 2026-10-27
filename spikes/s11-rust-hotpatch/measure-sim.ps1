# Manual opt-in: edit and restore a real Session system while the sim probe runs.
param([Parameter(Mandatory=$true)][string]$Output, [Parameter(Mandatory=$true)][string]$Root)
$ErrorActionPreference = 'Stop'
[IO.Directory]::CreateDirectory($Output) | Out-Null
$source = Join-Path $Root 'crates/ara-hotpatch/sim-probe/main.rs'
$original = [IO.File]::ReadAllText($source)
$old = 'transform.position[0] += 1.;'
$new = 'transform.position[0] += 2.;'
if (($original.Split(@($old), [StringSplitOptions]::None)).Length -ne 2) { throw 'sim anchor is not unique' }
$encoding = [Text.UTF8Encoding]::new($false)
function Read-State {
    try { Get-Content -LiteralPath (Join-Path $Output 'status.json') -Raw | ConvertFrom-Json } catch { $null }
}
function Wait-State([scriptblock]$Predicate, [int]$Seconds = 60) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $Seconds) {
        $state = Read-State
        if ($null -ne $state -and (& $Predicate $state)) { return $state }
        Start-Sleep -Milliseconds 20
    }
    throw "status timeout: $((Read-State) | ConvertTo-Json -Compress -Depth 8)"
}
$results = [Collections.Generic.List[object]]::new()
try {
    $initial = Wait-State { param($s) $s.frame -ge 3 } -Seconds 900
    # The transport connects asynchronously. Let dx install the native ASLR
    # reference before timing saves; a fast headless loop starts before that.
    Start-Sleep -Milliseconds 1000
    $hostPid = $initial.pid
    $entity = $initial.entity
    $position = $initial.position
    $generation = [uint64]$initial.rust.generation
    foreach ($phase in @('body', 'recovery', 'body-repeat', 'restore')) {
        $delta = if ($phase -in @('body', 'body-repeat')) { 2 } else { 1 }
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $text = if ($delta -eq 2) { $original.Replace($old, $new) } else { $original }
        [IO.File]::WriteAllText($source, $text, $encoding)
        $previous = $generation
        $state = Wait-State { param($s) $s.rust.generation -gt $previous -and $s.delta -eq $delta }
        $observed = $timer.Elapsed.TotalMilliseconds
        if ($state.pid -ne $hostPid -or $state.entity -ne $entity -or $state.position -le $position) { throw 'ECS host/entity/state did not survive patching' }
        if (-not $state.rust_hotpatched -or -not $state.guards_verified) { throw 'replay/network taint guards failed' }
        if ($null -ne $state.rust.last_error) { throw 'body patch rejected' }
        $generation = [uint64]$state.rust.generation
        $position = $state.position
        $row = [ordered]@{ phase=$phase; observed_ms=$observed; pid=$hostPid; entity=$entity; frame=$state.frame; position=$position; delta=$state.delta; generation=$generation; guards_verified=$state.guards_verified }
        $results.Add($row)
        $row | ConvertTo-Json -Compress -Depth 8
        $results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Output 'measurements.json')
    }
} finally {
    [IO.File]::WriteAllText($source, $original, $encoding)
    [IO.File]::WriteAllText((Join-Path $Output 'stop'), 'stop', $encoding)
}

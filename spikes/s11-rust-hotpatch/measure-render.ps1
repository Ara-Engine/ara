# Manual opt-in measurement. Edits two Rust sources and restores them in finally.
# Start ara-dev hotpatch render-probe separately with this same scratch Output.
param([Parameter(Mandatory=$true)][string]$Output, [Parameter(Mandatory=$true)][string]$Root, [switch]$Fidelity)
$ErrorActionPreference = 'Stop'
[IO.Directory]::CreateDirectory($Output) | Out-Null
$passPath = Join-Path $Root 'crates/ara-render/src/renderer/passes.rs'
$layoutPath = Join-Path $Root 'crates/ara-render/src/renderer/frame.rs'
$passBefore = [IO.File]::ReadAllText($passPath)
$layoutBefore = [IO.File]::ReadAllText($layoutPath)
$encoding = [Text.UTF8Encoding]::new($false)
$old = 'words[5] = u32::from(lit);'
$new = 'words[5] = u32::from(!lit);'
if ($Fidelity) {
    $passPath = Join-Path $Root 'crates/ara-render-fidelity/src/render.rs'
    $layoutPath = Join-Path $Root 'crates/ara-render-fidelity/src/frame.rs'
    $passBefore = [IO.File]::ReadAllText($passPath)
    $layoutBefore = [IO.File]::ReadAllText($layoutPath)
    $old = 'let (frame, vp, camera) = self.frame(input)?;'
    $new = 'let original = self.settings.post.compensation;' + "`n" +
        '        self.settings.post.compensation = original + 3.;' + "`n" +
        '        let recorded = self.frame(input);' + "`n" +
        '        self.settings.post.compensation = original;' + "`n" +
        '        let (frame, vp, camera) = recorded?;'
}
if (($passBefore.Split(@($old), [StringSplitOptions]::None)).Length -ne 2) { throw 'pass probe anchor is not unique' }
function Read-State {
    try { Get-Content -LiteralPath (Join-Path $Output 'status.json') -Raw | ConvertFrom-Json } catch { $null }
}
function Wait-State([scriptblock]$Predicate, [int]$Seconds = 60) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $Seconds) {
        $state = Read-State
        if ($null -ne $state -and (& $Predicate $state)) { return $state }
        Start-Sleep -Milliseconds 40
    }
    throw "status timeout: $((Read-State) | ConvertTo-Json -Compress -Depth 8)"
}
$results = [Collections.Generic.List[object]]::new()
try {
    $initial = Wait-State -Predicate { param($s) $s.frame -ge 3 } -Seconds 900
    $hostPid = $initial.pid
    $generation = [uint64]$initial.rust.generation
    $baselineHash = (Get-FileHash -LiteralPath (Join-Path $Output "generation-$generation.png") -Algorithm SHA256).Hash
    foreach ($phase in @('body', 'layout', 'recovery', 'body-repeat', 'restore')) {
        if ($phase -eq 'layout') { Start-Sleep -Seconds 8; $generation = [uint64](Read-State).rust.generation }
        $timer = [Diagnostics.Stopwatch]::StartNew()
        if ($phase -in @('body', 'body-repeat')) {
            [IO.File]::WriteAllText($passPath, $passBefore.Replace($old, $new), $encoding)
        } elseif ($phase -eq 'layout') {
            $layoutOld = if ($Fidelity) { 'inverse_vp: Mat4,' } else { 'pub vp: Mat4,' }
            $layoutNew = if ($Fidelity) { 'inverse_vp: [Mat4; 2],' } else { 'pub vp: [Mat4; 2],' }
            if (-not $layoutBefore.Contains($layoutOld)) { throw 'layout probe anchor is missing' }
            [IO.File]::WriteAllText($layoutPath, $layoutBefore.Replace($layoutOld, $layoutNew), $encoding)
        } else {
            [IO.File]::WriteAllText($layoutPath, $layoutBefore, $encoding)
            [IO.File]::WriteAllText($passPath, $passBefore, $encoding)
        }
        if ($phase -eq 'layout') {
            $state = Wait-State { param($s) $s.rust.last_error -match 'needs rebuild' }
            $observed = $timer.Elapsed.TotalMilliseconds
            if ($state.pid -ne $hostPid -or $state.rust.generation -ne $generation) { throw 'layout rejection replaced the running host or generation' }
            $frame = $state.frame
            $state = Wait-State { param($s) $s.frame -gt ($frame + 3) }
            $row = [ordered]@{ phase=$phase; observed_ms=$observed; pid=$state.pid; frame=$state.frame; generation=$state.rust.generation; rejected=$true; reason=$state.rust.last_error }
        } else {
            $previous = $generation
            $wantChanged = $phase -in @('body', 'body-repeat')
            $state = Wait-State {
                param($s)
                if ($s.rust.generation -le $previous) { return $false }
                $image = Join-Path $Output "generation-$($s.rust.generation).png"
                if (-not (Test-Path -LiteralPath $image)) { return $false }
                try { $different = (Get-FileHash -LiteralPath $image -Algorithm SHA256).Hash -ne $baselineHash } catch { return $false }
                return $different -eq $wantChanged
            }
            $observed = $timer.Elapsed.TotalMilliseconds
            if ($state.pid -ne $hostPid) { throw 'body patch restarted the renderer' }
            $generation = [uint64]$state.rust.generation
            $capture = Join-Path $Output "generation-$generation.png"
            $hash = (Get-FileHash -LiteralPath $capture -Algorithm SHA256).Hash
            $changed = $hash -ne $baselineHash
            if (($phase -in @('body', 'body-repeat')) -and -not $changed) { throw 'table applied but actual render pass output did not change' }
            if ($null -ne $state.rust.last_error) { throw 'successful patch retained a rejection' }
            $row = [ordered]@{phase=$phase; observed_ms=$observed; pid=$state.pid; frame=$state.frame; generation=$generation; image=$capture; hash=$hash; changed_from_baseline=$changed }
        }
        $results.Add($row)
        $row | ConvertTo-Json -Compress -Depth 8
        $results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Output 'measurements.json')
    }
} finally {
    [IO.File]::WriteAllText($passPath, $passBefore, $encoding)
    [IO.File]::WriteAllText($layoutPath, $layoutBefore, $encoding)
    [IO.File]::WriteAllText((Join-Path $Output 'stop'), 'stop', $encoding)
}

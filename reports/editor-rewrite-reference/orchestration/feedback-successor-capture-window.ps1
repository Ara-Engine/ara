$ErrorActionPreference='Stop'
. "$PSScriptRoot/monkey-gate-guard.ps1"
$guard=New-GateGuard
$deadline=[DateTime]::UtcNow.AddMinutes(40)
$statePath='D:/AraBuild/ux-editor-target/orchestration/feedback-successor-capture-window.json'
$lastPhase=''
do {
    $gate=Read-GateGuard $guard D:/AraBuild
    $phase="$($gate.path)|$($gate.phase)|$($gate.reason)"
    if($phase -ne $lastPhase){
        [ordered]@{waiting=$true;observedUtc=[DateTime]::UtcNow.ToString('o');gate=$gate;checkpoint='96e73f91';attempts=0}|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $statePath -Encoding UTF8
        $lastPhase=$phase
    }
    if($gate.allowed -and $gate.sequence -ge 69){
        [ordered]@{waiting=$false;launchedUtc=[DateTime]::UtcNow.ToString('o');gate=$gate;checkpoint='96e73f91';attempts=1;automaticRetry=$false}|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $statePath -Encoding UTF8
        & powershell -NoLogo -NoProfile -File "$PSScriptRoot/run-native-gate-guarded.ps1" -CheckpointSHA 96e73f91 -Mode behaviours -EvidenceMode correctness-queued *> D:/AraBuild/ux-editor-target/orchestration/feedback-successor-after-native.log
        exit $LASTEXITCODE
    }
    Start-Sleep -Milliseconds 750
}while([DateTime]::UtcNow -lt $deadline)
throw 'The bounded successor capture-window wait expired without starting a capture'

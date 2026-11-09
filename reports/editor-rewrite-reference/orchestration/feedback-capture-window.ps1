$ErrorActionPreference='Stop'
. "$PSScriptRoot/monkey-gate-guard.ps1"
$guard=New-GateGuard
$deadline=[DateTime]::UtcNow.AddMinutes(30)
$statePath='D:/AraBuild/ux-editor-target/orchestration/feedback-capture-window.json'
$lastPhase=''
do {
    $gate=Read-GateGuard $guard D:/AraBuild
    $phase="$($gate.path)|$($gate.phase)|$($gate.reason)"
    if($phase -ne $lastPhase){
        [ordered]@{waiting=$true;observedUtc=[DateTime]::UtcNow.ToString('o');gate=$gate;checkpoint='4e1cfd97';attempts=0}|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $statePath -Encoding UTF8
        $lastPhase=$phase
    }
    if($gate.allowed -and $gate.sequence -ge 69 -and $gate.phase -eq 'compiling'){
        [ordered]@{waiting=$false;launchedUtc=[DateTime]::UtcNow.ToString('o');gate=$gate;checkpoint='4e1cfd97';attempts=1;automaticRetry=$false}|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $statePath -Encoding UTF8
        & powershell -NoLogo -NoProfile -File "$PSScriptRoot/run-native-gate-guarded.ps1" -CheckpointSHA 4e1cfd97 -Mode behaviours -EvidenceMode correctness-queued *> D:/AraBuild/ux-editor-target/orchestration/feedback-after-native.log
        exit $LASTEXITCODE
    }
    Start-Sleep -Milliseconds 750
}while([DateTime]::UtcNow -lt $deadline)
throw 'The bounded b69 capture-window wait expired without starting a capture'

param(
    [string]$CheckpointSHA,
    [ValidateSet('behaviours','behaviours-core','docking')][string]$Mode='behaviours',
    [string]$Project,
    [UInt64]$Seed=104729,
    [ValidateSet('correctness-queued')][string]$EvidenceMode='correctness-queued',
    [string]$GateDirectory='D:/AraBuild',
    [string]$EvidenceRoot='D:/AraBuild/ux-editor-target/acceptance',
    [ValidateRange(1,1200)][int]$TimeoutSeconds=1200
)
# Shared guard owns gate parsing and exact PID/birth/executable/ancestry checks.
. (Join-Path $PSScriptRoot 'monkey-gate-guard.ps1')
function Get-NativeGateDisposition($Observation,[ValidateSet('before-launch','running','completion')][string]$Phase) {
    if($null -ne $Observation -and $Observation.allowed -eq $true){return 'continue'}
    if($Phase -eq 'before-launch'){return 'block'}
    return 'discard'
}
function Quote-NativeGuardArgument([string]$Value) {
    if($Value -notmatch '[\s"]'){return $Value}
    return '"'+($Value -replace '(\\*)"','$1$1\"' -replace '(\\+)$','$1$1')+'"'
}
function Invoke-NativeGateGuarded {
    param(
        [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-fA-F]{8}$')][string]$CheckpointSHA,
        [ValidateSet('behaviours','behaviours-core','docking')][string]$Mode='behaviours',
        [string]$Project,[UInt64]$Seed=104729,
        [ValidateSet('correctness-queued')][string]$EvidenceMode='correctness-queued',
        [string]$GateDirectory='D:/AraBuild',
        [string]$EvidenceRoot='D:/AraBuild/ux-editor-target/acceptance',
        [ValidateRange(1,1200)][int]$TimeoutSeconds=1200
    )
    $ErrorActionPreference='Stop'
    $root=[IO.Path]::GetFullPath($EvidenceRoot)
    $allowedRoot=[IO.Path]::GetFullPath('D:/AraBuild/ux-editor-target/acceptance')
    if($root -ne $allowedRoot -and !$root.StartsWith($allowedRoot+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Guard evidence must stay below the D: acceptance root'}
    $directory=Join-Path $root ('native-guard-'+$CheckpointSHA+'-'+$Mode+'-'+[Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($directory)|Out-Null
    $record=[pscustomobject][ordered]@{checkpoint=$CheckpointSHA;mode=$Mode;seed=$Seed;directory=$directory;startedUtc=[DateTime]::UtcNow.ToString('o');finishedUtc=$null;requestedEvidenceMode='correctness-queued';frameTimeStatus='informal (shared GPU)';timingAccepted=$false;accepted=$false;acceptanceEligible=$true;guardPassed=$false;discarded=$false;blockedBeforeStart=$false;automaticRetry=$false;timeoutSeconds=$TimeoutSeconds;pollMilliseconds=750;process=$null;processKnown=@();observations=@();cleanup=$null;error=$null;wrapperExitCode=1;note='Guard eligibility does not establish journey acceptance. Inspect the production run reports. No timings are formal.'}
    $stateFile=Join-Path $directory 'guard.json'
    $guard=New-GateGuard;$known=@();$process=$null
    try {
        $observation=Read-GateGuard $guard $GateDirectory;$record.observations+=,$observation
        if((Get-NativeGateDisposition $observation 'before-launch') -eq 'block'){
            $record.blockedBeforeStart=$true;$record.acceptanceEligible=$false;$record.wrapperExitCode=3
            $record.error='Blocked before launch: '+$observation.reason
            return [pscustomobject]$record
        }
        $shellExe=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        $arguments=@('-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'run-native.ps1'),'-CheckpointSHA',$CheckpointSHA,'-Mode',$Mode,'-Seed',[string]$Seed,'-EvidenceMode','correctness-queued')
        if($Project){$arguments+=@('-Project',$Project)}
        $argumentLine=($arguments|ForEach-Object{Quote-NativeGuardArgument $_}) -join ' '
        # Recheck immediately before spawning; no gate-clear result grants a slot.
        $observation=Read-GateGuard $guard $GateDirectory;$record.observations+=,$observation
        if((Get-NativeGateDisposition $observation 'before-launch') -eq 'block'){
            $record.blockedBeforeStart=$true;$record.acceptanceEligible=$false;$record.wrapperExitCode=3
            $record.error='Blocked at launch boundary: '+$observation.reason
            return [pscustomobject]$record
        }
        $process=Start-Process -FilePath $shellExe -ArgumentList $argumentLine -WorkingDirectory 'D:/AraBuild/ux-editor-source' -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $directory 'launcher.stdout.log') -RedirectStandardError (Join-Path $directory 'launcher.stderr.log')
        $null=$process.Handle
        $record.process=[ordered]@{pid=$process.Id;startedUtc=$process.StartTime.ToUniversalTime().ToString('o');executable=$shellExe;arguments=$arguments;completed=$false;exitCode=$null}
        $known=@([pscustomobject]@{pid=$process.Id;startedUtc=$record.process.startedUtc;executable=$shellExe;depth=0;parentPid=$null;parentStartedUtc=$null})
        $deadline=[DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        $completed=$false
        do {
            $plan=Plan-GateOwnedTree $record.process $known @(Get-GateProcessSnapshot);$known=@($plan.known)
            # A just-created process may not expose its executable in its first
            # snapshot. Retry observation while the gate permits running; final
            # certification and any stop still require exact owned identities.
            if(@($plan.issues).Count -gt 0){
                $record|Add-Member -NotePropertyName ownershipObservationIssues -NotePropertyValue @($plan.issues) -Force
            }
            $observation=Read-GateGuard $guard $GateDirectory
            $previous=$record.observations|Select-Object -Last 1
            if($previous.path -ne $observation.path -or $previous.phase -ne $observation.phase -or $previous.reason -ne $observation.reason){$record.observations+=,$observation}
            if((Get-NativeGateDisposition $observation 'running') -eq 'discard'){
                $record.discarded=$true;$record.acceptanceEligible=$false;$record.wrapperExitCode=3
                $record.error='Gate preemption: '+$observation.reason
                break
            }
            $record|ConvertTo-Json -Depth 14|Set-Content -LiteralPath $stateFile -Encoding UTF8
            $completed=$process.WaitForExit(750)
        }while(!$completed -and [DateTime]::UtcNow -lt $deadline)
        if(!$completed -and !$record.discarded){throw 'Owned native wrapper exceeded its bounded deadline'}
        if($completed -and !$record.discarded){
            $observation=Read-GateGuard $guard $GateDirectory;$record.observations+=,$observation
            if((Get-NativeGateDisposition $observation 'completion') -eq 'discard'){
                $record.discarded=$true;$record.acceptanceEligible=$false;$record.wrapperExitCode=3
                $record.error='Gate preemption at completion boundary: '+$observation.reason
            }else{
                $process.WaitForExit();$process.Refresh()
                $record.process.completed=$true;$record.process.exitCode=$process.ExitCode
                if($null -eq $process.ExitCode){throw 'Owned wrapper exit code is unavailable'}
                $plan=Plan-GateOwnedTree $record.process $known @(Get-GateProcessSnapshot);$known=@($plan.known)
                if(@($plan.issues).Count -gt 0 -or @($plan.processes).Count -gt 0){throw 'Owned descendants remain after the wrapper exit; evidence cannot be certified'}
                $record.wrapperExitCode=$process.ExitCode;$record.guardPassed=$process.ExitCode -eq 0
                $record.acceptanceEligible=$record.guardPassed
            }
        }
    }catch{
        $record.error=$_.Exception.Message;$record.discarded=$null -ne $process;$record.acceptanceEligible=$false;$record.wrapperExitCode=1
    }finally{
        if($null -ne $process -and $record.discarded){$record.cleanup=Stop-GateOwnedTree $record.process $known;$record.frameTimeStatus='discarded (gate preemption or incomplete run)'}
        $record.processKnown=$known;$record.finishedUtc=[DateTime]::UtcNow.ToString('o')
        $record|ConvertTo-Json -Depth 16|Set-Content -LiteralPath $stateFile -Encoding UTF8
        if($null -ne $process){$process.Dispose()}
    }
    return [pscustomobject]$record
}
if($MyInvocation.InvocationName -ne '.'){
    $result=Invoke-NativeGateGuarded @PSBoundParameters
    $result|ConvertTo-Json -Depth 16
    exit $result.wrapperExitCode
}

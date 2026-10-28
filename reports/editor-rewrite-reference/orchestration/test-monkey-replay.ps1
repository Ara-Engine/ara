# Synthetic assessment checks only: no driver top-level, editor, GPU or build runs.
$ErrorActionPreference='Stop'
Set-StrictMode -Version 2.0
$driver='D:/AraBuild/ux-editor-target/orchestration/monkey-replay.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($driver,[ref]$tokens,[ref]$errors)
if($errors.Count -gt 0){throw ($errors|Out-String)}
# Import only the named pure assessment functions, never the launch/fixture body.
foreach($name in @('Property','True','False','Has-Property','Recorder-Required','Recorder-Contract','Require-Array','Is-Counter','Add-JournalAction','Same-JournalStep','Read-Journal','Assess-Output','Assess-Recorder','Assess')) {
    $function=$ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)|Where-Object{$_.Name -eq $name}|Select-Object -First 1
    if($null -eq $function){throw "Missing assessment helper: $name"}
    Invoke-Expression $function.Extent.Text
}
$baseline=@{
    seed=104729;hostSeed=104729;hostSeedProvenance='startup';replayOriginalHostSeed=104729;replayOriginalHostSeedProvenance='journal';hostSeedMatchesReplay=$true;
    finished=$true;accepted=$true;replayedJournal=$true;finalDiagnosticsCollected=$true;soakRequested=$false;soakAccepted=$false;cleanupIncomplete=$false;diagnosticBusLoss=$false;
    termination='replay-exhausted';replayRemaining=0;panics=@();hangs=0;panelFailures=0;frameSpikes=0;diagnosticRecordsOmitted=0;unvisitedCommands=@();missingCommandEdgeCases=@();
    offscreenGpuOutput=$true;gpuSurfaceSubmissions=100;nativePresentations=0;gpuOutputStalls=0;gpuOutputHealth=@{observed=$true;healthy=$true;stalls=0;unknownSamples=0};
    endDrain=@{cleanupIncomplete=$false;heldKeys=@();heldButtons=@()};playResources=@{ownedActive=@();ownedStopping=@()};
    hostJobResources=@{baselineCollected=$true;finalInventoryCollected=$true;drained=$true;hostUnsaved=$false;fileFlushPending=$false;errors=@();shell=@{busy=$false}};
    typedCommandJobs=@{drained=$true};reportWriter=@{finalSnapshotRequested=$true;error=$null};
    journalRecorder=@{finalDrained=$true;error=$null;errorCount=0;recordsNotQueued=0;pendingAction=$null;abortedAction=$null;recordsAttempted=10;recordsFlushed=10;bytesAttempted=1000;bytesFlushed=1000}
}|ConvertTo-Json -Depth 20
$runTemplate=@{completed=$true;exitCode=0;requestedEvidenceMode='correctness-shared';frameTimeStatus='informal (shared GPU)';timingAccepted=$false;gpuWrapperUsed=$false}|ConvertTo-Json
$originalTemplate=@{seed=104729;hostSeed=104729;hostSeedProvenance='journal';actions=10;actionSha256='synthetic-action-hash'}|ConvertTo-Json
$results=New-Object 'Collections.Generic.List[object]'
function Check([string]$Name,[scriptblock]$Change,[bool]$Reject,[string]$Reason='',[bool]$RequireRecorder=$true) {
    $report=$baseline|ConvertFrom-Json;$original=$originalTemplate|ConvertFrom-Json;$replay=$originalTemplate|ConvertFrom-Json;$run=$runTemplate|ConvertFrom-Json
    & $Change $report $original $replay $run
    $failures=@(Assess $report $run $original $replay 104729 $RequireRecorder)
    if(($failures.Count -gt 0) -ne $Reject){throw "${Name}: unexpected assessment $($failures|ConvertTo-Json -Compress)"}
    if($Reason -and @($failures|Where-Object{$_ -like "*$Reason*"}).Count -eq 0){throw "${Name}: intended evidence failure was not detected: $Reason"}
    $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;contract=(Recorder-Contract $report $RequireRecorder);reasons=$failures})
}
Check 'valid bounded offscreen GPU replay' {param($r)} $false
Check 'informal frame spikes do not impose an FPS gate' {param($r) $r.frameSpikes=100000} $false
Check 'partial journal does not become catalog or soak acceptance' {param($r) $r.unvisitedCommands=@('unvisited');$r.missingCommandEdgeCases=@('missing')} $false
Check 'missing offscreen classification stays unknown' {param($r) $r.PSObject.Properties.Remove('offscreenGpuOutput')} $true 'offscreen GPU output'
Check 'visible output classification rejected' {param($r) $r.offscreenGpuOutput=$false} $true 'offscreen GPU output'
Check 'string true is not offscreen proof' {param($r) $r.offscreenGpuOutput='true'} $true 'offscreen GPU output'
Check 'missing submission count rejected' {param($r) $r.PSObject.Properties.Remove('gpuSurfaceSubmissions')} $true 'GPU surface submissions'
Check 'zero submissions rejected' {param($r) $r.gpuSurfaceSubmissions=0} $true 'GPU surface submissions'
Check 'negative submissions rejected' {param($r) $r.gpuSurfaceSubmissions=-1} $true 'GPU surface submissions'
Check 'string submission count rejected' {param($r) $r.gpuSurfaceSubmissions='100'} $true 'GPU surface submissions'
Check 'missing native presentation count rejected' {param($r) $r.PSObject.Properties.Remove('nativePresentations')} $true 'zero native presentations'
Check 'native presentation rejected' {param($r) $r.nativePresentations=1} $true 'zero native presentations'
Check 'null native presentation count stays unknown' {param($r) $r.nativePresentations=$null} $true 'zero native presentations'
Check 'string zero cannot prove no native presentations' {param($r) $r.nativePresentations='0'} $true 'zero native presentations'
Check 'missing output-stall count rejected' {param($r) $r.PSObject.Properties.Remove('gpuOutputStalls')} $true 'zero GPU-output stalls'
Check 'output stall rejected even with accepted true' {param($r) $r.gpuOutputStalls=1} $true 'zero GPU-output stalls'
Check 'string zero cannot prove no output stalls' {param($r) $r.gpuOutputStalls='0'} $true 'zero GPU-output stalls'
Check 'missing health object stays unknown' {param($r) $r.PSObject.Properties.Remove('gpuOutputHealth')} $true 'health does not confirm observed'
Check 'null health object stays unknown' {param($r) $r.gpuOutputHealth=$null} $true 'health does not confirm observed'
Check 'unobserved health rejected' {param($r) $r.gpuOutputHealth.observed=$false} $true 'health does not confirm observed'
Check 'missing observed flag rejected' {param($r) $r.gpuOutputHealth.PSObject.Properties.Remove('observed')} $true 'health does not confirm observed'
Check 'unhealthy output rejected' {param($r) $r.gpuOutputHealth.healthy=$false} $true 'health does not confirm healthy'
Check 'unknown health is not healthy' {param($r) $r.gpuOutputHealth.healthy=$null} $true 'health does not confirm healthy'
Check 'missing healthy flag rejected' {param($r) $r.gpuOutputHealth.PSObject.Properties.Remove('healthy')} $true 'health does not confirm healthy'
Check 'nested output stall cannot hide behind top-level zero' {param($r) $r.gpuOutputHealth.stalls=1} $true 'zero stalls'
Check 'missing nested stall count rejected' {param($r) $r.gpuOutputHealth.PSObject.Properties.Remove('stalls')} $true 'zero stalls'
Check 'null nested stall count stays unknown' {param($r) $r.gpuOutputHealth.stalls=$null} $true 'zero stalls'
Check 'string nested stall count rejected' {param($r) $r.gpuOutputHealth.stalls='0'} $true 'zero stalls'
Check 'lost native observations rejected' {param($r) $r.gpuOutputHealth.unknownSamples=1} $true 'zero unknownSamples'
Check 'missing unknown-sample count stays unknown' {param($r) $r.gpuOutputHealth.PSObject.Properties.Remove('unknownSamples')} $true 'zero unknownSamples'
Check 'null unknown-sample count stays unknown' {param($r) $r.gpuOutputHealth.unknownSamples=$null} $true 'zero unknownSamples'
Check 'legacy Host provenance is not invented' {param($r,$o) $o.hostSeed=$null;$o.hostSeedProvenance='unknown';$r.replayOriginalHostSeed=$null;$r.replayOriginalHostSeedProvenance='unknown';$r.hostSeedMatchesReplay=$null} $true 'Historical Host seed is unknown'
Check 'exact recorded actions remain required' {param($r,$o,$p) $p.actionSha256='different'} $true 'action objects/count differ'
Check 'owned resources must still drain' {param($r) $r.hostJobResources.drained=$false} $true 'Host job inventory does not confirm drained'
Check 'bounded replay cannot claim soak acceptance' {param($r) $r.soakAccepted=$true} $true 'does not rule out soakAccepted'
Check 'historical healthy report retains an explicit legacy assessment' {param($r) $r.PSObject.Properties.Remove('journalRecorder')} $false '' $false
Check 'new checkpoint rejects a missing recorder field' {param($r) $r.PSObject.Properties.Remove('journalRecorder')} $true 'requires journalRecorder'
Check 'provided new recorder contract is checked even on a legacy target' {param($r) $r.journalRecorder.finalDrained=$false} $true 'finalDrained' $false
Check 'null recorder is not a historical healthy report' {param($r) $r.journalRecorder=$null} $true 'journalRecorder is null' $false
Check 'unfinished recorder rejected' {param($r) $r.journalRecorder.finalDrained=$false} $true 'finalDrained'
Check 'string recorder drained flag rejected' {param($r) $r.journalRecorder.finalDrained='true'} $true 'finalDrained'
Check 'recorder error rejected' {param($r) $r.journalRecorder.error='disk full'} $true 'explicit null error'
Check 'missing error field is not explicit null' {param($r) $r.journalRecorder.PSObject.Properties.Remove('error')} $true 'explicit null error'
Check 'recorder error count rejected' {param($r) $r.journalRecorder.errorCount=1} $true 'zero errorCount'
Check 'missing error count rejected' {param($r) $r.journalRecorder.PSObject.Properties.Remove('errorCount')} $true 'zero errorCount'
Check 'string zero error count rejected' {param($r) $r.journalRecorder.errorCount='0'} $true 'zero errorCount'
Check 'recorder queue rejection rejected' {param($r) $r.journalRecorder.recordsNotQueued=1} $true 'zero recordsNotQueued'
Check 'missing queue rejection count rejected' {param($r) $r.journalRecorder.PSObject.Properties.Remove('recordsNotQueued')} $true 'zero recordsNotQueued'
Check 'pending recorder action rejected' {param($r) $r.journalRecorder.pendingAction=@{ticket=1;step=1}} $true 'explicit null pendingAction'
Check 'missing pending field rejected' {param($r) $r.journalRecorder.PSObject.Properties.Remove('pendingAction')} $true 'explicit null pendingAction'
Check 'aborted recorder action rejected' {param($r) $r.journalRecorder.abortedAction=@{ticket=1;step=1;wasDispatched=$false}} $true 'explicit null abortedAction'
Check 'missing aborted field rejected' {param($r) $r.journalRecorder.PSObject.Properties.Remove('abortedAction')} $true 'explicit null abortedAction'
Check 'unflushed record rejected' {param($r) $r.journalRecorder.recordsFlushed=9} $true 'recordsAttempted and recordsFlushed'
Check 'unflushed byte rejected' {param($r) $r.journalRecorder.bytesFlushed=999} $true 'bytesAttempted and bytesFlushed'
Check 'missing both record counters rejected' {param($r) $r.journalRecorder.PSObject.Properties.Remove('recordsAttempted');$r.journalRecorder.PSObject.Properties.Remove('recordsFlushed')} $true 'recordsAttempted and recordsFlushed'
Check 'null byte counters rejected' {param($r) $r.journalRecorder.bytesAttempted=$null;$r.journalRecorder.bytesFlushed=$null} $true 'bytesAttempted and bytesFlushed'
Check 'string matching counters are not recorder proof' {param($r) $r.journalRecorder.recordsAttempted='10';$r.journalRecorder.recordsFlushed='10'} $true 'recordsAttempted and recordsFlushed'
Check 'negative matching counters rejected' {param($r) $r.journalRecorder.bytesAttempted=-1;$r.journalRecorder.bytesFlushed=-1} $true 'bytesAttempted and bytesFlushed'

if(!(Recorder-Required 'json!({"journalRecorder":journal})')){throw 'Frozen-source recorder declaration must require the new report contract'}
if(Recorder-Required 'legacy report source'){throw 'Legacy target must not be silently assigned the new contract'}
if(!(Recorder-Required 'legacy report source' $true)){throw 'Explicit required contract must fail closed even without a source marker'}
if((Recorder-Contract ([pscustomobject]@{}) $false) -ne 'historical-report-without-recorder/legacy'){throw 'Historical report assessment lost its distinct label'}

$utf8=New-Object Text.UTF8Encoding($false)
$scratch=Join-Path 'D:/AraBuild/ux-editor-target/monkey-replay-assessment-tests' ([Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($scratch)|Out-Null
function Begin([object]$Step,[object]$Ticket,[bool]$Reverse) {
    $record=[ordered]@{phase='begin';seed=7;hostSeed=7;step=$Step;action=[ordered]@{kind='tab';reverse=$Reverse;verbatim='Unicode مرحبا / quoted "text"'}}
    if($null -ne $Ticket){$record.ticket=$Ticket}
    return $record
}
function End([object]$Step,[string]$Outcome='accepted'){return [ordered]@{phase='end';step=$Step;outcome=$Outcome}}
function Abort([object]$Step,[object]$Ticket,[object]$Dispatched=$false){return [ordered]@{phase='abort';step=$Step;ticket=$Ticket;wasDispatched=$Dispatched}}
function Action-Hash([object[]]$Actions) {
    $lines=@($Actions|ForEach-Object{ConvertTo-Json -InputObject $_ -Depth 100 -Compress})
    $digest=[Security.Cryptography.SHA256]::Create()
    try {return [BitConverter]::ToString($digest.ComputeHash($utf8.GetBytes(($lines -join "`n")+"`n"))).Replace('-','')}
    finally {$digest.Dispose()}
}
function Check-Journal([string]$Name,[object[]]$Records,[object[]]$Expected,[bool]$Reject=$false,[string]$Reason='abort must match') {
    $path=Join-Path $scratch ('journal-'+$results.Count+'.jsonl')
    [IO.File]::WriteAllText($path,(@($Records|ForEach-Object{ConvertTo-Json -InputObject $_ -Depth 100 -Compress}) -join "`n")+"`n",$utf8)
    $before=(Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
    $metadata=$null;$failure=$null
    try {$metadata=Read-Journal $path}catch{$failure=$_.Exception.Message}
    if(($null -ne $failure) -ne $Reject){throw "${Name}: unexpected journal result: $failure"}
    if($Reject -and $Reason -and $failure -notlike "*$Reason*"){throw "${Name}: wrong journal rejection: $failure"}
    if(!$Reject -and ($metadata.actions -ne $Expected.Count -or $metadata.actionSha256 -ne (Action-Hash $Expected))){throw "${Name}: exact action objects/count/order changed"}
    if((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -ne $before){throw "${Name}: source journal was rewritten"}
    $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;reason=$failure;metadata=$metadata;journal=$path;sha256=$before})
    return $metadata
}
$first=Begin 0 1 $false;$second=Begin 1 2 $true;$retry=Begin 1 3 $true
$null=Check-Journal 'healthy ticketed begin/end actions remain verbatim' @($first,(End 0),$second,(End 1)) @($first.action,$second.action)
$null=Check-Journal 'ended rejected action still replays verbatim' @($first,(End 0 'rejected: real failure'),$second,(End 1)) @($first.action,$second.action)
$matched=Check-Journal 'exact uncompleted abort removes only its ticket and same step can retry' @($first,(End 0 'rejected'),$second,(Abort 1 2),$retry,(End 1)) @($first.action,$retry.action)
if($matched.abortedBegins -ne 1 -or $matched.beginRecords -ne 3){throw 'Exact abort metadata did not retain the source begin/abort counts'}
$legacy1=Begin 0 $null $false;$legacy2=Begin 1 $null $true
$legacy1.Remove('hostSeed');$legacy2.Remove('hostSeed')
$legacy=Check-Journal 'legacy incomplete begins keep next-begin and EOF replay semantics' @($legacy1,$legacy2) @($legacy1.action,$legacy2.action)
if($legacy.hostSeedProvenance -ne 'unknown' -or $null -ne $legacy.hostSeed){throw 'Legacy Host provenance was invented'}
$null=Check-Journal 'mismatched end cannot cancel a pending begin' @($first,(End 99),$second) @($first.action,$second.action)
$null=Check-Journal 'wrong abort ticket rejected' @($first,(Abort 0 2)) @() $true
$null=Check-Journal 'wrong abort step rejected' @($first,(Abort 1 1)) @() $true
$null=Check-Journal 'abort after accepted end rejected' @($first,(End 0),(Abort 0 1)) @() $true
$null=Check-Journal 'abort after rejected end rejected' @($first,(End 0 'rejected'),(Abort 0 1)) @() $true
$null=Check-Journal 'abort without begin rejected' @((Abort 0 1)) @() $true
$null=Check-Journal 'repeated abort rejected' @($first,(Abort 0 1),(Abort 0 1)) @() $true
$null=Check-Journal 'old begin cannot be aborted after later begin' @($first,$second,(Abort 0 1)) @() $true
$null=Check-Journal 'dispatched action cannot be erased by abort' @($first,(Abort 0 1 $true)) @() $true
$null=Check-Journal 'string false cannot forge an undispatched abort' @($first,(Abort 0 1 'false')) @() $true
$null=Check-Journal 'string ticket cannot forge an exact abort' @($first,(Abort 0 '1')) @() $true
$null=Check-Journal 'floating point ticket cannot forge an exact abort' @($first,(Abort 0 1.5)) @() $true
$null=Check-Journal 'missing ticket cannot forge an exact abort' @($first,(Abort 0 $null)) @() $true
$null=Check-Journal 'negative step cannot forge an exact abort' @($first,(Abort -1 1)) @() $true
$null=Check-Journal 'legacy ticketless begin cannot be cancelled by a forged abort' @($legacy1,(Abort 0 1)) @() $true
$null=Check-Journal 'a journal containing only an undispatched abort has no replay actions' @($first,(Abort 0 1)) @() $true 'no recorded actions'
$abortedUnknown=Begin 1 2 $true;$abortedUnknown.Remove('hostSeed')
$unknown=Check-Journal 'aborted begin still contributes truthful Host-seed provenance' @($first,(End 0),$abortedUnknown,(Abort 1 2),$retry,(End 1)) @($first.action,$retry.action)
if($unknown.hostSeedProvenance -ne 'unknown'){throw 'Aborted metadata hid unknown historical Host provenance'}
$testTokens=$null;$testErrors=$null
$null=[Management.Automation.Language.Parser]::ParseFile($PSCommandPath,[ref]$testTokens,[ref]$testErrors)
if($testErrors.Count -gt 0){throw ($testErrors|Out-String)}
$summary=[ordered]@{passed=$results.Count;parserPassed=$true;nativeLaunched=$false;gpuWorkload=$false;buildWorkload=$false;
    driverSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $driver).Hash;testsSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash;cases=$results.ToArray()}
[IO.File]::WriteAllText((Join-Path $scratch 'assessment.json'),($summary|ConvertTo-Json -Depth 30),$utf8)
Write-Output "$($results.Count) replay assessment/journal cases passed; both parsers passed; no native, GPU or build workload ran."
Write-Output "Evidence: $(Join-Path $scratch 'assessment.json')"

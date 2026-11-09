$ErrorActionPreference='Stop'
$driver='D:/AraBuild/ux-editor-target/orchestration/monkey-acceptance.ps1'
$target='D:/AraBuild/ux-editor-target'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($driver,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
foreach($name in @('Property','True','False','Is-Counter','Assess-Journal','Assess-Request','Assess-EvidenceMode','Under','DPath','Read-Json','Hash','Assess','Counts','Crash-Evidence','Assess-Crashes')) {
 $fn=$ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)|Where-Object {$_.Name -eq $name}|Select-Object -First 1
 if($null -eq $fn){throw "Missing assessment helper $name"}
 Invoke-Expression $fn.Extent.Text
}
$baseline=@{
 seed=104729;hostSeed=104729;hostSeedProvenance='startup';seconds=1801.;requestedSeconds=1800;requestedSteps=$null;actions=500;frames=1000;panics=@();hangs=0;panelFailures=0;frameSpikes=1;nativeEventsRouted=100;diagnosticRecordsOmitted=0;
 journalRecorder=@{finalDrained=$true;error=$null;pendingAction=$null;abortedAction=$null;errorCount=0;recordsNotQueued=0;metadataRecordsPending=0;metadataBytesPending=0;queuedForSubmission=$false;recordsAttempted=100;recordsFlushed=100;bytesAttempted=5000;bytesFlushed=5000;lastTicket=20;completedTicket=20};
 offscreenGpuOutput=$true;gpuSurfaceSubmissions=100;nativePresentations=0;gpuOutputStalls=0;gpuOutputHealth=@{observed=$true;healthy=$true;stalls=0;unknownSamples=0};
 finished=$true;accepted=$true;finalDiagnosticsCollected=$true;cleanupIncomplete=$false;diagnosticBusLoss=$false;replayedJournal=$false;
 endDrain=@{cleanupIncomplete=$false;heldKeys=@();heldButtons=@()};playResources=@{ownedActive=@();ownedStopping=@()};soakRequested=$true;soakAccepted=$true;durationCompleted=$true;coverageComplete=$true;
 shellCommandCoverageComplete=$true;typedCommandCatalogReady=$true;typedCommandCoverageComplete=$true;typedCommandCatalogFinalMatched=$true;typedCommandCatalog=@('entity.create');typedCommandCatalogErrors=@();missingTypedCommandEdgeCases=@();typedCommandCounts=@{ 'entity.create'=@{attempts=7;accepted=1;committedTransactions=1}};typedCommandJobs=@{drained=$true};unvisitedCommands=@();missingCommandEdgeCases=@();termination='elapsed-duration';
 shellCommandCatalogFinalMatched=$true;shellCommandCatalogSettled=$true;shellCommandCatalogObserved=@(@{id='entity.create';builtin=$true;generation=$null});shellCommandCatalogFinal=@('entity.create');shellCommandCatalogErrors=@();shellCommandCatalogFinalUnloaded=@();shellRegistrationCases=@(@{registration=@{id='entity.create';builtin=$true;generation=$null};cases=@(0..11)});edgeCasesPerCommand=12;reportWriter=@{finalSnapshotRequested=$true;error=$null};
 hostJobResources=@{baselineCollected=$true;finalInventoryCollected=$true;drained=$true;hostUnsaved=$false;fileFlushPending=$false;errors=@();shell=@{busy=$false}}
}|ConvertTo-Json -Depth 20
$run=[pscustomobject]@{completed=$true;exitCode=0;requestedEvidenceMode='correctness-shared';frameTimeStatus='informal (shared GPU)';timingAccepted=$false;gpuWrapperUsed=$false;launchExecutable='D:\AraBuild\ux-editor-target\debug\ara-editor.exe';processRole='editor';arguments=@('isolated-project','--verify-monkey')}
$results=New-Object 'Collections.Generic.List[object]'
function Check([string]$Name,[scriptblock]$Change,[bool]$Reject,[bool]$Soak=$true) {
 $report=$baseline|ConvertFrom-Json
 & $Change $report
 $duration=if($Soak){1800}else{60}
 $failures=@(Assess $report 104729 $duration $Soak $run)
 if(($failures.Count -gt 0) -ne $Reject){throw "${Name}: unexpected assessment $($failures|ConvertTo-Json -Compress)"}
 $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;reasons=$failures})
}
Check 'complete typed and shell soak' {param($r)} $false
Check 'missing offscreen mode evidence' {param($r) $r.PSObject.Properties.Remove('offscreenGpuOutput')} $true
Check 'hidden windows must not present' {param($r) $r.nativePresentations=1} $true
Check 'no actual GPU submission' {param($r) $r.gpuSurfaceSubmissions=0} $true
Check 'missing GPU submission evidence' {param($r) $r.PSObject.Properties.Remove('gpuSurfaceSubmissions')} $true
Check 'missing native presentation evidence' {param($r) $r.PSObject.Properties.Remove('nativePresentations')} $true
Check 'unobserved GPU health' {param($r) $r.gpuOutputHealth.observed=$false} $true
Check 'unknown GPU health' {param($r) $r.gpuOutputHealth.healthy=$null} $true
Check 'recovered GPU stall still fails acceptance' {param($r) $r.gpuOutputStalls=1;$r.gpuOutputHealth.stalls=1} $true
Check 'missing GPU health samples' {param($r) $r.gpuOutputHealth.unknownSamples=1} $true
Check 'absent GPU stall evidence' {param($r) $r.PSObject.Properties.Remove('gpuOutputStalls')} $true
Check 'missing Host seed remains unproven' {param($r) $r.PSObject.Properties.Remove('hostSeed')} $true
Check 'Host seed differs from event generator' {param($r) $r.hostSeed=17} $true
Check 'Host startup provenance unknown' {param($r) $r.hostSeedProvenance='unknown'} $true
Check 'shell catalog final mismatch' {param($r) $r.shellCommandCatalogFinalMatched=$false} $true
Check 'shell registration discovery unsettled' {param($r) $r.shellCommandCatalogSettled=$false} $true
Check 'shell registration discovery errors' {param($r) $r.shellCommandCatalogErrors=@('editor.commands unavailable')} $true
Check 'final C# registration still unloaded' {param($r) $r.shellCommandCatalogFinalUnloaded=@('script.late')} $true
Check 'absent final shell inventory' {param($r) $r.PSObject.Properties.Remove('shellCommandCatalogFinal')} $true
Check 'empty final shell inventory' {param($r) $r.shellCommandCatalogFinal=@()} $true
Check 'null shell error inventory is unknown' {param($r) $r.shellCommandCatalogErrors=$null} $true
Check 'missing registration coverage inventory' {param($r) $r.PSObject.Properties.Remove('shellRegistrationCases')} $true
Check 'unprobed new registration generation' {param($r) $r.missingCommandEdgeCases=@('script.late@generation2:missing-reference')} $true
Check 'missing registration attempts is unknown' {param($r) $r.PSObject.Properties.Remove('missingCommandEdgeCases')} $true
Check 'missing unvisited list is unknown' {param($r) $r.PSObject.Properties.Remove('unvisitedCommands')} $true
Check 'duplicate registration cases cannot inflate coverage' {param($r) $r.shellRegistrationCases[0].cases=@(0..10)+10} $true
Check 'missing registration generation' {param($r) $r.shellRegistrationCases[0].registration.builtin=$false} $true
Check 'repeated registration tuples' {param($r) $r.shellRegistrationCases+=@($r.shellRegistrationCases[0])} $true
Check 'final evidence writer not requested' {param($r) $r.reportWriter.finalSnapshotRequested=$false} $true
Check 'background evidence IO failure' {param($r) $r.reportWriter.error='disk failure'} $true
Check 'missing typed coverage fields' {param($r) $r.PSObject.Properties.Remove('typedCommandCounts');$r.PSObject.Properties.Remove('typedCommandCoverageComplete')} $true
Check 'typed coverage incomplete' {param($r) $r.typedCommandCoverageComplete=$false;$r.missingTypedCommandEdgeCases=@('entity.create:missing-reference')} $true
Check 'typed catalog failed' {param($r) $r.typedCommandCatalogErrors=@('schema.describe failed')} $true
Check 'typed job drain incomplete' {param($r) $r.typedCommandJobs.drained=$false} $true
Check 'native events absent' {param($r) $r.nativeEventsRouted=0} $true
Check 'typed catalog drifted after startup' {param($r) $r.typedCommandCatalogFinalMatched=$false} $true
Check 'shell work still running' {param($r) $r.hostJobResources.shell.busy=$true} $true
Check 'host job baseline absent' {param($r) $r.hostJobResources.baselineCollected=$false} $true
Check 'final file flush incomplete' {param($r) $r.hostJobResources.fileFlushPending=$true;$r.hostJobResources.hostUnsaved=$true} $true
Check 'bounded baseline cannot claim soak' {param($r) $r.seconds=60.1;$r.requestedSeconds=60;$r.soakRequested=$false;$r.soakAccepted=$false;$r.coverageComplete=$false} $false $false

function Check-Mode([string]$Name,[string]$Mode,[scriptblock]$Change,[bool]$Reject) {
 $manifest=$run|ConvertTo-Json -Depth 10|ConvertFrom-Json
 & $Change $manifest
 $failures=@(Assess-EvidenceMode $manifest $Mode)
 if(($failures.Count -gt 0) -ne $Reject){throw "${Name}: unexpected evidence-class assessment $($failures|ConvertTo-Json -Compress)"}
 $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;reasons=$failures})
}
Check-Mode 'shared correctness direct launch is informal only' 'correctness-shared' {param($r)} $false
Check-Mode 'shared correctness cannot claim timing acceptance' 'correctness-shared' {param($r) $r.timingAccepted=$true} $true
Check-Mode 'shared correctness cannot silently queue on the GPU wrapper' 'correctness-shared' {param($r) $r.gpuWrapperUsed=$true;$r.processRole='ara-dev gpu wrapper';$r.launchExecutable='D:\ara-dev.exe'} $true
Check-Mode 'missing evidence class remains unknown' 'correctness-shared' {param($r) $r.PSObject.Properties.Remove('requestedEvidenceMode')} $true
$formal={param($r) $r.requestedEvidenceMode='timing-exclusive';$r.frameTimeStatus='formal (exclusive GPU)';$r.timingAccepted=$true;$r.gpuWrapperUsed=$true;$r.processRole='ara-dev gpu wrapper';$r.launchExecutable='D:\ara-dev.exe';$r.arguments=@('gpu','--wait-for-quiet','--timeout','300','--','D:\ara-editor.exe')}
Check-Mode 'exclusive timing requires successful production GPU wrapper' 'timing-exclusive' $formal $false
Check-Mode 'exclusive timing cannot use shared direct launch' 'timing-exclusive' {param($r)} $true
Check-Mode 'exclusive foreign GPU rejection is not accepted timing' 'timing-exclusive' {param($r) & $formal $r;$r.exitCode=3} $true
function Check-Request([string]$Name,[string]$Mode,[UInt64[]]$RequestedSeeds,[bool]$Reject) {
 $failures=@(Assess-Request $Mode $RequestedSeeds)
 if(($failures.Count -gt 0) -ne $Reject){throw "${Name}: unexpected run request assessment"}
 $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;reasons=$failures})
}
Check-Request 'default three seeds are shared correctness' 'correctness-shared' @(104729,130363,155921) $false
Check-Request 'exclusive timing rejects accidental default three seeds' 'timing-exclusive' @(104729,130363,155921) $true
Check-Request 'exclusive timing accepts one explicit seed' 'timing-exclusive' @(104729) $false
Check-Request 'duplicate seeds cannot inflate acceptance count' 'correctness-shared' @(104729,104729) $true
Check-Request 'empty seed list cannot vacuously pass' 'correctness-shared' @() $true

# These files are explicitly synthetic and stay outside native acceptance outputs.
$fixture=DPath (Join-Path $target ('orchestration\assessment-fixture-'+[Guid]::NewGuid().ToString('N')))
$native=Join-Path $fixture 'native';$project=Join-Path $fixture 'project';$crashes=Join-Path $native 'captures\.ara-profile\crashes'
[IO.Directory]::CreateDirectory($crashes)|Out-Null;[IO.Directory]::CreateDirectory($project)|Out-Null
[IO.File]::WriteAllText((Join-Path $native 'stderr.log'),'')
[IO.File]::WriteAllText((Join-Path $fixture 'SYNTHETIC.txt'),'Acceptance-helper test data; no editor or native soak ran.')
function Check-Crash([string]$Name,$Evidence,[bool]$Reject) {
 $failures=@(Assess-Crashes $Evidence)
 if(($failures.Count -gt 0) -ne $Reject){throw "${Name}: unexpected crash assessment"}
 $results.Add([pscustomobject]@{case=$Name;passed=$true;rejected=$Reject;reasons=$failures})
}
Check-Crash 'fresh crash inspection has no faults' (Crash-Evidence $native $project) $false
Check-Crash 'missing independent crash inspection fails closed' $null $true
[IO.File]::WriteAllText((Join-Path $crashes 'synthetic.crash.json'),' {"format":"ara.crash/1","kind":"panic","buildId":"synthetic","message":"Background worker panic","stack":"synthetic"} ')
$evidence=Crash-Evidence $native $project
if($evidence.panicArtifacts -ne 1){throw 'Structured global panic was not found under the actual isolated-profile crash path'}
Check-Crash 'background panic artifact fails even with empty action panic list' $evidence $true
[IO.File]::WriteAllText((Join-Path $native 'stderr.log'),"thread 'owned worker' panicked at code.rs:1:1")
$evidence=Crash-Evidence $native $project
if(@($evidence.stderrPanicLines).Count -ne 1){throw 'Independent stderr panic was not found'}
Check-Crash 'stderr panic fallback fails closed' $evidence $true
[IO.File]::WriteAllText((Join-Path $crashes 'incomplete.crash.json'),'{incomplete')
$evidence=Crash-Evidence $native $project
if(@($evidence.errors).Count -eq 0){throw 'An unreadable crash artifact must remain an inspection error'}
Check-Crash 'unreadable crash artifact fails closed' $evidence $true
$historical=Counts ([pscustomobject]@{seconds=1.;panics=@()})
if($null -ne $historical.globalPanicArtifacts -or $null -ne $historical.hostJobResources){throw 'Historical reports must retain unknown global-panic/resource evidence'}
$results.Add([pscustomobject]@{case='historical new evidence stays unknown';passed=$true;rejected=$false;reasons=@()})
$stale=Counts ([pscustomobject]@{hangs=0;gpuOutputStalls=0}) $null ([pscustomobject]@{hangs=1})
if($stale.hangs -ne 1 -or $stale.reportedHangs -ne 0 -or $stale.watchdogHangs -ne 1){throw 'Independent watchdog stall was erased by a stale report'}
$results.Add([pscustomobject]@{case='watchdog stall survives a stale clean checkpoint';passed=$true;rejected=$true;reasons=@()})
$watchdogOnly=Counts $null $null ([pscustomobject]@{hangs=1})
if($watchdogOnly.hangs -ne 1 -or $null -ne $watchdogOnly.panics -or $null -ne $watchdogOnly.reportedHangs){throw 'Watchdog-only evidence fabricated missing report counts'}
$results.Add([pscustomobject]@{case='watchdog-only evidence retains unknown report counts';passed=$true;rejected=$true;reasons=@()})
if($null -ne $historical.hangs -or $null -ne $historical.offscreenGpuOutput -or $null -ne $historical.gpuOutputHealth){throw 'Historical missing output or hang evidence must remain unknown'}
$results.Add([pscustomobject]@{case='historical GPU and hang evidence stays unknown';passed=$true;rejected=$false;reasons=@()})
$output=[ordered]@{driverSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $driver).Hash;testKind='synthetic assessment checks only; no native soak executed';fixture=$fixture;passed=$results.Count;cases=$results.ToArray()}
$output|ConvertTo-Json -Depth 20|Set-Content -LiteralPath 'D:/AraBuild/ux-editor-target/orchestration/monkey-driver-tests.json' -Encoding UTF8
Write-Output "$($results.Count) driver acceptance cases passed; no native workload was run."

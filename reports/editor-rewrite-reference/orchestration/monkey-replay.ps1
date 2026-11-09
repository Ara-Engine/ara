<#
Replay a preserved native journal on a fresh copy of its frozen pre-run fixture.
This script never builds, deletes a fixture, retries, or terminates a process.
Frame times are informal (shared GPU); journal exhaustion is a bounded check,
never thirty-minute soak acceptance. Run only after freezing/building the source.

  powershell -NoProfile -File D:\AraBuild\ux-editor-target\orchestration\monkey-replay.ps1 `
    -CheckpointSHA <built-sha> -OriginalRunDirectory <native-run-directory> `
    -FrozenFixtureRoot <unchanged-marked-fixture> -FixtureManifest <pre-run-fixture.json> `
    -BuildEvidence <matching-build-log>

Requires run-native.ps1's optional -MonkeyReplay adapter. Legacy journals without
hostSeed can still be replayed for diagnosis, but their historical Host entropy is
unknown and this driver does not claim deterministic Host-ID replay acceptance.
#>
param(
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-fA-F]{8,40}$')][string]$CheckpointSHA,
    [Parameter(Mandatory=$true)][string]$OriginalRunDirectory,
    [Parameter(Mandatory=$true)][string]$FrozenFixtureRoot,
    [Parameter(Mandatory=$true)][string]$FixtureManifest,
    [Parameter(Mandatory=$true)][string]$BuildEvidence,
    [ValidateRange(1,7200)][int]$BudgetSeconds=2100,
    [switch]$RequireJournalRecorder
)
Set-StrictMode -Version 2.0
$ErrorActionPreference='Stop'
$target='D:\AraBuild\ux-editor-target'
$source='D:\AraBuild\ux-editor-source'
$acceptance=Join-Path $target 'acceptance'
$runner=Join-Path $target 'orchestration\run-native.ps1'
$editor=Join-Path $target 'debug\ara-editor.exe'
$player=Join-Path $target 'debug\ara-player.exe'
$utf8=New-Object Text.UTF8Encoding($false)

function Property($Object,[string]$Name) {
    if($null -eq $Object){return $null}
    if($Object -is [Collections.IDictionary]){if($Object.Contains($Name)){return $Object[$Name]};return $null}
    $field=$Object.PSObject.Properties[$Name]
    if($null -ne $field){return $field.Value}
    return $null
}
function True($Value){return $Value -is [bool] -and $Value}
function False($Value){return $Value -is [bool] -and !$Value}
function Has-Property($Object,[string]$Name) {
    if($null -eq $Object){return $false}
    if($Object -is [Collections.IDictionary]){return $Object.Contains($Name)}
    return $null -ne $Object.PSObject.Properties[$Name]
}
function Recorder-Required([string]$MonkeySource,[bool]$Explicit=$false) {
    return $Explicit -or $MonkeySource -match '"journalRecorder"\s*:'
}
function Recorder-Contract($Report,[bool]$Required=$true) {
    if(Has-Property $Report 'journalRecorder'){return 'journal-recorder-final-drain/1'}
    if($Required){return 'journal-recorder-final-drain/1-required-but-missing'}
    return 'historical-report-without-recorder/legacy'
}
function Under([string]$Path,[string]$Root) {
    return [IO.Path]::GetFullPath($Path).TrimEnd('\').StartsWith(
        [IO.Path]::GetFullPath($Root).TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)
}
function DPath([string]$Path) {
    $full=[IO.Path]::GetFullPath($Path)
    if(!($full -match '^[Dd]:\\') -or !(Under $full $target)){throw "Path escapes the D: task target: $full"}
    return $full
}
function Read-Json([string]$Path){return Get-Content -Raw -Encoding UTF8 -LiteralPath $Path|ConvertFrom-Json}
function Json([string]$Path,$Value){[IO.File]::WriteAllText((DPath $Path),($Value|ConvertTo-Json -Depth 60),$utf8)}
function Hash([string]$Path) {
    $file=Get-Item -LiteralPath $Path
    if($file.PSIsContainer){throw "Expected a file: $Path"}
    return [ordered]@{path=$file.FullName;bytes=$file.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash;modifiedUtc=$file.LastWriteTimeUtc.ToString('o')}
}
function Quote-Argument([string]$Value){return '"'+(($Value -replace '(\\*)"','$1$1\"') -replace '(\\+)$','$1$1')+'"'}
function Read-Git([string[]]$Arguments) {
    $old=$env:GIT_OPTIONAL_LOCKS
    try {
        $env:GIT_OPTIONAL_LOCKS='0';$value=@(& git.exe -C $source @Arguments)
        if($LASTEXITCODE -ne 0){throw 'Read-only source identity inspection failed'}
        return $value
    } finally {$env:GIT_OPTIONAL_LOCKS=$old}
}
function Identity {
    $sha=(Read-Git @('rev-parse','--verify','HEAD')|Select-Object -First 1).Trim()
    if(!$sha.StartsWith($CheckpointSHA,[StringComparison]::OrdinalIgnoreCase)){throw "Built-source checkpoint mismatch: $sha"}
    if(@(Read-Git @('status','--porcelain','--untracked-files=normal')).Count -ne 0){throw 'Frozen D: source is dirty'}
    $files=@('crates\ara-editor\src\main.rs','crates\ara-editor\src\app\setup.rs','crates\ara-editor\src\app\mod.rs','crates\ara-editor\src\app\monkey.rs','crates\ara-editor\src\app\monkey_route.rs')|ForEach-Object{Join-Path $source $_}
    $files+=@(Get-ChildItem -LiteralPath (Join-Path $source 'crates\ara-editor\src\app\monkey') -Filter '*.rs' -Recurse -File|ForEach-Object{$_.FullName})
    $sdkTools=@('Ara.CompileServer','Ara.Runtime','Ara.Editor','Ara.Generators','Ara.Analyzers')|ForEach-Object{Hash (Join-Path $target "sdk-tools/bin/Ara.CompileServer/release/$_.dll")}
    $sdkTools+=Hash (Join-Path $target 'sdk-tools/bin/Ara.Host/release/Ara.Host.dll')
    $recorderRequired=Recorder-Required ([IO.File]::ReadAllText((Join-Path $source 'crates/ara-editor/src/app/monkey.rs'))) ([bool]$RequireJournalRecorder)
    return [ordered]@{commit=$sha;tree=(Read-Git @('rev-parse','HEAD^{tree}')|Select-Object -First 1).Trim();journalRecorderRequired=$recorderRequired;
        editor=(Hash $editor);player=(Hash $player);runner=(Hash $runner);sourceFiles=@($files|Sort-Object|ForEach-Object{Hash $_});sdkTools=$sdkTools}
}
function Same-Identity($Expected) {
    $actual=Identity
    if($actual.commit -ne $Expected.commit -or $actual.tree -ne $Expected.tree){throw 'Source changed during replay'}
    foreach($key in @('editor','player','runner')){if($actual[$key].sha256 -ne $Expected[$key].sha256){throw "$key bytes changed during replay"}}
    if(@(Compare-Object $Expected.sdkTools $actual.sdkTools -Property path,bytes,sha256).Count -gt 0){throw 'Prepared C# SDK tool bytes changed during replay'}
    return $actual
}
function Inventory([string]$Root) {
    $full=(DPath $Root).TrimEnd('\');$pending=New-Object 'Collections.Generic.Stack[string]';$pending.Push($full)
    while($pending.Count -gt 0) {
        $directory=$pending.Pop()
        foreach($item in Get-ChildItem -Force -LiteralPath $directory) {
            if($directory -eq $full -and $item.Name -in @('.ara','.git','.ara-profile','target','builds','.ara-monkey-fixture','.ara-behaviour-fixture')){continue}
            if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw "Fixture has a reparse point: $($item.FullName)"}
            if(!(Under $item.FullName $full)){throw 'Fixture inventory escaped its root'}
            if($item.PSIsContainer){$pending.Push($item.FullName);continue}
            [pscustomobject][ordered]@{relative=$item.FullName.Substring($full.Length+1).Replace('\','/');bytes=$item.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash}
        }
    }
}
function Add-JournalAction($Record,[long]$Line,$State) {
    $action=Property $Record 'action'
    if($null -eq $action){throw "No recorded action on journal line $Line"}
    $encoded=ConvertTo-Json -InputObject $action -Depth 100 -Compress
    $bytes=$utf8.GetBytes($encoded+"`n")
    $null=$State.digest.TransformBlock($bytes,0,$bytes.Length,$bytes,0)
    $State.actionCount++
}
function Same-JournalStep($Left,$Right) {
    if($null -eq $Left -or $null -eq $Right){return $null -eq $Left -and $null -eq $Right}
    if((Is-Counter $Left) -and (Is-Counter $Right)){return [UInt64]$Left -eq [UInt64]$Right}
    return (ConvertTo-Json -InputObject $Left -Depth 100 -Compress) -ceq (ConvertTo-Json -InputObject $Right -Depth 100 -Compress)
}
function Read-Journal([string]$Path) {
    $reader=[IO.File]::OpenText($Path);$digest=[Security.Cryptography.SHA256]::Create()
    $seed=[UInt64]0;$hostSeed=$null;$allHostSeedsKnown=$true;$lineNumber=0L;$pending=$null;$aborts=0L;$begins=0L
    $state=@{digest=$digest;actionCount=0L}
    try {
        while($null -ne ($line=$reader.ReadLine())) {
            $lineNumber++
            if([string]::IsNullOrWhiteSpace($line)){continue}
            $record=$line|ConvertFrom-Json
            $phase=Property $record 'phase'
            if($phase -ceq 'begin') {
                if($null -ne $pending){Add-JournalAction $pending.record $pending.line $state}
                $current=Property $record 'seed'
                if(Is-Counter $current){$seed=[UInt64]$current}
                $known=Property $record 'hostSeed'
                if($null -eq $known){$allHostSeedsKnown=$false}
                else {
                    if(!(Is-Counter $known)){throw "Invalid Host seed on journal line $lineNumber"}
                    $known=[UInt64]$known
                    if($known -ne $seed -or ($null -ne $hostSeed -and $known -ne $hostSeed)){throw "Inconsistent Host/generator seed on journal line $lineNumber"}
                    $hostSeed=$known
                }
                $pending=@{line=$lineNumber;record=$record};$begins++
            } elseif($phase -ceq 'end') {
                if($null -ne $pending -and (Same-JournalStep (Property $pending.record 'step') (Property $record 'step'))) {
                    Add-JournalAction $pending.record $pending.line $state
                    $pending=$null
                }
            } elseif($phase -ceq 'abort') {
                $ticket=Property $record 'ticket';$step=Property $record 'step'
                $matching=$null -ne $pending -and (False (Property $record 'wasDispatched')) -and (Is-Counter $ticket) -and (Is-Counter $step)
                if($matching) {
                    $beginTicket=Property $pending.record 'ticket';$beginStep=Property $pending.record 'step'
                    $matching=(Is-Counter $beginTicket) -and (Is-Counter $beginStep) -and [UInt64]$ticket -eq [UInt64]$beginTicket -and [UInt64]$step -eq [UInt64]$beginStep
                }
                if(!$matching){throw "Replay line ${lineNumber}: abort must match an uncompleted begin's exact ticket and step"}
                $pending=$null;$aborts++
            }
        }
        if($null -ne $pending){Add-JournalAction $pending.record $pending.line $state}
        if($state.actionCount -eq 0){throw 'Journal has no recorded actions to replay'}
        $null=$digest.TransformFinalBlock([byte[]]@(),0,0)
        return [ordered]@{actions=$state.actionCount;beginRecords=$begins;abortedBegins=$aborts;seed=$seed;hostSeed=if($allHostSeedsKnown){$hostSeed}else{$null};
            hostSeedProvenance=if($allHostSeedsKnown -and $null -ne $hostSeed){'journal'}else{'unknown'};
            actionSha256=[BitConverter]::ToString($digest.Hash).Replace('-','');
            comparison='Production begin/end/exact-abort semantics; compact action JSON in recorded property order; rejected ended actions included; only exact uncompleted abort excluded; source journal bytes unchanged'}
    } finally {$reader.Dispose();$digest.Dispose()}
}
function Require-Array($Object,[string]$Name,$Failures) {
    if($null -eq $Object){$Failures.Add("Missing array: $Name");return}
    $field=$Object.PSObject.Properties[$Name]
    if($null -eq $field -or $field.Value -isnot [Array]){$Failures.Add("Missing array: $Name")}
}
function Is-Counter($Value) {
    return ($Value -is [int] -or $Value -is [long] -or $Value -is [uint32] -or $Value -is [uint64]) -and $Value -ge 0
}
function Assess-Output($Report) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if(!(True (Property $Report 'offscreenGpuOutput'))){$failures.Add('Replay does not confirm offscreen GPU output')}
    $submissions=Property $Report 'gpuSurfaceSubmissions'
    if(!(Is-Counter $submissions) -or $submissions -le 0){$failures.Add('Replay has no proven GPU surface submissions')}
    $presentations=Property $Report 'nativePresentations'
    if(!(Is-Counter $presentations) -or $presentations -ne 0){$failures.Add('Replay does not prove zero native presentations')}
    $stalls=Property $Report 'gpuOutputStalls'
    if(!(Is-Counter $stalls) -or $stalls -ne 0){$failures.Add('Replay does not prove zero GPU-output stalls')}
    $health=Property $Report 'gpuOutputHealth'
    foreach($key in @('observed','healthy')){if(!(True (Property $health $key))){$failures.Add("GPU-output health does not confirm $key")}}
    foreach($key in @('stalls','unknownSamples')) {
        $count=Property $health $key
        if(!(Is-Counter $count) -or $count -ne 0){$failures.Add("GPU-output health does not prove zero $key")}
    }
    return @($failures)
}
function Assess-Recorder($Report,[bool]$Required=$true) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if(!(Has-Property $Report 'journalRecorder')) {
        if($Required){$failures.Add('Checkpoint requires journalRecorder final-drain proof, but the report field is missing')}
        return @($failures)
    }
    $recorder=Property $Report 'journalRecorder'
    if($null -eq $recorder){$failures.Add('New report journalRecorder is null');return @($failures)}
    if(!(True (Property $recorder 'finalDrained'))){$failures.Add('Journal recorder does not confirm finalDrained')}
    foreach($key in @('error','pendingAction','abortedAction')) {
        if(!(Has-Property $recorder $key) -or $null -ne (Property $recorder $key)){$failures.Add("Journal recorder does not prove explicit null $key")}
    }
    foreach($key in @('errorCount','recordsNotQueued')) {
        $count=Property $recorder $key
        if(!(Is-Counter $count) -or $count -ne 0){$failures.Add("Journal recorder does not prove zero $key")}
    }
    foreach($pair in @(@('recordsAttempted','recordsFlushed'),@('bytesAttempted','bytesFlushed'))) {
        $attempted=Property $recorder $pair[0];$flushed=Property $recorder $pair[1]
        if(!(Is-Counter $attempted) -or !(Is-Counter $flushed) -or [UInt64]$attempted -ne [UInt64]$flushed){$failures.Add("Journal recorder $($pair[0]) and $($pair[1]) are not proven equal")}
    }
    return @($failures)
}
function Assess($Report,$Run,$Original,$Replay,[UInt64]$Seed,[bool]$RequireRecorder=$true) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if(!(True (Property $Run 'completed')) -or (Property $Run 'exitCode') -ne 0){$failures.Add('Native runner did not complete with exit code zero')}
    if((Property $Run 'requestedEvidenceMode') -ne 'correctness-shared' -or (Property $Run 'frameTimeStatus') -ne 'informal (shared GPU)' -or !(False (Property $Run 'timingAccepted')) -or !(False (Property $Run 'gpuWrapperUsed'))){$failures.Add('Replay evidence was not correctly classified as shared-GPU correctness')}
    if($null -eq $Report){$failures.Add('No authoritative monkey.json');return @($failures)}
    foreach($failure in @(Assess-Output $Report)){$failures.Add($failure)}
    foreach($failure in @(Assess-Recorder $Report $RequireRecorder)){$failures.Add($failure)}
    foreach($key in @('finished','accepted','replayedJournal','finalDiagnosticsCollected')){if(!(True (Property $Report $key))){$failures.Add("Report does not confirm $key")}}
    foreach($key in @('soakRequested','soakAccepted','cleanupIncomplete','diagnosticBusLoss')){if(!(False (Property $Report $key))){$failures.Add("Bounded replay does not rule out $key")}}
    if((Property $Report 'termination') -ne 'replay-exhausted' -or (Property $Report 'replayRemaining') -ne 0){$failures.Add('The exact recorded action sequence was not exhausted')}
    if((Property $Report 'seed') -ne $Seed -or (Property $Report 'hostSeed') -ne $Seed -or (Property $Report 'hostSeedProvenance') -ne 'startup'){$failures.Add('Current replay did not seed the real Host from the recorded seed')}
    if($Original.hostSeedProvenance -ne 'journal'){$failures.Add('Historical Host seed is unknown; this is a best-effort action replay only')}
    if((Property $Report 'replayOriginalHostSeedProvenance') -ne 'journal' -or (Property $Report 'replayOriginalHostSeed') -ne $Original.hostSeed -or !(True (Property $Report 'hostSeedMatchesReplay'))){$failures.Add('Original and current Host seeds are not proven identical')}
    if($null -eq $Replay -or $Replay.actions -ne $Original.actions -or $Replay.actionSha256 -ne $Original.actionSha256){$failures.Add('Replayed action objects/count differ from the frozen original journal')}
    foreach($key in @('panics','unvisitedCommands','missingCommandEdgeCases')){Require-Array $Report $key $failures}
    if(@(Property $Report 'panics').Count -ne 0 -or (Property $Report 'hangs') -ne 0 -or (Property $Report 'panelFailures') -ne 0){$failures.Add('Replay reports a panic, hang, or quarantined panel')}
    if((Property $Report 'diagnosticRecordsOmitted') -ne 0){$failures.Add('Diagnostic records were omitted')}
    $drain=Property $Report 'endDrain';$play=Property $Report 'playResources';$jobs=Property $Report 'hostJobResources'
    foreach($key in @('heldKeys','heldButtons')){Require-Array $drain $key $failures;if(@(Property $drain $key).Count -gt 0){$failures.Add("Held input remains: $key")}}
    if(!(False (Property $drain 'cleanupIncomplete'))){$failures.Add('Replay input/resource drain is incomplete')}
    foreach($key in @('ownedActive','ownedStopping')){Require-Array $play $key $failures;if(@(Property $play $key).Count -gt 0){$failures.Add("Owned Play resources remain: $key")}}
    foreach($key in @('baselineCollected','finalInventoryCollected','drained')){if(!(True (Property $jobs $key))){$failures.Add("Host job inventory does not confirm $key")}}
    foreach($key in @('hostUnsaved','fileFlushPending')){if(!(False (Property $jobs $key))){$failures.Add("Host job drain does not rule out $key")}}
    Require-Array $jobs 'errors' $failures
    if(@(Property $jobs 'errors').Count -gt 0 -or !(False (Property (Property $jobs 'shell') 'busy'))){$failures.Add('Host inventory errors or Shell work remain')}
    $typed=Property $Report 'typedCommandJobs';if(!(True (Property $typed 'drained'))){$failures.Add('Owned typed-command jobs remain')}
    $writer=Property $Report 'reportWriter'
    if($null -eq $writer -or !(True (Property $writer 'finalSnapshotRequested')) -or $null -ne (Property $writer 'error')){$failures.Add('No clean authoritative final report-writer snapshot')}
    # Partial original journals need not cover the complete catalog. Their coverage
    # is retained as evidence, never promoted into duration-soak acceptance.
    return @($failures)
}
function Crash-Evidence([string]$RunRoot,[string]$Project) {
    $reports=New-Object 'Collections.Generic.List[object]';$dumps=New-Object 'Collections.Generic.List[object]';$errors=New-Object 'Collections.Generic.List[string]';$panicLines=New-Object 'Collections.Generic.List[object]'
    $roots=@((Join-Path $RunRoot 'crashes'),(Join-Path $RunRoot 'captures\.ara-profile\crashes'),(Join-Path $Project '.ara\play'),(Join-Path $Project 'builds'))
    $logs=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    $stderr=Join-Path $RunRoot 'stderr.log'
    if(Test-Path -LiteralPath $stderr){$null=$logs.Add($stderr)}else{$errors.Add('Native stderr is missing')}
    foreach($root in $roots) {
        if(!(Test-Path -LiteralPath $root)){continue}
        $pending=New-Object 'Collections.Generic.Stack[string]';$pending.Push($root)
        while($pending.Count -gt 0) {
            $directory=$pending.Pop()
            foreach($item in Get-ChildItem -Force -LiteralPath $directory) {
                if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){$errors.Add("Crash inventory excluded reparse point $($item.FullName)");continue}
                if($item.PSIsContainer){$pending.Push($item.FullName);continue}
                if($item.Name -like '*.crash.json') {
                    try {$crash=Read-Json $item.FullName;if((Property $crash 'format') -ne 'ara.crash/1' -or (Property $crash 'kind') -notin @('panic','native','managed')){throw 'Unknown crash contract'};$reports.Add([ordered]@{artifact=(Hash $item.FullName);kind=(Property $crash 'kind');message=(Property $crash 'message')})}
                    catch {$errors.Add("Unreadable crash report: $($item.FullName)")}
                } elseif($item.Extension -ieq '.dmp'){$dumps.Add((Hash $item.FullName))}elseif($item.Extension -ieq '.log'){$null=$logs.Add($item.FullName)}
            }
        }
    }
    foreach($log in $logs) {
        foreach($match in Select-String -Encoding UTF8 -LiteralPath $log -Pattern '^thread .+ panicked at|^thread panicked at|^fatal runtime error:|^Unhandled exception\.') {
            $panicLines.Add([ordered]@{path=$log;line=$match.LineNumber;text=$match.Line.Substring(0,[Math]::Min(2048,$match.Line.Length))})
        }
    }
    return [ordered]@{reports=$reports.ToArray();dumps=$dumps.ToArray();errors=$errors.ToArray();stderrPanicLines=$panicLines.ToArray();
        backendWorkerRetirementVerified=$null;scope='Crash artifacts and logs from this run only; terminal jobs and exit do not prove retirement of every detached backend worker.'}
}

$OriginalRunDirectory=DPath $OriginalRunDirectory;$FrozenFixtureRoot=DPath $FrozenFixtureRoot
$FixtureManifest=DPath $FixtureManifest;$BuildEvidence=DPath $BuildEvidence
if(!(Under $OriginalRunDirectory $acceptance)){throw 'Original run must be preserved under D: acceptance'}
if(!(Test-Path -LiteralPath (Join-Path $FrozenFixtureRoot '.ara-monkey-fixture')) -and !(Test-Path -LiteralPath (Join-Path $FrozenFixtureRoot '.ara-behaviour-fixture'))){throw 'A marked frozen scratch fixture is required'}
$originalRunPath=DPath (Join-Path $OriginalRunDirectory 'run.json');$originalRun=Read-Json $originalRunPath
if((Property $originalRun 'mode') -ne 'monkey'){throw 'Original manifest is not a native monkey run'}
$originalJournal=DPath (Join-Path $OriginalRunDirectory 'captures\monkey-events.jsonl')
$originalMetadata=Read-Journal $originalJournal;$originalJournalIdentity=Hash $originalJournal
if((Property $originalRun 'seed') -ne $originalMetadata.seed){throw 'Original manifest and journal seed differ'}
$fixture=Read-Json $FixtureManifest
if(!(False (Property $fixture 'historyCopied')) -or [IO.Path]::GetFullPath((Property $fixture 'source')) -ne $FrozenFixtureRoot){throw 'Pre-run fixture manifest has no matching source/history exclusion proof'}
if(@(Property $originalRun 'arguments') -notcontains (Property $fixture 'project')){throw 'Fixture manifest is not bound to the original run project'}
$expected=@(Property $fixture 'files'|Sort-Object relative)
if($expected.Count -eq 0){throw 'Pre-run fixture inventory is empty'}
$inventory=@(Inventory $FrozenFixtureRoot|Sort-Object relative)
if((Compare-Object $expected $inventory -Property relative,bytes,sha256|Measure-Object).Count -ne 0){throw 'Frozen fixture differs from original pre-run bytes'}
$parseTokens=$null;$parseErrors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($runner,[ref]$parseTokens,[ref]$parseErrors)
if($parseErrors.Count -gt 0 -or @($ast.ParamBlock.Parameters|ForEach-Object{$_.Name.VariablePath.UserPath}) -notcontains 'MonkeyReplay'){throw 'run-native.ps1 needs its reviewed optional -MonkeyReplay parameter; no process was launched'}
$identity=Identity;$short=$identity.commit.Substring(0,8)
$output=DPath (Join-Path (Join-Path $acceptance $short) ('replay-'+$originalMetadata.seed+'-'+(Get-Date -Format yyyyMMdd-HHmmss)+'-'+[Guid]::NewGuid().ToString('N').Substring(0,8)))
if((Under $output $FrozenFixtureRoot) -or (Under $FrozenFixtureRoot $output)){throw 'Replay output and frozen fixture must be disjoint'}
$fixtureBytes=($inventory|Measure-Object bytes -Sum).Sum
if((New-Object IO.DriveInfo('D:\')).AvailableFreeSpace -lt ($fixtureBytes+$originalJournalIdentity.bytes*2+1GB)){throw 'D: has insufficient room for isolated replay evidence'}
[IO.Directory]::CreateDirectory($output)|Out-Null
$project=DPath (Join-Path $output 'project');[IO.Directory]::CreateDirectory($project)|Out-Null
$frozenInputs=DPath (Join-Path $output 'frozen-inputs');[IO.Directory]::CreateDirectory($frozenInputs)|Out-Null
$frozenJournal=Join-Path $frozenInputs 'monkey-events.jsonl';[IO.File]::Copy($originalJournal,$frozenJournal,$false)
if((Hash $frozenJournal).sha256 -ne $originalJournalIdentity.sha256){throw 'Frozen journal copy differs'}
$record=[ordered]@{format='ara.monkey.replay/1';startedUtc=[DateTime]::UtcNow.ToString('o');completed=$false;accepted=$false;boundedReplay=$true;soakAccepted=$false;timingAccepted=$false;
    frameTimeStatus='informal (shared GPU)';requestedEvidenceMode='correctness-shared';budgetSeconds=$BudgetSeconds;output=$output;project=$project;
    driver=(Hash $PSCommandPath);identity=$identity;buildEvidence=(Hash $BuildEvidence);fixtureManifest=(Hash $FixtureManifest);
    fixture=[ordered]@{source=$FrozenFixtureRoot;files=$inventory;historyCopied=$false;copyContract='The same pre-run file inventory as the original driver; no cached history or layout migration'};
    original=[ordered]@{run=(Hash $originalRunPath);manifest=$originalRun;journal=$originalJournalIdentity;metadata=$originalMetadata;frozenJournal=(Hash $frozenJournal)};
    binarySourceBinding='Source/tree and executable bytes are bound to the supplied build evidence; hashes alone do not prove the source used to build a binary.';
    assessmentContract=[ordered]@{journalRecorderRequired=$identity.journalRecorderRequired;reportContract='pending';basis='Frozen target source journalRecorder declaration or explicit -RequireJournalRecorder; legacy report assessment is labeled separately'};
    process=$null;productionRun=$null;raw=@();report=$null;journalComparison=$null;crashEvidence=$null;reasons=@();
    notes=@('Exact replay preserves recorded actions, not the outcome of a previous buggy binary. There is no duration/FPS/frame-spike gate.',
        'A missing historical hostSeed stays unknown; only current-format seed evidence can establish Host-ID reproducibility.',
        'Source journals, reports and fixture bytes are never overwritten. New profiles, copied fixture and output remain on D:.',
        'Bounded replay requires observed healthy offscreen GPU output, real surface submissions, zero native presentations and no GPU-output stalls or unknown observation samples.',
        'New reports also require a fully drained, error-free acknowledged journal recorder; this proves flush-to-OS buffers, not fsync or power-loss durability.',
        'Only the retained owned child handle is waited on. No process is killed, including on timeout.')}
Json (Join-Path $output 'replay.json') $record
try {
    foreach($file in $inventory) {
        $destination=DPath (Join-Path $project $file.relative)
        if(!(Under $destination $project)){throw 'Fixture manifest escaped the fresh project'}
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))|Out-Null
        [IO.File]::Copy((Join-Path $FrozenFixtureRoot $file.relative),$destination,$false)
    }
    $copied=@(Inventory $project|Sort-Object relative)
    if((Compare-Object $inventory $copied -Property relative,bytes,sha256|Measure-Object).Count -ne 0){throw 'Fresh replay fixture bytes differ'}
    [IO.File]::WriteAllText((Join-Path $project '.ara-monkey-fixture'),"Ara fresh native journal replay fixture`n",$utf8)
    foreach($key in @('ARA_CONFIG_DIR','ARA_CRASH_DIR','DOTNET_CLI_HOME','APPDATA','LOCALAPPDATA','XDG_CACHE_HOME','XDG_CONFIG_HOME')) {
        $directory=DPath (Join-Path (Join-Path $output 'launcher-profile') $key);[IO.Directory]::CreateDirectory($directory)|Out-Null
        [Environment]::SetEnvironmentVariable($key,$directory,'Process')
    }
    $temporary=DPath (Join-Path $target ('tmp\r-'+[Guid]::NewGuid().ToString('N').Substring(0,8)));[IO.Directory]::CreateDirectory($temporary)|Out-Null
    $env:TEMP=$temporary;$env:TMP=$temporary;$env:ARA_PLAY_OFFSCREEN='1';$env:CARGO_TARGET_DIR=$target;$env:CARGO_BUILD_BUILD_DIR=$target;$env:CARGO_BUILD_JOBS='3'
    [Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
    $null=Same-Identity $identity
    $shellExe=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $arguments=@('-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',$runner,'-CheckpointSHA',$short,'-Mode','monkey','-Project',$project,'-Seed',[string]$originalMetadata.seed,'-Seconds',[string]$BudgetSeconds,'-EvidenceMode','correctness-shared','-MonkeyReplay',$frozenJournal)
    $process=Start-Process -FilePath $shellExe -ArgumentList (($arguments|ForEach-Object{Quote-Argument $_}) -join ' ') -WorkingDirectory $source -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output 'launcher.stdout.log') -RedirectStandardError (Join-Path $output 'launcher.stderr.log')
    $null=$process.Handle
    $record.process=[ordered]@{pid=$process.Id;startedUtc=$process.StartTime.ToUniversalTime().ToString('o');executable=$shellExe;arguments=$arguments;completed=$false;exitCode=$null;timeoutSeconds=$BudgetSeconds+720}
    Json (Join-Path $output 'replay.json') $record
    Write-Output "Replay wrapper PID $($process.Id); evidence $output"
    $completed=$process.WaitForExit(($BudgetSeconds+720)*1000);$record.process.completed=$completed
    if(!$completed){throw 'Owned replay wrapper timed out; it was not terminated. Inspect its exact PID/birth before any further GPU work.'}
    $process.WaitForExit();$process.Refresh();$record.process.exitCode=$process.ExitCode
    $matches=@(Get-ChildItem -Directory -LiteralPath (Join-Path $acceptance $short)|ForEach-Object{
        $path=Join-Path $_.FullName 'run.json'
        if(Test-Path -LiteralPath $path){$run=Read-Json $path;if(@(Property $run 'arguments') -contains $project){[pscustomobject]@{path=$path;run=$run}}}
    })
    if($matches.Count -ne 1){throw 'Could not uniquely bind this project to one production run manifest'}
    $nativeRoot=DPath (Split-Path -Parent $matches[0].path);$run=$matches[0].run;$record.productionRun=$matches[0].path
    $raw=DPath (Join-Path $output 'raw');[IO.Directory]::CreateDirectory($raw)|Out-Null
    foreach($relative in @('run.json','stdout.log','stderr.log','captures\monkey.json','captures\monkey-events.jsonl','captures\monkey-watchdog.json','captures\monkey-typed-catalog.json')) {
        $from=DPath (Join-Path $nativeRoot $relative)
        if(Test-Path -LiteralPath $from){$to=Join-Path $raw ([IO.Path]::GetFileName($relative));[IO.File]::Copy($from,$to,$false);$record.raw+=Hash $to}
    }
    $reportPath=Join-Path $raw 'monkey.json';$report=$null
    if(Test-Path -LiteralPath $reportPath){$report=Read-Json $reportPath}
    $replayedPath=Join-Path $raw 'monkey-events.jsonl';$replayed=$null
    if(Test-Path -LiteralPath $replayedPath){$replayed=Read-Journal $replayedPath}
    $record.journalComparison=[ordered]@{original=$originalMetadata;replay=$replayed;matched=$null -ne $replayed -and $replayed.actions -eq $originalMetadata.actions -and $replayed.actionSha256 -eq $originalMetadata.actionSha256}
    $record.assessmentContract.reportContract=Recorder-Contract $report $identity.journalRecorderRequired
    $record.reasons=@(Assess $report $run $originalMetadata $replayed $originalMetadata.seed $identity.journalRecorderRequired)
    if($record.process.exitCode -ne 0){$record.reasons+='Production replay wrapper exited nonzero'}
    if(@(Property $run 'arguments') -notcontains '--monkey-replay' -or @(Property $run 'arguments') -notcontains $frozenJournal){$record.reasons+='Production run was not bound to the frozen replay journal'}
    $replayBinding=Property $run 'replayJournal'
    if($null -eq $replayBinding -or (Property $replayBinding 'sha256') -ne (Hash $frozenJournal).sha256){$record.reasons+='No matching immutable replay-journal binding in run.json'}
    $watchdogPath=Join-Path $raw 'monkey-watchdog.json'
    if(!(Test-Path -LiteralPath $watchdogPath)){$record.reasons+='No independent watchdog report'}
    else {$watchdog=Read-Json $watchdogPath;if(!(False (Property $watchdog 'stalled')) -or (Property $watchdog 'hangs') -ne 0){$record.reasons+='Independent watchdog recorded a stall'}}
    $record.crashEvidence=Crash-Evidence $nativeRoot $project
    if(@($record.crashEvidence.reports).Count -gt 0 -or @($record.crashEvidence.dumps).Count -gt 0 -or @($record.crashEvidence.errors).Count -gt 0 -or @($record.crashEvidence.stderrPanicLines).Count -gt 0){$record.reasons+='Independent crash inspection found a fault or incomplete inventory'}
    if((Hash $originalJournal).sha256 -ne $originalJournalIdentity.sha256){throw 'Original journal changed during replay'}
    if((Hash $frozenJournal).sha256 -ne $originalJournalIdentity.sha256){throw 'Frozen input journal changed during replay'}
    $record.identityAfter=Same-Identity $identity
    $record.report=[ordered]@{artifact=if(Test-Path -LiteralPath $reportPath){Hash $reportPath}else{$null};
        accepted=(Property $report 'accepted');finished=(Property $report 'finished');actions=(Property $report 'actions');frames=(Property $report 'frames');
        panics=(Property $report 'panics');hangs=(Property $report 'hangs');panelFailures=(Property $report 'panelFailures');frameSpikes=(Property $report 'frameSpikes');
        offscreenGpuOutput=(Property $report 'offscreenGpuOutput');gpuSurfaceSubmissions=(Property $report 'gpuSurfaceSubmissions');nativePresentations=(Property $report 'nativePresentations');
        gpuOutputStalls=(Property $report 'gpuOutputStalls');gpuOutputHealth=(Property $report 'gpuOutputHealth');
        errorDiagnosticsBefore=(Property $report 'errorDiagnosticsBefore');errorDiagnosticsDuring=(Property $report 'errorDiagnosticsDuring');
        hostSeed=(Property $report 'hostSeed');hostSeedProvenance=(Property $report 'hostSeedProvenance');hostSeedMatchesReplay=(Property $report 'hostSeedMatchesReplay');
        replayOriginalHostSeedProvenance=(Property $report 'replayOriginalHostSeedProvenance');hostJobResources=(Property $report 'hostJobResources');playResources=(Property $report 'playResources');
        reportWriter=(Property $report 'reportWriter');journalRecorder=(Property $report 'journalRecorder');coverageComplete=(Property $report 'coverageComplete')}
    $record.completed=(True (Property $run 'completed'))
    $record.accepted=$record.reasons.Count -eq 0
} catch {$record.reasons+=$_.Exception.Message}
finally {$record.finishedUtc=[DateTime]::UtcNow.ToString('o');Json (Join-Path $output 'replay.json') $record}
Write-Output "Bounded native replay evidence: $output"
if(!$record.accepted){exit 1}
exit 0

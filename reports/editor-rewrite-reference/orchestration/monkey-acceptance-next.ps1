<#
Three serial shared-GPU correctness soaks by default. This script never builds, deletes
fixtures, kills processes, retries a failed seed, or modifies a supplied project. The
owner freezes and builds the checkpoint before launching this driver.

Example:
  powershell -NoProfile -File D:\AraBuild\ux-editor-target\orchestration\monkey-acceptance.ps1 `
    -CheckpointSHA <frozen-sha> -FixtureRoot <marked-scratch-project-on-D> `
    -BuildEvidence D:\AraBuild\ux-editor-target\orchestration\windows-build-<sha>.log

After the user opens the GPU slot, request exactly one formal timing seed:
  ... -EvidenceMode timing-exclusive -Seeds 104729

-RunBoundedBaseline adds a separate, sub-1800s run of the SAME binary. It is never
treated as before-fix evidence or as an accepted thirty-minute seed.
-BeforeMonkeyReport accepts existing historical monkey.json files; missing evidence
stays unknown. Prior raw reports and failures are never overwritten.
#>
param(
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-fA-F]{8,40}$')][string]$CheckpointSHA,
    [Parameter(Mandatory=$true)][string]$FixtureRoot,
    [string]$BuildEvidence,
    [string[]]$BeforeMonkeyReport=@(),
    [switch]$RunBoundedBaseline,
    [ValidateRange(1,1799)][int]$BaselineSeconds=60,
    [ValidateSet('correctness-shared','timing-exclusive')][string]$EvidenceMode='correctness-shared',
    [UInt64[]]$Seeds=@(104729,130363,155921)
)
Set-StrictMode -Version 2.0
$ErrorActionPreference='Stop'
$target='D:\AraBuild\ux-editor-target'
$source='D:\AraBuild\ux-editor-source'
$acceptance=Join-Path $target 'acceptance'
$runner=Join-Path $target 'orchestration\run-native.ps1'
$childLauncher=Join-Path $target 'orchestration\native-child.ps1'
$editor=Join-Path $target 'debug\ara-editor.exe'
$player=Join-Path $target 'debug\ara-player.exe'
$dev='D:\AraBuild\ux-editor-target\debug\ara-dev.exe'
$seconds=1800
$utf8=New-Object Text.UTF8Encoding($false)

function Property($Object,[string]$Name) {
    if($null -eq $Object){return $null}
    if($Object -is [Collections.IDictionary]) {if($Object.Contains($Name)){return $Object[$Name]};return $null}
    $property=$Object.PSObject.Properties[$Name]
    if($null -ne $property){return $property.Value}
    return $null
}
function True($Value){return $Value -is [bool] -and $Value}
function False($Value){return $Value -is [bool] -and !$Value}
function Is-Counter($Value) {
    if($Value -isnot [byte] -and $Value -isnot [sbyte] -and $Value -isnot [int16] -and $Value -isnot [uint16] -and $Value -isnot [int32] -and $Value -isnot [uint32] -and $Value -isnot [int64] -and $Value -isnot [uint64] -and $Value -isnot [single] -and $Value -isnot [double] -and $Value -isnot [decimal]){return $false}
    $number=[double]$Value
    return ![double]::IsNaN($number) -and ![double]::IsInfinity($number) -and $number -ge 0 -and [Math]::Floor($number) -eq $number
}
function Assess-Journal($Report) {
    $failures=New-Object 'Collections.Generic.List[string]'
    $journal=Property $Report 'journalRecorder'
    if($null -eq $journal){$failures.Add('Missing asynchronous journal recorder evidence');return @($failures)}
    if(!(True (Property $journal 'finalDrained'))){$failures.Add('Journal recorder did not finish draining')}
    foreach($key in @('error','pendingAction','abortedAction')) {
        if($null -eq $journal.PSObject.Properties[$key] -or $null -ne (Property $journal $key)){$failures.Add("Journal recorder does not rule out $key")}
    }
    foreach($key in @('errorCount','recordsNotQueued','metadataRecordsPending','metadataBytesPending')) {
        $value=Property $journal $key
        if(!(Is-Counter $value) -or $value -ne 0){$failures.Add("Journal recorder does not confirm zero $key")}
    }
    if(!(False (Property $journal 'queuedForSubmission'))){$failures.Add('Journal recorder still has a queued batch')}
    foreach($pair in @(@('recordsAttempted','recordsFlushed'),@('bytesAttempted','bytesFlushed'),@('lastTicket','completedTicket'))) {
        $expected=Property $journal $pair[0];$actual=Property $journal $pair[1]
        if(!(Is-Counter $expected) -or !(Is-Counter $actual) -or $expected -le 0 -or $expected -ne $actual){$failures.Add("Journal recorder has incomplete or unknown $($pair[0])/$($pair[1])")}
    }
    return @($failures)
}
function Assess-Request([string]$RequestedMode,[UInt64[]]$RequestedSeeds) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if(@($RequestedSeeds).Count -eq 0){$failures.Add('At least one seed is required')}
    if(@($RequestedSeeds|Select-Object -Unique).Count -ne @($RequestedSeeds).Count){$failures.Add('Seeds must be unique')}
    if($RequestedMode -notin @('correctness-shared','timing-exclusive')){$failures.Add('Unknown evidence mode')}
    if($RequestedMode -eq 'timing-exclusive' -and @($RequestedSeeds).Count -ne 1){$failures.Add('Exclusive timing requires exactly one explicit seed; the default three seeds are correctness-only')}
    return @($failures)
}
function Assess-EvidenceMode($Run,[string]$RequestedMode) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if((Property $Run 'requestedEvidenceMode') -ne $RequestedMode){$failures.Add('Production evidence mode does not match the requested class')}
    if($RequestedMode -eq 'correctness-shared') {
        if((Property $Run 'frameTimeStatus') -ne 'informal (shared GPU)' -or !(False (Property $Run 'timingAccepted'))){$failures.Add('Shared-GPU correctness must label frame times informal and cannot claim timing acceptance')}
        if(!(False (Property $Run 'gpuWrapperUsed')) -or (Property $Run 'processRole') -ne 'editor' -or [IO.Path]::GetFileName((Property $Run 'launchExecutable')) -ne 'ara-editor.exe'){$failures.Add('Shared-GPU correctness did not launch the editor directly')}
    } elseif($RequestedMode -eq 'timing-exclusive') {
        $arguments=@(Property $Run 'arguments')
        if(!(True (Property $Run 'gpuWrapperUsed')) -or (Property $Run 'processRole') -ne 'ara-dev gpu wrapper' -or [IO.Path]::GetFileName((Property $Run 'launchExecutable')) -ne 'ara-dev.exe' -or $arguments.Count -eq 0 -or $arguments[0] -ne 'gpu' -or $arguments -notcontains '--wait-for-quiet'){$failures.Add('Exclusive timing has no successful production GPU-admission wrapper')}
        if(!(True (Property $Run 'completed')) -or (Property $Run 'exitCode') -ne 0 -or !(True (Property $Run 'timingAccepted')) -or (Property $Run 'frameTimeStatus') -ne 'formal (exclusive GPU)'){$failures.Add('Exclusive timing did not complete successfully under the GPU wrapper')}
    } else {$failures.Add('Unknown requested evidence class')}
    return @($failures)
}
function Under([string]$Path,[string]$Root) {
    $pathFull=[IO.Path]::GetFullPath($Path).TrimEnd('\')
    $rootFull=[IO.Path]::GetFullPath($Root).TrimEnd('\')
    return $pathFull.StartsWith($rootFull+'\',[StringComparison]::OrdinalIgnoreCase)
}
function DPath([string]$Path) {
    $full=[IO.Path]::GetFullPath($Path)
    if(!($full -match '^[Dd]:\\') -or !(Under $full $target)){throw "Path must remain below the D: task target: $full"}
    return $full
}
function Json([string]$Path,$Value) {
    [IO.File]::WriteAllText((DPath $Path),($Value|ConvertTo-Json -Depth 40),$utf8)
}
function Read-Json([string]$Path){return Get-Content -Raw -Encoding UTF8 -LiteralPath $Path|ConvertFrom-Json}
function Hash([string]$Path) {
    $file=Get-Item -LiteralPath $Path
    if($file.PSIsContainer){throw "Expected a file: $Path"}
    return [ordered]@{path=$file.FullName;bytes=$file.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash;modifiedUtc=$file.LastWriteTimeUtc.ToString('o')}
}
function Read-Git([string[]]$Arguments) {
    $old=$env:GIT_OPTIONAL_LOCKS
    try {
        $env:GIT_OPTIONAL_LOCKS='0'
        $value=@(& git.exe -C $source @Arguments)
        if($LASTEXITCODE -ne 0){throw "Read-only git inspection failed: $Arguments"}
        return $value
    } finally {$env:GIT_OPTIONAL_LOCKS=$old}
}
function Identity {
    $sha=(Read-Git @('rev-parse','--verify','HEAD')|Select-Object -First 1).Trim()
    if(!$sha.StartsWith($CheckpointSHA,[StringComparison]::OrdinalIgnoreCase)){throw "Frozen source mismatch: requested $CheckpointSHA, actual $sha"}
    if(@(Read-Git @('status','--porcelain','--untracked-files=normal')).Count -ne 0){throw 'Frozen D: source has uncommitted changes'}
    $sourceFiles=@('crates\ara-editor\src\main.rs','crates\ara-editor\src\app\setup.rs','crates\ara-editor\src\app\mod.rs','crates\ara-editor\src\app\monkey.rs','crates\ara-editor\src\app\monkey_route.rs')|ForEach-Object{Join-Path $source $_}
    $sourceFiles+=@(Get-ChildItem -LiteralPath (Join-Path $source 'crates\ara-editor\src\app\monkey') -Filter '*.rs' -File -Recurse|ForEach-Object{$_.FullName})
    $sdkTools=@('Ara.CompileServer','Ara.Runtime','Ara.Editor','Ara.Generators','Ara.Analyzers')|ForEach-Object{Hash (Join-Path $target "sdk-tools/bin/Ara.CompileServer/release/$_.dll")}
    $sdkTools+=Hash (Join-Path $target 'sdk-tools/bin/Ara.Host/release/Ara.Host.dll')
    return [ordered]@{commit=$sha;tree=(Read-Git @('rev-parse','HEAD^{tree}')|Select-Object -First 1).Trim();sourceFiles=@($sourceFiles|Sort-Object|ForEach-Object{Hash $_});editor=(Hash $editor);player=(Hash $player);runner=(Hash $runner);childLauncher=(Hash $childLauncher);araDev=(Hash $dev);sdkTools=$sdkTools}
}
function Same-Identity($Expected) {
    $actual=Identity
    if($actual.commit -ne $Expected.commit -or $actual.tree -ne $Expected.tree){throw 'Source changed during the acceptance batch'}
    foreach($key in @('editor','player','runner','childLauncher','araDev')) {
        if($actual[$key].sha256 -ne $Expected[$key].sha256){throw "$key bytes changed during the acceptance batch"}
    }
    if(@(Compare-Object $Expected.sdkTools $actual.sdkTools -Property path,bytes,sha256).Count -gt 0){throw 'Prepared C# SDK tool bytes changed during the acceptance batch'}
    return $actual
}
function Inventory([string]$Root) {
    $rootFull=(DPath $Root).TrimEnd('\')
    $stack=New-Object 'Collections.Generic.Stack[string]'
    $stack.Push($rootFull)
    while($stack.Count -gt 0) {
        $directory=$stack.Pop()
        foreach($item in Get-ChildItem -LiteralPath $directory -Force) {
            if($directory -eq $rootFull -and $item.Name -in @('.ara','.git','.ara-profile','target','builds','.ara-monkey-fixture','.ara-behaviour-fixture')){continue}
            if(($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw "Fixture contains a symbolic link/reparse point: $($item.FullName)"}
            if(!(Under $item.FullName $rootFull)){throw 'Fixture inventory escaped its named root'}
            if($item.PSIsContainer){$stack.Push($item.FullName);continue}
            [pscustomobject][ordered]@{relative=$item.FullName.Substring($rootFull.Length+1).Replace('\','/');bytes=$item.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash}
        }
    }
}
function Quote-Argument([string]$Value) {
    # Windows argv quoting: preserve quotes and any backslashes before a quote/end.
    return '"'+(($Value -replace '(\\*)"','$1$1\"') -replace '(\\+)$','$1$1')+'"'
}
function Counts($Report,$Crashes=$null,$Watchdog=$null) {
    if($null -eq $Report -and $null -eq $Watchdog){return $null}
    # A stale UI checkpoint cannot erase a later independent watchdog stall.
    $knownHangs=@((Property $Report 'hangs'),(Property $Watchdog 'hangs'),(Property $Report 'gpuOutputStalls')|Where-Object{$null -ne $_})
    return [ordered]@{
        seconds=(Property $Report 'seconds');actions=(Property $Report 'actions');frames=(Property $Report 'frames')
        panics=if($null -ne $Report -and $null -ne $Report.PSObject.Properties['panics']){@($Report.panics).Count}else{$null}
        hangs=if($knownHangs.Count){($knownHangs|Measure-Object -Maximum).Maximum}else{$null}
        reportedHangs=(Property $Report 'hangs');watchdogHangs=(Property $Watchdog 'hangs');gpuOutputStalls=(Property $Report 'gpuOutputStalls')
        panelFailures=(Property $Report 'panelFailures');frameSpikes=(Property $Report 'frameSpikes')
        offscreenGpuOutput=(Property $Report 'offscreenGpuOutput');gpuSurfaceSubmissions=(Property $Report 'gpuSurfaceSubmissions');nativePresentations=(Property $Report 'nativePresentations');gpuOutputHealth=(Property $Report 'gpuOutputHealth')
        journalRecorder=(Property $Report 'journalRecorder')
        maxFrameMs=(Property $Report 'maxFrameMs');rejectedInputs=(Property $Report 'rejectedInputs');nativeEventsRouted=(Property $Report 'nativeEventsRouted')
        errorDiagnosticsBefore=(Property $Report 'errorDiagnosticsBefore');errorDiagnosticsDuring=(Property $Report 'errorDiagnosticsDuring')
        omittedDiagnostics=(Property $Report 'diagnosticRecordsOmitted');unvisitedCommands=(Property $Report 'unvisitedCommands')
        missingCommandEdgeCases=(Property $Report 'missingCommandEdgeCases');commandCallbacksAccepted=(Property $Report 'commandCallbacksAccepted')
        typedCommandCounts=(Property $Report 'typedCommandCounts');typedCommandJobs=(Property $Report 'typedCommandJobs')
        typedCommandCatalogErrors=(Property $Report 'typedCommandCatalogErrors');missingTypedCommandEdgeCases=(Property $Report 'missingTypedCommandEdgeCases')
        hostJobResources=(Property $Report 'hostJobResources')
        hostSeed=(Property $Report 'hostSeed');hostSeedProvenance=(Property $Report 'hostSeedProvenance')
        shellCommandCatalogFinalMatched=(Property $Report 'shellCommandCatalogFinalMatched');shellCommandCatalogSettled=(Property $Report 'shellCommandCatalogSettled')
        shellCommandCatalogFinal=(Property $Report 'shellCommandCatalogFinal');shellCommandCatalogErrors=(Property $Report 'shellCommandCatalogErrors')
        shellCommandCatalogFinalUnloaded=(Property $Report 'shellCommandCatalogFinalUnloaded');shellRegistrationCases=(Property $Report 'shellRegistrationCases')
        reportWriter=(Property $Report 'reportWriter')
        globalPanicArtifacts=if($null -ne $Crashes){Property $Crashes 'panicArtifacts'}else{$null}
        nativeCrashArtifacts=if($null -ne $Crashes){Property $Crashes 'nativeArtifacts'}else{$null}
        nativeDumpArtifacts=if($null -ne $Crashes){@(Property $Crashes 'dumps').Count}else{$null}
        managedCrashArtifacts=if($null -ne $Crashes){Property $Crashes 'managedArtifacts'}else{$null}
        stderrPanicLines=if($null -ne $Crashes){@(Property $Crashes 'stderrPanicLines').Count}else{$null}
    }
}
function Assess($Report,[UInt64]$Seed,[int]$Duration,[bool]$Soak,$Run,[string]$RequestedEvidenceMode='correctness-shared') {
    $failures=New-Object 'Collections.Generic.List[string]'
    foreach($failure in @(Assess-EvidenceMode $Run $RequestedEvidenceMode)){$failures.Add($failure)}
    if($null -eq $Run -or !(True (Property $Run 'completed')) -or (Property $Run 'exitCode') -ne 0){$failures.Add('Production runner did not complete with exit code zero')}
    if($null -eq $Report){$failures.Add('No parseable authoritative monkey.json');return @($failures)}
    foreach($failure in @(Assess-Journal $Report)){$failures.Add($failure)}
    foreach($key in @('seed','hostSeed','actions','frames','hangs','panelFailures','frameSpikes','nativeEventsRouted','diagnosticRecordsOmitted','gpuSurfaceSubmissions','nativePresentations','gpuOutputStalls')) {
        if(!(Is-Counter (Property $Report $key))){$failures.Add("Missing or invalid numeric counter: $key")}
    }
    foreach($key in @('actions','frames','nativeEventsRouted','gpuSurfaceSubmissions')) {
        if((Property $Report $key) -le 0){$failures.Add("No completed workload evidence: $key")}
    }
    foreach($key in @('seed','hostSeed','hostSeedProvenance','seconds','requestedSeconds','requestedSteps','panics','hangs','panelFailures','frameSpikes','nativeEventsRouted','diagnosticRecordsOmitted','endDrain','playResources','hostJobResources')) {
        if($null -eq $Report.PSObject.Properties[$key]){$failures.Add("Missing authoritative report field: $key")}
    }
    foreach($key in @('finished','accepted','finalDiagnosticsCollected')){if(!(True (Property $Report $key))){$failures.Add("Report does not confirm $key")}}
    foreach($key in @('cleanupIncomplete','diagnosticBusLoss','replayedJournal')){if(!(False (Property $Report $key))){$failures.Add("Report does not rule out $key")}}
    if((Property $Report 'seed') -ne $Seed){$failures.Add('Report seed differs from requested seed')}
    if((Property $Report 'hostSeed') -ne $Seed -or (Property $Report 'hostSeedProvenance') -ne 'startup'){$failures.Add('Real Host seed was not fixed to the requested seed at startup')}
    $writer=Property $Report 'reportWriter'
    if(!(True (Property $writer 'finalSnapshotRequested')) -or $null -eq $writer -or $null -eq $writer.PSObject.Properties['error'] -or $null -ne (Property $writer 'error')){$failures.Add('Final evidence writer status is missing or failed')}
    if(!(True (Property $Report 'offscreenGpuOutput')) -or (Property $Report 'gpuSurfaceSubmissions') -le 0 -or (Property $Report 'nativePresentations') -ne 0){$failures.Add('Offscreen run must submit real GPU output without native presentation')}
    $health=Property $Report 'gpuOutputHealth'
    foreach($key in @('stalls','unknownSamples')) {
        if(!(Is-Counter (Property $health $key))){$failures.Add("Missing or invalid GPU health counter: $key")}
    }
    if((Property $Report 'gpuOutputStalls') -ne 0 -or !(True (Property $health 'observed')) -or !(True (Property $health 'healthy')) -or (Property $health 'stalls') -ne 0 -or (Property $health 'unknownSamples') -ne 0){$failures.Add('Requested GPU output health is missing, unknown or stalled')}
    if((Property $Report 'requestedSeconds') -ne $Duration){$failures.Add('Report duration differs from requested duration')}
    if($null -ne (Property $Report 'requestedSteps')){$failures.Add('An action cap invalidates this duration-only run')}
    if((Property $Report 'seconds') -lt $Duration){$failures.Add('Reported elapsed duration is short')}
    if(@(Property $Report 'panics').Count -ne 0 -or (Property $Report 'hangs') -ne 0 -or (Property $Report 'panelFailures') -ne 0){$failures.Add('Panic, hang, or quarantined panel reported')}
    if((Property $Report 'diagnosticRecordsOmitted') -ne 0){$failures.Add('Diagnostic records were omitted')}
    $drain=Property $Report 'endDrain';$resources=Property $Report 'playResources'
    if(!(False (Property $drain 'cleanupIncomplete')) -or @(Property $drain 'heldKeys').Count -gt 0 -or @(Property $drain 'heldButtons').Count -gt 0){$failures.Add('Input/resource drain is incomplete')}
    if(@(Property $resources 'ownedActive').Count -gt 0 -or @(Property $resources 'ownedStopping').Count -gt 0){$failures.Add('Run-owned Play worlds remain active or stopping')}
    $hostJobs=Property $Report 'hostJobResources'
    foreach($key in @('baselineCollected','finalInventoryCollected','drained')){if(!(True (Property $hostJobs $key))){$failures.Add("Host job inventory does not confirm $key")}}
    foreach($key in @('hostUnsaved','fileFlushPending')){if(!(False (Property $hostJobs $key))){$failures.Add("Host job drain does not rule out $key")}}
    if(@(Property $hostJobs 'errors').Count -gt 0 -or !(False (Property (Property $hostJobs 'shell') 'busy'))){$failures.Add('Shell work or Host inventory errors remain')}
    if($Soak) {
        foreach($key in @('soakRequested','soakAccepted','durationCompleted','coverageComplete','shellCommandCoverageComplete','shellCommandCatalogFinalMatched','shellCommandCatalogSettled','typedCommandCatalogReady','typedCommandCoverageComplete','typedCommandCatalogFinalMatched')){if(!(True (Property $Report $key))){$failures.Add("Thirty-minute acceptance does not confirm $key")}}
        foreach($key in @('shellCommandCatalogObserved','shellCommandCatalogFinal','shellCommandCatalogErrors','shellCommandCatalogFinalUnloaded','shellRegistrationCases','missingCommandEdgeCases','unvisitedCommands')) {
            $field=$Report.PSObject.Properties[$key]
            if($null -eq $field -or $field.Value -isnot [Array]){$failures.Add("Missing array of shell-registration evidence: $key")}
        }
        if(@(Property $Report 'shellCommandCatalogFinal').Count -eq 0 -or @(Property $Report 'shellRegistrationCases').Count -eq 0){$failures.Add('Final shell catalog or per-registration coverage is empty')}
        if(@(Property $Report 'shellCommandCatalogErrors').Count -gt 0 -or @(Property $Report 'shellCommandCatalogFinalUnloaded').Count -gt 0){$failures.Add('Shell catalog discovery failed or final registrations remain unloaded')}
        $edgeCount=Property $Report 'edgeCasesPerCommand'
        if($edgeCount -isnot [int] -and $edgeCount -isnot [long]){$failures.Add('Missing integer shell edge-case count');$edgeCount=0}
        if($edgeCount -le 0){$failures.Add('Shell edge-case count must be positive')}
        $registrations=New-Object 'Collections.Generic.HashSet[string]'
        foreach($entry in @(Property $Report 'shellRegistrationCases')) {
            if($null -eq $entry){$failures.Add('Missing shell registration coverage entry');continue}
            $registration=Property $entry 'registration';$id=Property $registration 'id';$builtin=Property $registration 'builtin';$generation=Property $registration 'generation'
            if([string]::IsNullOrWhiteSpace($id) -or $builtin -isnot [bool] -or (!$builtin -and [string]::IsNullOrWhiteSpace($generation))){$failures.Add('Malformed shell registration identity')}
            $identity=@($id,$builtin,$generation)|ConvertTo-Json -Compress
            if(!$registrations.Add($identity)){$failures.Add('Repeated shell registration coverage entry')}
            $cases=Property $entry 'cases';$caseField=$entry.PSObject.Properties['cases']
            if($null -eq $caseField -or $caseField.Value -isnot [Array] -or @($cases).Count -ne $edgeCount -or @($cases|Select-Object -Unique).Count -ne $edgeCount){$failures.Add("Incomplete shell registration cases: $id")}
            foreach($caseNumber in @($cases)) {
                if(($caseNumber -isnot [int] -and $caseNumber -isnot [long]) -or $caseNumber -lt 0 -or $caseNumber -ge $edgeCount){$failures.Add("Invalid shell registration case: $id")}
            }
        }
        foreach($key in @('typedCommandCatalog','typedCommandCatalogErrors','missingTypedCommandEdgeCases','typedCommandCounts','typedCommandJobs')) {
            if($null -eq $Report.PSObject.Properties[$key]){$failures.Add("Missing typed-command evidence: $key")}
        }
        if(@(Property $Report 'typedCommandCatalog').Count -eq 0){$failures.Add('Typed command catalog is empty')}
        if(@(Property $Report 'typedCommandCatalogErrors').Count -gt 0 -or @(Property $Report 'missingTypedCommandEdgeCases').Count -gt 0){$failures.Add('Typed-command discovery or edge-case coverage is incomplete')}
        if(!(True (Property (Property $Report 'typedCommandJobs') 'drained'))){$failures.Add('Run-owned typed-command jobs were not drained')}
        if((Property $Report 'nativeEventsRouted') -le 0){$failures.Add('No actual events reached the native production router')}
        if(@(Property $Report 'unvisitedCommands').Count -gt 0 -or @(Property $Report 'missingCommandEdgeCases').Count -gt 0){$failures.Add('Registered command edge-case coverage is incomplete')}
        if((Property $Report 'termination') -ne 'elapsed-duration'){$failures.Add('Soak did not terminate by its duration budget')}
    } elseif(!(False (Property $Report 'soakRequested')) -or !(False (Property $Report 'soakAccepted'))) {
        $failures.Add('A short baseline incorrectly claims thirty-minute acceptance')
    }
    return @($failures)
}
function Crash-Evidence([string]$RunDirectory,[string]$Project) {
    $runRoot=DPath $RunDirectory;$projectRoot=DPath $Project
    $reports=New-Object 'Collections.Generic.List[object]';$dumps=New-Object 'Collections.Generic.List[object]';$errors=New-Object 'Collections.Generic.List[string]';$logs=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    $roots=@((Join-Path $runRoot 'crashes'),(Join-Path $runRoot 'captures\.ara-profile\crashes'),(Join-Path $projectRoot '.ara\play'),(Join-Path $projectRoot 'builds'))
    foreach($root in $roots) {
        if(!(Test-Path -LiteralPath $root)){continue}
        $pending=New-Object 'Collections.Generic.Stack[string]';$pending.Push($root)
        while($pending.Count -gt 0) {
            $directory=$pending.Pop()
            foreach($item in Get-ChildItem -Force -LiteralPath $directory) {
                if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){$errors.Add("Crash inventory excludes reparse point: $($item.FullName)");continue}
                if($item.PSIsContainer){$pending.Push($item.FullName);continue}
                if(!$seen.Add($item.FullName)){continue}
                if($item.Name -like '*.crash.json') {
                    try {
                        $record=Read-Json $item.FullName
                        $kind=Property $record 'kind'
                        if((Property $record 'format') -ne 'ara.crash/1' -or $kind -notin @('panic','native','managed')){throw 'Unknown crash report contract'}
                        $reports.Add([ordered]@{artifact=(Hash $item.FullName);kind=$kind;buildId=(Property $record 'buildId');message=(Property $record 'message')})
                    } catch {$errors.Add("Unreadable crash artifact $($item.FullName): $($_.Exception.Message)")}
                } elseif($item.Extension -ieq '.dmp') {$dumps.Add((Hash $item.FullName))} elseif($item.Extension -ieq '.log') {$null=$logs.Add($item.FullName)}
            }
        }
    }
    $panicLines=@();$stderr=Join-Path $runRoot 'stderr.log'
    if(Test-Path -LiteralPath $stderr) {$null=$logs.Add($stderr)} else {$errors.Add('No stderr log for independent panic inspection')}
    foreach($log in $logs) {
        $panicLines+=@(Select-String -Encoding UTF8 -LiteralPath $log -Pattern '^thread .+ panicked at|^thread panicked at|^fatal runtime error:|^Unhandled exception\.'|ForEach-Object{
            [ordered]@{path=$log;line=$_.LineNumber;text=$_.Line.Substring(0,[Math]::Min(2048,$_.Line.Length))}
        })
    }
    return [ordered]@{checked=$true;roots=$roots;reports=$reports.ToArray();dumps=$dumps.ToArray();errors=$errors.ToArray();stderrPanicLines=$panicLines;
        panicArtifacts=@($reports|Where-Object{$_.kind -eq 'panic'}).Count;nativeArtifacts=@($reports|Where-Object{$_.kind -eq 'native'}).Count;managedArtifacts=@($reports|Where-Object{$_.kind -eq 'managed'}).Count;
        note='Artifact and stderr counts overlap action panics and each other; they are not summed as unique faults. Backend worker/child retirement is not established by process exit alone.'}
}
function Assess-Crashes($Evidence) {
    $failures=New-Object 'Collections.Generic.List[string]'
    if(!(True (Property $Evidence 'checked'))){$failures.Add('No independent per-run crash inspection')}
    if(@(Property $Evidence 'errors').Count -gt 0){$failures.Add('Per-run crash inspection was incomplete')}
    if(@(Property $Evidence 'reports').Count -gt 0 -or @(Property $Evidence 'dumps').Count -gt 0 -or @(Property $Evidence 'stderrPanicLines').Count -gt 0){$failures.Add('Independent global panic/native/managed crash evidence was recorded')}
    return @($failures)
}
function Totals($Entries) {
    $reported=@($Entries|Where-Object{$null -ne $_.counts})
    if($reported.Count -eq 0){return $null}
    $total=[ordered]@{reports=$reported.Count;seconds=0.0;actions=0L;frames=0L;panics=0L;hangs=0L;panelFailures=0L;frameSpikes=0L;rejectedInputs=0L;nativeEventsRouted=0L;globalPanicArtifacts=0L;nativeCrashArtifacts=0L;nativeDumpArtifacts=0L;managedCrashArtifacts=0L;stderrPanicLines=0L;errorDiagnosticsBefore=@{};errorDiagnosticsDuring=@{}}
    foreach($entry in $reported) {
        foreach($key in @('seconds','actions','frames','panics','hangs','panelFailures','frameSpikes','rejectedInputs','nativeEventsRouted','globalPanicArtifacts','nativeCrashArtifacts','nativeDumpArtifacts','managedCrashArtifacts','stderrPanicLines')) {
            $value=$entry.counts[$key]
            if($null -eq $value){$total[$key]=$null}elseif($null -ne $total[$key]){$total[$key]+=$value}
        }
        foreach($key in @('errorDiagnosticsBefore','errorDiagnosticsDuring')) {
            $value=$entry.counts[$key]
            if($null -eq $value){$total[$key]=$null;continue}
            if($null -eq $total[$key]){continue}
            foreach($property in $value.PSObject.Properties) {
                if(!$total[$key].ContainsKey($property.Name)){$total[$key][$property.Name]=0L}
                $total[$key][$property.Name]+=[long]$property.Value
            }
        }
    }
    return $total
}

$requestFailures=@(Assess-Request $EvidenceMode $Seeds)
if($requestFailures.Count -gt 0){throw ($requestFailures -join '; ')}
if($env:OS -ne 'Windows_NT'){throw 'This driver targets the frozen Windows native binaries'}
$FixtureRoot=DPath $FixtureRoot
if(!(Under $FixtureRoot $acceptance)){throw 'Use a known scratch project below D: acceptance, never a real project'}
if(!(Test-Path -LiteralPath (Join-Path $FixtureRoot '.ara-monkey-fixture')) -and !(Test-Path -LiteralPath (Join-Path $FixtureRoot '.ara-behaviour-fixture'))){throw 'Source fixture lacks an explicit isolated-fixture marker'}
$marker=Read-Json (Join-Path $FixtureRoot 'ara.project.json')
$inventory=@(Inventory $FixtureRoot|Sort-Object relative)
if($inventory.Count -lt 2){throw 'The scratch source must contain an authored graph, not only a project marker'}
$identity=Identity
$short=(Read-Git @('rev-parse','--short=8','HEAD')|Select-Object -First 1).Trim()
$batch=DPath (Join-Path $acceptance "monkey-$short-$(Get-Date -Format yyyyMMdd-HHmmss)-$([Guid]::NewGuid().ToString('N').Substring(0,8))")
New-Item -ItemType Directory -Path $batch|Out-Null
$fixtureBytes=($inventory|Measure-Object -Property bytes -Sum).Sum
$batchRecord=[ordered]@{
    startedUtc=[DateTime]::UtcNow.ToString('o');completed=$false;accepted=$false;output=$batch;requestedEvidenceMode=$EvidenceMode;correctnessAccepted=$false;timingAccepted=$false
    frameTimeStatus=if($EvidenceMode -eq 'correctness-shared'){'informal (shared GPU)'}else{'unverified (exclusive GPU requested)'}
    requested=[ordered]@{evidenceMode=$EvidenceMode;seeds=$Seeds;secondsPerSeed=$seconds;serial=$true;runBoundedBaseline=[bool]$RunBoundedBaseline;baselineSeconds=if($RunBoundedBaseline){$BaselineSeconds}else{$null}}
    identity=$identity;driver=(Hash $PSCommandPath);buildEvidence=if($BuildEvidence){Hash (DPath $BuildEvidence)}else{$null}
    binarySourceBinding='Source commit/tree and executable bytes are recorded independently. The owner must review the matching build evidence; hashes alone do not prove which source produced a binary.'
    fixture=[ordered]@{source=$FixtureRoot;marker=$marker;files=$inventory;bytes=$fixtureBytes;excludedRoot=@('.ara','.git','.ara-profile','target','builds','fixture ownership markers');copiedWithoutHistory=$true;layoutMigrated=$false}
    freeBytesAtStart=(New-Object IO.DriveInfo('D:\')).AvailableFreeSpace
    historicalBefore=@();boundedBaseline=$null;soaks=@();stopScheduling=$false;error=$null
    notes=@('Acceptance requires authoritative monkey.json fields, independent crash-artifact/stderr inspection and completed zero-exit production processes. run.json.accepted is not maintained by run-native.ps1.',
        'Job terminal states and process exit do not prove retirement of every detached backend worker or external child. No universal backend-retirement claim is made.',
        'Frame spikes and error diagnostics are reported, not silently zeroed. Expected malformed-input rejections are separate from crashes.',
        'Shared-GPU frame times are informal (shared GPU); correctness acceptance imposes no FPS or frame-spike timing gate. Formal timing requires the successful exclusive ara-dev gpu wrapper.',
        'Coverage means attempted registered-command edge targets. Accepted command callbacks are reported separately; rejected/disabled paths are not implementation success.',
        'errorDiagnosticsBefore describes each run before its first action, not a historical before-fix crash baseline.',
        'A runner or wrapper timeout stops scheduling. No process is killed; recorded exact PID/start time requires owner inspection before any later GPU work.',
        'In timing-exclusive mode gpu --timeout 300 applies to GPU admission, not the 1800-second editor workload. Both launch modes allow duration+600 seconds.')
}
function Save-Batch {
    $batchRecord.afterCounts=Totals $batchRecord.soaks
    $batchRecord.beforeCounts=Totals $batchRecord.historicalBefore
    $batchRecord.counts=[ordered]@{requestedSoaks=@($Seeds).Count;attemptedSoaks=@($batchRecord.soaks).Count;authoritativeReports=@($batchRecord.soaks|Where-Object{$_.reportParsed}).Count;completedSoaks=@($batchRecord.soaks|Where-Object{$null -ne $_.process -and (True (Property ([pscustomobject]$_.process) 'completed')) -and $null -ne $_.authoritativeAcceptance -and (True (Property ([pscustomobject]$_.authoritativeAcceptance) 'finished'))}).Count;acceptedSoaks=@($batchRecord.soaks|Where-Object{$_.accepted}).Count}
    $batchRecord.accepted=$batchRecord.counts.acceptedSoaks -eq @($Seeds).Count -and !$batchRecord.stopScheduling
    $batchRecord.correctnessAccepted=$batchRecord.accepted -and $EvidenceMode -eq 'correctness-shared'
    $batchRecord.timingAccepted=$batchRecord.accepted -and $EvidenceMode -eq 'timing-exclusive'
    if($batchRecord.timingAccepted){$batchRecord.frameTimeStatus='formal (exclusive GPU)'}
    Json (Join-Path $batch 'monkey-acceptance.json') $batchRecord
}
function Invoke-Run([string]$Label,[UInt64]$Seed,[int]$Duration,[bool]$Soak) {
    $directory=DPath (Join-Path $batch $Label);New-Item -ItemType Directory -Path $directory|Out-Null
    $project=DPath (Join-Path $directory 'project');New-Item -ItemType Directory -Path $project|Out-Null
    $entry=[ordered]@{label=$Label;seed=$Seed;requestedSeconds=$Duration;soak=$Soak;accepted=$false;requestedEvidenceMode=$EvidenceMode;frameTimeStatus=if($EvidenceMode -eq 'correctness-shared'){'informal (shared GPU)'}else{'unverified (exclusive GPU requested)'};correctnessAccepted=$false;timingAccepted=$false;reportParsed=$false;counts=$null;project=$project;process=$null;productionRun=$null;authoritativeAcceptance=$null;raw=@();reasons=@();crashEvidence=$null}
    try {
        $null=Same-Identity $identity
        $fresh=@(Inventory $FixtureRoot|Sort-Object relative)
        if((Compare-Object $inventory $fresh -Property relative,bytes,sha256|Measure-Object).Count -ne 0){throw 'The known source fixture changed after its manifest was captured'}
        foreach($file in $inventory) {
            $destination=DPath (Join-Path $project $file.relative)
            if(!(Under $destination $project)){throw 'Fixture path escaped the per-seed project'}
            [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))|Out-Null
            [IO.File]::Copy((Join-Path $FixtureRoot $file.relative),$destination,$false)
        }
        $copied=@(Inventory $project|Sort-Object relative)
        if((Compare-Object $inventory $copied -Property relative,bytes,sha256|Measure-Object).Count -ne 0){throw 'Fresh per-seed fixture bytes differ from the frozen inventory'}
        [IO.File]::WriteAllText((Join-Path $project '.ara-monkey-fixture'),"Ara isolated serial monkey fixture $Label`n",$utf8)
        Json (Join-Path $directory 'fixture.json') ([ordered]@{source=$FixtureRoot;project=$project;files=$inventory;historyCopied=$false})
        foreach($key in @('ARA_CONFIG_DIR','ARA_CRASH_DIR','DOTNET_CLI_HOME','TEMP','TMP','APPDATA','LOCALAPPDATA','XDG_CACHE_HOME','XDG_CONFIG_HOME')) {
            $path=DPath (Join-Path $directory (Join-Path 'launcher-profile' $key));[IO.Directory]::CreateDirectory($path)|Out-Null
            [Environment]::SetEnvironmentVariable($key,$path,'Process')
        }
        $env:ARA_PLAY_OFFSCREEN='1';$env:CARGO_TARGET_DIR=$target;$env:CARGO_BUILD_BUILD_DIR=$target;$env:CARGO_BUILD_JOBS='3'
        [Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
        $shellExe=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        $arguments=@('-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',$runner,'-CheckpointSHA',$short,'-Mode','monkey','-Project',$project,'-Seed',[string]$Seed,'-Seconds',[string]$Duration,'-EvidenceMode',$EvidenceMode)
        $argumentLine=($arguments|ForEach-Object{Quote-Argument $_}) -join ' '
        $process=Start-Process -FilePath $shellExe -ArgumentList $argumentLine -WorkingDirectory $source -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $directory 'launcher.stdout.log') -RedirectStandardError (Join-Path $directory 'launcher.stderr.log')
        $null=$process.Handle
        $entry.process=[ordered]@{pid=$process.Id;startedUtc=$process.StartTime.ToUniversalTime().ToString('o');executable=$shellExe;arguments=$arguments;completed=$false;exitCode=$null;timeoutSeconds=$Duration+720}
        Json (Join-Path $directory 'process.json') $entry.process
        Write-Host "${Label}: owned wrapper PID $($process.Id); waiting for the production runner"
        # Exact owned handle, once. No name-based waiting, sleeps, polling or termination.
        $completed=$process.WaitForExit(($Duration+720)*1000)
        $entry.process.completed=$completed
        if(!$completed){$batchRecord.stopScheduling=$true;throw 'Owned wrapper timed out; it was not terminated'}
        $process.WaitForExit();$process.Refresh();$entry.process.exitCode=$process.ExitCode;$entry.process.finishedUtc=[DateTime]::UtcNow.ToString('o')
        Json (Join-Path $directory 'process.json') $entry.process
        $matches=@(Get-ChildItem -LiteralPath (Join-Path $acceptance $short) -Directory|ForEach-Object {
            $path=Join-Path $_.FullName 'run.json'
            if(Test-Path -LiteralPath $path){$run=Read-Json $path;if(@($run.arguments) -contains $project){[pscustomobject]@{path=$path;run=$run}}}
        })
        if($matches.Count -ne 1){$batchRecord.stopScheduling=$true;throw 'Could not uniquely identify this project-owned production run manifest'}
        $run=$matches[0].run;$entry.productionRun=$matches[0].path
        if(!(True (Property $run 'completed')) -or (True (Property $run 'timeout'))){$batchRecord.stopScheduling=$true}
        $nativeRoot=DPath (Split-Path -Parent $matches[0].path)
        $raw=DPath (Join-Path $directory 'raw');New-Item -ItemType Directory -Path $raw|Out-Null
        foreach($relative in @('run.json','stdout.log','stderr.log','captures\monkey.json','captures\monkey-events.jsonl','captures\monkey-watchdog.json','captures\monkey-typed-catalog.json')) {
            $from=DPath (Join-Path $nativeRoot $relative)
            if(Test-Path -LiteralPath $from) {
                $to=Join-Path $raw ([IO.Path]::GetFileName($relative));[IO.File]::Copy($from,$to,$false)
                $entry.raw+=Hash $to
            }
        }
        $reportPath=Join-Path $raw 'monkey.json';$report=$null
        if(Test-Path -LiteralPath $reportPath){$report=Read-Json $reportPath;$entry.reportParsed=$null -ne $report}
        $watchdogPath=Join-Path $raw 'monkey-watchdog.json';$watchdog=$null
        if(Test-Path -LiteralPath $watchdogPath){$watchdog=Read-Json $watchdogPath}
        $entry.crashEvidence=Crash-Evidence $nativeRoot $project
        Json (Join-Path $directory 'crash-evidence.json') $entry.crashEvidence
        $entry.counts=Counts $report $entry.crashEvidence $watchdog
        $entry.reasons=@(Assess $report $Seed $Duration $Soak $run $EvidenceMode)
        $entry.reasons+=@(Assess-Crashes $entry.crashEvidence)
        if($entry.process.exitCode -ne 0){$entry.reasons+='Production wrapper did not exit with code zero'}
        if(!(Test-Path -LiteralPath $watchdogPath)){$entry.reasons+='No authoritative watchdog report'}
        else {$watchdog=Read-Json $watchdogPath;if(!(False (Property $watchdog 'stalled')) -or (Property $watchdog 'hangs') -ne 0){$entry.reasons+='Independent watchdog reports a stall'}}
        if(!(Test-Path -LiteralPath (Join-Path $raw 'monkey-events.jsonl'))){$entry.reasons+='No durable action journal'}
        $entry.identityAfter=Same-Identity $identity
        $entry.accepted=$entry.reasons.Count -eq 0
        $entry.correctnessAccepted=$entry.accepted -and $EvidenceMode -eq 'correctness-shared'
        $entry.timingAccepted=$entry.accepted -and $EvidenceMode -eq 'timing-exclusive'
        if($entry.timingAccepted){$entry.frameTimeStatus='formal (exclusive GPU)'}
        $entry.authoritativeAcceptance=[ordered]@{accepted=(Property $report 'accepted');soakAccepted=(Property $report 'soakAccepted');soakRequested=(Property $report 'soakRequested');finished=(Property $report 'finished')}
    } catch {
        $entry.reasons+=$_.Exception.Message
        if($null -ne $entry.process -and !(True (Property ([pscustomobject]$entry.process) 'completed'))){$batchRecord.stopScheduling=$true}
        if($_.Exception.Message -match 'changed|mismatch'){$batchRecord.stopScheduling=$true}
    }
    Json (Join-Path $directory 'result.json') $entry
    return $entry
}

try {
    foreach($path in $BeforeMonkeyReport) {
        $path=DPath $path;$report=Read-Json $path
        $batchRecord.historicalBefore+=,[ordered]@{report=(Hash $path);counts=(Counts $report);accepted=(Property $report 'accepted');soakAccepted=(Property $report 'soakAccepted');historical=$true}
    }
    Save-Batch
    if($RunBoundedBaseline) {
        $batchRecord.boundedBaseline=Invoke-Run 'bounded-baseline' $Seeds[0] $BaselineSeconds $false
        if(!$batchRecord.boundedBaseline.accepted){$batchRecord.stopScheduling=$true}
        Save-Batch
    }
    foreach($seed in $Seeds) {
        if($batchRecord.stopScheduling){break}
        $batchRecord.soaks+=,(Invoke-Run "soak-$seed" $seed $seconds $true)
        Save-Batch
    }
    Save-Batch
    $batchRecord.completed=$batchRecord.counts.completedSoaks -eq @($Seeds).Count -and !$batchRecord.stopScheduling
} catch {$batchRecord.error=$_.Exception.Message;$batchRecord.stopScheduling=$true}
finally {$batchRecord.finishedUtc=[DateTime]::UtcNow.ToString('o');Save-Batch}
Write-Output "Monkey acceptance evidence: $batch"
if(!$batchRecord.accepted){exit 1}
exit 0

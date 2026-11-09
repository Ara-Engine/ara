#requires -Version 7.0
<#
.SYNOPSIS
Sequential preview-4 acceptance. This file is an orchestration artifact, not a gate result.
.DESCRIPTION
Review first, then invoke with absolute prebuilt executable paths and a frozen D: source snapshot.
No executable is built merely by loading this file with -PlanOnly. Normal execution compiles
focused tests through the supplied ara-dev. All stages reuse one stable D: Cargo target/build
directory; only profiles, temporary files and evidence outputs are fresh for each stage.
Default/-NativeOnly runs Windows native acceptance; -AllGates adds CPU tests, clippy and goldens
that normally belong to the single Linux m6u owner stream. Do not run those streams concurrently.
PowerShell 7 is required for ProcessStartInfo.ArgumentList and ConvertFrom-Json -AsHashtable;
this file does not claim Windows PowerShell 5 runtime compatibility.
Native routes use hidden native windows, never --headless as an editor-input substitute.
The existing journey itself intentionally runs its exported game headlessly.

On a timeout this script records owned PID/start-time pairs and stops scheduling work. It does
not terminate any process. Review owned-processes.json before stopping an exact PID yourself.
No golden update opt-in is propagated. No files are deleted and no git stash/push is used.

Fresh-copy fallback: samples/outpost is copied file-for-file into a new D: directory, excluding
only .git/.ara/target/builds/bin/obj/evidence and reparse points. Journey always uses that copy
because its source requires levels/hangar.scene. Docking/monkeys prefer a sibling ara.exe init;
if absent, they use a separately recorded fresh sample copy, never the source project itself.
.EXAMPLE
pwsh -File D:\AraBuild\ux-editor-target\orchestration\run-acceptance.ps1 `
  -AraDevExe D:\AraBuild\bin\ara-dev.exe -EditorExe D:\AraBuild\bin\ara-editor.exe `
  -SourceRootD D:\AraBuild\source-snapshot -OutputRootD D:\AraBuild\ux-editor-target\acceptance `
  -CheckpointSHA 979314c4 -PlanOnly
#>
[CmdletBinding(DefaultParameterSetName='NativeOnly')]
param(
    [Parameter(Mandatory)][string]$AraDevExe,
    [Parameter(Mandatory)][string]$EditorExe,
    [Parameter(Mandatory)][string]$SourceRootD,
    [Parameter(Mandatory)][string]$OutputRootD,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-fA-F]{7,40}$')][string]$CheckpointSHA,
    [string]$CargoTargetDir = 'D:\AraBuild\ux-editor-target',
    [string]$CargoBuildDir,
    [string]$CargoHomeD,
    [Parameter(ParameterSetName='NativeOnly')][switch]$NativeOnly,
    [Parameter(ParameterSetName='AllGates',Mandatory)][switch]$AllGates,
    [string]$AraCliExe,
    [string]$JourneyTemplateD,
    [string]$FreshProjectTemplateD,
    [UInt64[]]$MonkeySeeds = @(104729, 2789, 64019),
    [ValidateRange(1800,86400)][int]$MonkeySeconds = 1800,
    [ValidateRange(300,43200)][int]$CargoTimeoutSeconds = 7200,
    [ValidateRange(300,43200)][int]$NativeTimeoutSeconds = 3600,
    [ValidateRange(300,43200)][int]$JourneyTimeoutSeconds = 7200,
    [switch]$PlanOnly
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-DPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetPathRoot($absolute) -ine 'D:\') { throw "Path must stay on D: $absolute" }
    if ($absolute.TrimEnd('\') -eq 'D:') { throw 'Use a named directory under D:, not the drive root' }
    $inspect = $absolute
    while ($inspect) {
        if (Test-Path -LiteralPath $inspect) {
            $item = Get-Item -LiteralPath $inspect -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse path refused; D: outputs may not redirect to C: or another owner: $inspect"
            }
        }
        $parent = [IO.Path]::GetDirectoryName($inspect)
        if ($parent -eq $inspect) { break }
        $inspect = $parent
    }
    return $absolute.TrimEnd('\')
}
function New-DirectoryD([string]$Path) {
    $resolved = Assert-DPath $Path
    [void][IO.Directory]::CreateDirectory($resolved)
    return $resolved
}
function Write-Json([string]$Path, $Value) {
    $resolved = Assert-DPath $Path
    [IO.File]::WriteAllText($resolved, (($Value | ConvertTo-Json -Depth 100) + "`n"), [Text.UTF8Encoding]::new($false))
}
function Read-Json([string]$Path) {
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    try { return Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable }
    catch { return @{ parseError = $_.Exception.Message; path = $Path } }
}
function Copy-FreshProject([string]$Template, [string]$Destination) {
    $source = Assert-DPath $Template
    $destinationPath = Assert-DPath $Destination
    if ($destinationPath -ieq $source -or $destinationPath.StartsWith($source+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Fresh-copy destination must be outside its template' }
    if (!(Test-Path -LiteralPath (Join-Path $source 'ara.project.json') -PathType Leaf)) { throw "Not a project template: $source" }
    if (Test-Path -LiteralPath $destinationPath) { throw "Fresh project already exists: $destinationPath" }
    [void](New-DirectoryD $destinationPath)
    $queue = [Collections.Generic.Queue[object]]::new()
    $queue.Enqueue(@($source,$destinationPath))
    while ($queue.Count) {
        $pair = $queue.Dequeue()
        foreach ($item in Get-ChildItem -LiteralPath $pair[0] -Force) {
            if ($item.Name -in @('.git','.ara','target','builds','bin','obj','evidence')) { continue }
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Template link refused: $($item.FullName)" }
            $to = Assert-DPath (Join-Path $pair[1] $item.Name)
            if ($item.PSIsContainer) {
                [void](New-DirectoryD $to)
                $queue.Enqueue(@($item.FullName,$to))
            } else { Copy-Item -LiteralPath $item.FullName -Destination $to }
        }
    }
    return @{ method='fresh-copy'; source=$source; destination=$destinationPath; excluded=@('.git','.ara','target','builds','bin','obj','evidence'); authoringSourceUntouched=$true }
}
function New-StageEnvironment([string]$Directory) {
    $map = @{}
    foreach ($pair in @{
        ARA_CONFIG_DIR='config'; ARA_CRASH_DIR='crashes'; DOTNET_CLI_HOME='dotnet';
        TEMP='temp'; TMP='temp'; CARGO_TARGET_TMPDIR='temp';
        APPDATA='appdata'; NUGET_PACKAGES='nuget'; XDG_CACHE_HOME='cache';
        XDG_CONFIG_HOME='config'
    }.GetEnumerator()) { $map[$pair.Key] = New-DirectoryD (Join-Path $Directory $pair.Value) }
    # Reuse the parent's warm build artifacts; never create per-stage build caches.
    $map.CARGO_TARGET_DIR = $CargoTargetDir
    $map.CARGO_BUILD_TARGET_DIR = $CargoTargetDir
    $map.CARGO_BUILD_BUILD_DIR = $CargoBuildDir
    $map.ARA_SHARED_BUILD_DIR = $CargoBuildDir
    # Existing Cargo home is inherited read-warm/offline unless explicitly given a D: home.
    # Native-only execution does not create a second Cargo download cache.
    if ($CargoHomeD) { $map.CARGO_HOME = $CargoHomeD }
    $map.CARGO_NET_OFFLINE = 'true'
    $map.CARGO_TERM_COLOR = 'never'
    $map.LOCALAPPDATA = New-DirectoryD 'D:\AraBuild\ux-editor-target\orchestration\service-local'
    $map.CARGO_BUILD_JOBS = '3'
    $map.RUSTC_WRAPPER = ''
    $map.ARA_PLAY_OFFSCREEN = '1'
    $map.ARA_EDITOR_MEMO_CHECK = '1'
    $map.DOTNET_NOLOGO = '1'
    $map.DOTNET_CLI_TELEMETRY_OPTOUT = '1'
    $map.DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
    $map.RUSTUP_AUTO_INSTALL = '0'
    $map.ARA_EDITOR_QUALITY_CAPTURE_DIR = New-DirectoryD (Join-Path $Directory 'quality-captures')
    return $map
}
function Observe-OwnedProcesses([Collections.Generic.Dictionary[int,object]]$Owned, [DateTime]$Started) {
    # Only descendants of PID/start-time identities already observed from our launched root.
    foreach ($entry in @($Owned.Values)) {
        try {
            $parentProcess = [Diagnostics.Process]::GetProcessById($entry.pid)
            if ($parentProcess.StartTime.ToUniversalTime().Ticks -ne $entry.startTicks) { continue }
            foreach ($child in Get-CimInstance Win32_Process -Filter "ParentProcessId = $($entry.pid)" -ErrorAction Stop) {
                $childId = [int]$child.ProcessId
                $process = [Diagnostics.Process]::GetProcessById($childId)
                $birth = $process.StartTime.ToUniversalTime()
                if ($birth -lt $Started.AddSeconds(-1)) { continue }
                if (!$Owned.ContainsKey($childId)) {
                    $Owned[$childId] = @{ pid=$childId; parent=$entry.pid; startUtc=$birth.ToString('o'); startTicks=$birth.Ticks; executable=$child.ExecutablePath; commandLine=$child.CommandLine }
                    try { $process.PriorityClass = [Diagnostics.ProcessPriorityClass]::BelowNormal } catch { $Owned[$childId].priorityError=$_.Exception.Message }
                }
            }
        } catch { } # A process may exit between enumeration and identity validation.
    }
}
function Invoke-Owned([string]$Name,[string]$Executable,[string[]]$Arguments,[string]$Directory,[hashtable]$Environment,[int]$Timeout) {
    $stdout = Join-Path $Directory "$Name.stdout.log"
    $stderr = Join-Path $Directory "$Name.stderr.log"
    $result = [ordered]@{ name=$Name; executable=$Executable; arguments=$Arguments; cwd=$SourceRootD; requested=$true; started=$false; completed=$false; exitCode=$null; timeoutSeconds=$Timeout; timedOut=$false; stdout=$stdout; stderr=$stderr; ownedProcesses=@(); environment=$Environment }
    $started = [DateTime]::UtcNow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $owned = [Collections.Generic.Dictionary[int,object]]::new()
    $process = [Diagnostics.Process]::new()
    $outFile = $null; $errFile = $null
    try {
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $Executable; $start.WorkingDirectory = $SourceRootD
        $start.UseShellExecute = $false; $start.CreateNoWindow = $true
        $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
        $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
        foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
        foreach ($entry in $Environment.GetEnumerator()) { $start.Environment[$entry.Key] = [string]$entry.Value }
        foreach ($key in @($start.Environment.Keys)) {
            if ($key -match '(?i)(UPDATE.*GOLDEN|GOLDEN.*UPDATE)' -or $key -eq 'ARA_DEV_GPU_GUARD_FIXTURE') { [void]$start.Environment.Remove($key) }
        }
        $process.StartInfo = $start
        if (!$process.Start()) { throw "Could not start $Executable" }
        $result.started=$true; $result.pid=$process.Id; $result.startedUtc=$started.ToString('o')
        $birth=$process.StartTime.ToUniversalTime()
        $owned[$process.Id]=@{pid=$process.Id;parent=$PID;startUtc=$birth.ToString('o');startTicks=$birth.Ticks;executable=$Executable;commandLine=$Arguments}
        try { $process.PriorityClass = [Diagnostics.ProcessPriorityClass]::BelowNormal; $result.priorityApplied=$true } catch { $result.priorityError=$_.Exception.Message; $script:StopScheduling=$true }
        $outFile=[IO.File]::Create($stdout); $errFile=[IO.File]::Create($stderr)
        $outTask=$process.StandardOutput.BaseStream.CopyToAsync($outFile)
        $errTask=$process.StandardError.BaseStream.CopyToAsync($errFile)
        Observe-OwnedProcesses $owned $started
        $nextObservation=0.0
        while (!$process.WaitForExit(1000)) {
            if ($watch.Elapsed.TotalSeconds -ge $nextObservation) {
                Observe-OwnedProcesses $owned $started
                $result.ownedProcesses=@($owned.Values)
                Write-Json (Join-Path $Directory 'owned-processes.json') $result.ownedProcesses
                Write-Json (Join-Path $Directory "$Name.running.json") $result
                $nextObservation=$watch.Elapsed.TotalSeconds+10
            }
            if ($watch.Elapsed.TotalSeconds -ge $Timeout) { $result.timedOut=$true; $script:StopScheduling=$true; break }
        }
        Observe-OwnedProcesses $owned $started
        if (!$result.timedOut) {
            $result.exitCode=$process.ExitCode; $result.completed=$true
            # No indefinite wait for an inherited pipe held by an orphaned worker.
            if (![Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($outTask,$errTask),30000)) {
                $result.pipeDrainIncomplete=$true; $script:StopScheduling=$true
            }
            $remaining=@()
            foreach($identity in $owned.Values){
                try{
                    $stillLive=[Diagnostics.Process]::GetProcessById($identity.pid)
                    if(!$stillLive.HasExited -and $stillLive.StartTime.ToUniversalTime().Ticks -eq $identity.startTicks){$remaining+=,$identity}
                }catch{}
            }
            if($remaining.Count){$result.remainingOwnedProcesses=$remaining;$script:StopScheduling=$true}
        } else {
            $result.note='Timed out. No process was stopped. Do not launch another acceptance stage until the recorded PID/start-time identities are reviewed.'
            # Keep handles alive until this runner returns; partial logs are explicitly incomplete.
            $script:LiveHandles.Add(@($process,$outFile,$errFile,$outTask,$errTask))
        }
    } catch {
        $result.error=$_.Exception.Message
        if ($result.started -and !$process.HasExited) { $script:StopScheduling=$true }
    } finally {
        $watch.Stop(); $result.durationSeconds=$watch.Elapsed.TotalSeconds
        $result.endedUtc=[DateTime]::UtcNow.ToString('o'); $result.ownedProcesses=@($owned.Values)
        Write-Json (Join-Path $Directory 'owned-processes.json') $result.ownedProcesses
        Write-Json (Join-Path $Directory "$Name.process.json") $result
        if ($result.completed -and !$result.Contains('pipeDrainIncomplete')) {
            if ($null -ne $outFile) { $outFile.Dispose() }; if ($null -ne $errFile) { $errFile.Dispose() }
            $process.Dispose()
        }
    }
    return $result
}
function Read-StageEvidence($Stage,[string]$Directory) {
    $capture=Join-Path $Directory 'captures'
    $result=@{ completed=$false; accepted=$false; reports=@(); passed=0; failed=0; note=$null }
    $files=@(switch($Stage.kind) {
        'behaviours' { @('behaviours.json','navigation.json','selection.json','snapping.json','native-input.json','prefab.json','diagnostics.json','play-behaviours.json') }
        'docking' { @('drags.json') }
        'journey' { @('journey.json') }
        'monkey' { @('monkey.json') }
        'quality' { @('capture-index.json','quality-measurements.json') }
        default { @() }
    })
    if ($Stage.kind -eq 'quality') { $capture=Join-Path $Directory 'quality-captures' }
    $allComplete=$true; $allAccepted=$true
    foreach ($file in $files) {
        $path=Join-Path $capture $file; $doc=Read-Json $path
        $entry=@{path=$path;present=$null -ne $doc;completed=$false;accepted=$false;passed=0;failed=0}
        if ($null -ne $doc -and !$doc.ContainsKey('parseError')) {
            if ($Stage.kind -eq 'monkey') {
                $entry.completed=$doc.finished -eq $true -and [double]$doc.requestedSeconds -ge $MonkeySeconds -and [double]$doc.seconds -ge $MonkeySeconds -and $null -eq $doc.requestedSteps
                $entry.accepted=$entry.completed -and $doc.accepted -eq $true -and $doc.coverageComplete -eq $true -and !$doc.cleanupIncomplete
                $entry.counts=@{actions=$doc.actions;frames=$doc.frames;panics=@($doc.panics).Count;hangs=$doc.hangs;panelFailures=$doc.panelFailures;frameSpikes=$doc.frameSpikes;errorDiagnosticsBefore=$doc.errorDiagnosticsBefore;errorDiagnosticsDuring=$doc.errorDiagnosticsDuring;unvisitedCommands=$doc.unvisitedCommands;requestedSeconds=$doc.requestedSeconds;completedSeconds=$doc.seconds}
                $entry.finishedReported=$doc.finished; $entry.seed=$doc.seed
            } elseif ($Stage.kind -eq 'quality') {
                if ($file -eq 'capture-index.json') { $entry.passed=@($doc.captures).Count; $entry.completed=$entry.passed -gt 0; $entry.accepted=$entry.completed }
                else { $entry.failed=@($doc.violations).Count; $entry.passed=@($doc.overlays).Count; $entry.completed=$entry.passed -gt 100; $entry.accepted=$entry.completed -and $entry.failed -eq 0 }
            } else {
                $rows=if ($doc.ContainsKey('drags')) {@($doc.drags)} elseif ($doc.ContainsKey('steps')) {@($doc.steps)} elseif ($doc.ContainsKey('journeys')) {@($doc.journeys)} elseif ($doc.ContainsKey('behaviours')) {@($doc.behaviours)} else {@()}
                $entry.passed=@($rows | Where-Object {$_.ok -eq $true}).Count
                $entry.failed=@($rows | Where-Object {$_.ok -ne $true}).Count
                $entry.completed=if($doc.ContainsKey('completed')){$doc.completed -eq $true}elseif($doc.ContainsKey('finished')){$doc.finished -eq $true}else{$Stage.kind -eq 'docking' -and $rows.Count -gt 0}
                $entry.accepted=$entry.completed -and $entry.failed -eq 0 -and $entry.passed -gt 0
            }
        }
        $allComplete=$allComplete -and $entry.completed; $allAccepted=$allAccepted -and $entry.accepted
        $result.passed+=$entry.passed; $result.failed+=$entry.failed; $result.reports+=,$entry
    }
    $result.completed=$files.Count -gt 0 -and $allComplete
    $result.accepted=$files.Count -gt 0 -and $allAccepted
    return $result
}

$SourceRootD=Assert-DPath $SourceRootD; $OutputRootD=New-DirectoryD $OutputRootD
$CargoTargetDir=New-DirectoryD $CargoTargetDir
if(!$CargoBuildDir){$CargoBuildDir=$CargoTargetDir}
$CargoBuildDir=New-DirectoryD $CargoBuildDir
if($CargoHomeD){$CargoHomeD=New-DirectoryD $CargoHomeD}
if (!(Test-Path -LiteralPath (Join-Path $SourceRootD 'Cargo.toml'))) { throw 'SourceRootD is not the frozen Ara workspace' }
if ($OutputRootD -ieq $SourceRootD -or $OutputRootD.StartsWith($SourceRootD+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'OutputRootD must be independent of the source snapshot' }
foreach ($exe in @($AraDevExe,$EditorExe)) { if (![IO.Path]::IsPathRooted($exe) -or !(Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Absolute prebuilt executable missing: $exe" } }
if ($MonkeySeeds.Count -ne 3 -or @($MonkeySeeds | Select-Object -Unique).Count -ne 3) { throw 'Exactly three distinct monkey seeds are required' }
if (!$AraCliExe) { $candidate=Join-Path (Split-Path -Parent $EditorExe) 'ara.exe'; if(Test-Path -LiteralPath $candidate -PathType Leaf){$AraCliExe=$candidate} }
if ($AraCliExe -and (![IO.Path]::IsPathRooted($AraCliExe) -or !(Test-Path -LiteralPath $AraCliExe -PathType Leaf))) { throw "Explicit absolute CLI missing: $AraCliExe" }
if (!$JourneyTemplateD) { $JourneyTemplateD=Join-Path $SourceRootD 'samples/outpost' }
if (!$FreshProjectTemplateD) { $FreshProjectTemplateD=$JourneyTemplateD }
$JourneyTemplateD=Assert-DPath $JourneyTemplateD; $FreshProjectTemplateD=Assert-DPath $FreshProjectTemplateD
$RunRoot=New-DirectoryD (Join-Path $OutputRootD ("$CheckpointSHA-"+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')))
$script:RunRoot=$RunRoot; $script:StopScheduling=$false; $script:LiveHandles=[Collections.Generic.List[object]]::new()
$stages=@(
    @{name='strict-clippy';kind='clippy';gpu=$false;timeout=$CargoTimeoutSeconds;args=@('cargo','--','clippy','-p','ara-editor','-p','ara-ui','-p','ara-ui-document','-p','ara-ui-text','--all-targets','--','-D','warnings')},
    @{name='unit-editor-kit';kind='cargo';gpu=$false;timeout=$CargoTimeoutSeconds;args=@('cargo','--','test','-p','ara-editor','-p','ara-ui','-p','ara-ui-document','-p','ara-ui-text','--lib','--','--test-threads=1')},
    @{name='kit-goldens';kind='cargo';gpu=$false;timeout=$CargoTimeoutSeconds;args=@('cargo','--','test','-p','ara-ui-test','--test','suite','--','--test-threads=1')},
    @{name='diag-coverage';kind='cargo';gpu=$false;timeout=$CargoTimeoutSeconds;args=@('cargo','--','test','-p','ara-diag','--test','coverage','--','--test-threads=1')},
    @{name='full-editor-quality-captures';kind='quality';gpu=$true;timeout=$CargoTimeoutSeconds;args=@('cargo','--','test','-p','ara-editor','--lib','shell::quality_tests::captures::native_full_editor_overlay_quality_captures','--','--exact','--ignored','--nocapture','--test-threads=1')},
    @{name='native-behaviours';kind='behaviours';gpu=$true;timeout=$NativeTimeoutSeconds},
    @{name='native-docking';kind='docking';gpu=$true;timeout=$NativeTimeoutSeconds},
    @{name='native-journey';kind='journey';gpu=$true;timeout=$JourneyTimeoutSeconds}
)
if(!$AllGates){$stages=@($stages|Where-Object {$_.gpu})}
foreach ($seed in $MonkeySeeds) { $stages+=@{name="monkey-$seed";kind='monkey';gpu=$true;timeout=($MonkeySeconds+600);seed=$seed} }
$manifest=[ordered]@{
    checkpointClaim=$CheckpointSHA
    binaryCheckpointVerification='operator-supplied checkpoint claim; executable SHA256 recorded, not inferred from version strings'
    source=$SourceRootD; runRoot=$RunRoot; createdUtc=[DateTime]::UtcNow.ToString('o'); planOnly=[bool]$PlanOnly
    mode=if($AllGates){'all-gates'}else{'native-only'}
    cpuGatePolicy=if($AllGates){'Windows focused CPU gates explicitly requested; do not overlap the Linux owner stream'}else{'CPU package/clippy/story gates not requested here; their Linux m6u checkpoint evidence must be reviewed separately'}
    buildRoots=@{cargoTarget=$CargoTargetDir;cargoBuild=$CargoBuildDir;cargoHome=if($CargoHomeD){$CargoHomeD}else{'inherited existing warm home; offline Cargo; no new duplicate cache'};sharedAcrossStages=$true}
    beforeEvidence=@{path='D:\AraBuild\ux-round-final';exists=(Test-Path -LiteralPath 'D:\AraBuild\ux-round-final')}
    executables=@{araDev=@{path=$AraDevExe;sha256=(Get-FileHash -LiteralPath $AraDevExe -Algorithm SHA256).Hash};editor=@{path=$EditorExe;sha256=(Get-FileHash -LiteralPath $EditorExe -Algorithm SHA256).Hash};cli=@{path=$AraCliExe;available=[bool]$AraCliExe}}
    requested=@{stages=$stages.Count;monkeyRuns=3;secondsPerSeed=$MonkeySeconds;totalMonkeySeconds=3*$MonkeySeconds;seeds=$MonkeySeeds}
    stages=$stages;results=@();completed=$false;accepted=$false
    limitations=@(
        'Offscreen native correctness, not visible IME candidate-window or OS clipboard-service proof.',
        'Frame-spike/timing values are valid only when ara-dev GPU monitor reports valid; correctness reports are kept even if monitoring is invalid.',
        'Foreign-load guard observes other GPU users; D-local queue does not claim admission coordination with callers using another LOCALAPPDATA.',
        'A timeout never proves a crash-free run and stops further scheduling.',
        'Target/build artifacts are reused on D:. Only profiles/temp/evidence are fresh. Offline Cargo fails truthfully if the warm dependency cache is incomplete.',
        'No golden updates, recursive deletion or process termination. Acceptance covers the requested scope; native-only does not substitute for the Linux CPU gates.'
    )
}
Write-Json (Join-Path $RunRoot 'acceptance-plan.json') $manifest
if ($PlanOnly) { Write-Output (Join-Path $RunRoot 'acceptance-plan.json'); return }

foreach ($stage in $stages) {
    $directory=New-DirectoryD (Join-Path $RunRoot $stage.name)
    if ($script:StopScheduling) { $manifest.results+=@{name=$stage.name;state='not-run-after-unresolved-process';requested=$true;completed=$false;accepted=$false}; continue }
    $environment=New-StageEnvironment $directory
    $captures=New-DirectoryD (Join-Path $directory 'captures')
    $entry=[ordered]@{name=$stage.name;kind=$stage.kind;requested=$true;completed=$false;accepted=$false;state='preparing';directory=$directory}
    try {
        $arguments=if($stage.ContainsKey('args')){[string[]]$stage.args}else{[string[]]@()}
        if ($stage.kind -in @('behaviours','docking','journey','monkey')) {
            $project=Join-Path $directory 'project'
            if ($stage.kind -eq 'behaviours') { [void](New-DirectoryD $project); $entry.fixture=@{method='editor-owned isolated behaviour-project fixture';suppliedPathIgnored=$project} }
            elseif ($stage.kind -eq 'journey') {
                $entry.fixture=Copy-FreshProject $JourneyTemplateD $project
                if(!(Test-Path -LiteralPath (Join-Path $project 'levels/hangar.scene'))){throw 'Journey copy lacks its required levels/hangar.scene'}
            } elseif ($AraCliExe) {
                $entry.fixture=Invoke-Owned 'init' $AraCliExe @('init',$project,'--name',"Acceptance $($stage.name)",'--json') $directory $environment 180
                if(!$entry.fixture.completed -or $entry.fixture.exitCode -ne 0){throw 'Typed ara init did not complete successfully'}
            } else { $entry.fixture=Copy-FreshProject $FreshProjectTemplateD $project; $entry.fixture.cliUnavailable=$true }
            $flag=switch($stage.kind){'behaviours'{'--verify-behaviours'}'docking'{'--verify-docking'}'journey'{'--verify-journey'}'monkey'{'--verify-monkey'}}
            $arguments=@($EditorExe,$project,$flag,'--sample','--output',$captures,'--size','1440x900')
            if($stage.kind -eq 'monkey'){$arguments+=@('--monkey-seed',[string]$stage.seed,'--monkey-seconds',[string]$MonkeySeconds)}
        }
        # Captures, native routes and soak frame timings always use the GPU admission/monitor wrapper.
        if($stage.kind -eq 'quality'){$arguments=@($AraDevExe)+$arguments}
        if($stage.gpu){$arguments=@('gpu','--timeout','300','--')+$arguments}
        $entry.process=Invoke-Owned 'run' $AraDevExe $arguments $directory $environment $stage.timeout
        if($stage.kind -in @('behaviours','docking','journey','monkey')){$entry.effectiveNativeProfile=Join-Path $captures '.ara-profile'}
        $log=@((Get-Content -LiteralPath $entry.process.stdout -Raw -ErrorAction SilentlyContinue),(Get-Content -LiteralPath $entry.process.stderr -Raw -ErrorAction SilentlyContinue)) -join "`n"
        $entry.gpuMonitor=@{required=[bool]$stage.gpu;status='not-required';timingsValid=$false;report=$null}
        if($stage.gpu){
            $entry.gpuMonitor.status='missing-or-unavailable'
            foreach($line in ($log -split "`r?`n")){
                if($line -match '^ARA_DEV_GPU_READING\s+(\{.*\})$'){
                    try{$reading=$Matches[1]|ConvertFrom-Json -AsHashtable;$entry.gpuMonitor.status=$reading.status;$entry.gpuMonitor.report=$reading.report;$entry.gpuMonitor.timingsValid=$reading.status -eq 'valid'}catch{$entry.gpuMonitor.parseError=$_.Exception.Message}
                }
            }
        }
        $entry.evidence=Read-StageEvidence $stage $directory
        if($stage.kind -eq 'cargo'){
            $summaries=[regex]::Matches($log,'test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;')
            $passed=0;$failed=0;$ignored=0
            foreach($summary in $summaries){$passed+=[int]$summary.Groups[2].Value;$failed+=[int]$summary.Groups[3].Value;$ignored+=[int]$summary.Groups[4].Value}
            $entry.evidence=@{completed=$entry.process.completed -and $summaries.Count -gt 0;accepted=$summaries.Count -gt 0 -and $passed -gt 0 -and $failed -eq 0;passed=$passed;failed=$failed;ignored=$ignored;testSummaryCount=$summaries.Count}
        }
        if($stage.kind -eq 'clippy'){
            $entry.evidence=@{completed=$entry.process.completed;accepted=$entry.process.completed -and $entry.process.exitCode -eq 0;passed=0;failed=[int]($entry.process.exitCode -ne 0);note='Compiler exit status for explicit clippy -D warnings; this is not a test-count claim'}
        }
        $entry.completed=$entry.process.completed -and $entry.evidence.completed -and !$entry.process.Contains('error') -and !$entry.process.Contains('priorityError') -and !$entry.process.Contains('pipeDrainIncomplete') -and !$entry.process.Contains('remainingOwnedProcesses')
        $entry.accepted=$entry.completed -and $entry.process.exitCode -eq 0 -and $entry.evidence.accepted -and (!$stage.gpu -or $entry.gpuMonitor.timingsValid)
        $entry.state=if($entry.process.timedOut){'timed-out-process-left-running'}elseif($entry.accepted){'accepted'}elseif(!$entry.process.completed){'could-not-complete'}elseif(!$entry.evidence.completed){'missing-or-partial-evidence'}elseif($stage.gpu -and !$entry.gpuMonitor.timingsValid){'invalid-or-unavailable-gpu-monitor'}else{'failed'}
    } catch { $entry.error=$_.Exception.Message; $entry.state='setup-or-prerequisite-failure' }
    $manifest.results+=,$entry
    $manifest.completed=@($manifest.results|Where-Object {$_.completed}).Count -eq $stages.Count
    $manifest.accepted=@($manifest.results|Where-Object {$_.accepted}).Count -eq $stages.Count
    $manifest.counts=@{requested=$stages.Count;attempted=$manifest.results.Count;completed=@($manifest.results|Where-Object {$_.completed}).Count;accepted=@($manifest.results|Where-Object {$_.accepted}).Count;failedOrIncomplete=@($manifest.results|Where-Object {!$_.accepted}).Count;monkeyRequested=3;monkeyCompleted=@($manifest.results|Where-Object {$_.Contains('kind') -and $_.kind -eq 'monkey' -and $_.completed}).Count}
    Write-Json (Join-Path $RunRoot 'acceptance-summary.json') $manifest
}
$manifest.endedUtc=[DateTime]::UtcNow.ToString('o')
$manifest.counts=@{requested=$stages.Count;attempted=@($manifest.results|Where-Object {$_.state -ne 'not-run-after-unresolved-process'}).Count;completed=@($manifest.results|Where-Object {$_.completed}).Count;accepted=@($manifest.results|Where-Object {$_.accepted}).Count;failedOrIncomplete=@($manifest.results|Where-Object {!$_.accepted}).Count;notRun=@($manifest.results|Where-Object {$_.state -eq 'not-run-after-unresolved-process'}).Count;monkeyRequested=3;monkeyCompleted=@($manifest.results|Where-Object {$_.Contains('kind') -and $_.kind -eq 'monkey' -and $_.completed}).Count}
$monkeyReports=@($manifest.results|Where-Object {$_.Contains('kind') -and $_.kind -eq 'monkey' -and $_.Contains('evidence')}|ForEach-Object {$_.evidence.reports}|Where-Object {$_.Contains('counts')})
$manifest.monkeyTotals=@{requestedRuns=3;reportsReceived=$monkeyReports.Count;unreportedRuns=3-$monkeyReports.Count;requestedSeconds=3*$MonkeySeconds;reportedSeconds=0.0;reportedActions=0;reportedFrames=0;reportedPanics=0;reportedHangs=0;reportedPanelFailures=0;reportedFrameSpikes=0;errorDiagnosticsBefore=@{};errorDiagnosticsDuring=@{};historicalBeforeCounts='Not invented: historical evidence is linked separately in beforeEvidence'}
foreach($report in $monkeyReports){
    $manifest.monkeyTotals.reportedSeconds+=[double]$report.counts.completedSeconds
    foreach($pair in @{reportedActions='actions';reportedFrames='frames';reportedPanics='panics';reportedHangs='hangs';reportedPanelFailures='panelFailures';reportedFrameSpikes='frameSpikes'}.GetEnumerator()){$manifest.monkeyTotals[$pair.Key]+=[long]$report.counts[$pair.Value]}
    foreach($field in @('errorDiagnosticsBefore','errorDiagnosticsDuring')){
        foreach($pair in $report.counts[$field].GetEnumerator()){
            if(!$manifest.monkeyTotals[$field].ContainsKey($pair.Key)){$manifest.monkeyTotals[$field][$pair.Key]=0L}
            $manifest.monkeyTotals[$field][$pair.Key]+=[long]$pair.Value
        }
    }
}
Write-Json (Join-Path $RunRoot 'acceptance-summary.json') $manifest
Write-Output (Join-Path $RunRoot 'acceptance-summary.json')
if(!$manifest.accepted){exit 1}

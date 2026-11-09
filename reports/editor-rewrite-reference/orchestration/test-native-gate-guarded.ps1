# Synthetic wrapper tests only: no GPU, editor, process launch or termination.
Set-StrictMode -Version 2.0
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'run-native-gate-guarded.ps1')
$script:launches=0;$script:stops=0
function Start-Process { $script:launches++;throw 'Tests must never launch a process' }
function Stop-GateOwnedTree { $script:stops++;throw 'Tests must never stop a process' }
$cases=New-Object 'Collections.Generic.List[object]'
function Check([string]$Name,[scriptblock]$Assert){&$Assert;$cases.Add([pscustomobject]@{case=$Name;passed=$true})}
function Require([bool]$Condition,[string]$Message){if(!$Condition){throw $Message}}
$sha='a'*40
$prefix="Running ara-dev.exe check --base abcdef12 --json`nBase: $sha (merge-base)`nCompilation: disabled by flag; CARGO_INCREMENTAL=0`n"
$prepared=$prefix+"tests: 62 prepared harnesses, 4 workers`n"
$blocked=Parse-GateLog $prepared
$clear=Parse-GateLog $prefix
Check 'wrapper parses as PowerShell without syntax errors' {$tokens=$null;$errors=$null;[void][Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'run-native-gate-guarded.ps1'),[ref]$tokens,[ref]$errors);Require ($errors.Count -eq 0) 'Parse errors'}
foreach($phase in @('before-launch','running','completion')){
    $captured=$phase
    Check "recognized compilation permits $captured" {Require ((Get-NativeGateDisposition $clear $captured) -eq 'continue') 'Compilation was blocked'}
}
Check 'prepared harnesses block launch' {Require ((Get-NativeGateDisposition $blocked 'before-launch') -eq 'block') 'Prepared gate launched'}
Check 'prepared harnesses discard running evidence' {Require ((Get-NativeGateDisposition $blocked 'running') -eq 'discard') 'Running evidence retained'}
Check 'prepared harnesses at final boundary discard evidence' {Require ((Get-NativeGateDisposition $blocked 'completion') -eq 'discard') 'Final gate race retained evidence'}
Check 'unknown completion fails closed' {Require ((Get-NativeGateDisposition (Parse-GateLog 'unrecognized') 'completion') -eq 'discard') 'Unknown completion authorized'}
Check 'missing observation fails closed' {Require ((Get-NativeGateDisposition $null 'before-launch') -eq 'block') 'Missing gate authorized'}
Check 'monkey is not accepted by this wrapper' {$rejected=$false;try{$null=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d6 -Mode monkey}catch{$rejected=$true};Require $rejected 'Monkey was accepted'}
Check 'formal timing mode is not exposed' {$rejected=$false;try{$null=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d6 -EvidenceMode timing-exclusive}catch{$rejected=$true};Require $rejected 'Formal timing was accepted'}
Check 'wrong checkpoint length rejects before launch' {$rejected=$false;try{$null=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d61}catch{$rejected=$true};Require $rejected 'Wrong source ID shape accepted'}
Check 'evidence cannot escape D acceptance root' {$rejected=$false;try{$null=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d6 -EvidenceRoot C:/Temp}catch{$rejected=$true};Require $rejected 'Evidence escaped'}
$fixture='D:/AraBuild/ux-editor-target/acceptance/native-guard-tests-'+[Guid]::NewGuid().ToString('N')
$gates=Join-Path $fixture 'gates';[IO.Directory]::CreateDirectory($gates)|Out-Null
[IO.File]::WriteAllText((Join-Path $gates 'gate-b999.log'),$prepared,[Text.Encoding]::UTF8)
Check 'real wrapper blocks prepared fixture without launch or stop' {
    $result=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d6 -Mode behaviours -GateDirectory $gates -EvidenceRoot $fixture
    Require ($result.blockedBeforeStart -and !$result.acceptanceEligible -and !$result.guardPassed -and !$result.accepted) 'Blocked evidence was eligible'
    Require ($result.wrapperExitCode -eq 3 -and $null -eq $result.process) 'Blocked run has a process or wrong exit'
    Require (!$result.automaticRetry -and !$result.timingAccepted) 'Retry or timing claim introduced'
    $saved=Get-Content -LiteralPath (Join-Path $result.directory 'guard.json') -Encoding UTF8 -Raw|ConvertFrom-Json
    Require ($saved.blockedBeforeStart -and $null -ne $saved.finishedUtc -and $saved.requestedEvidenceMode -eq 'correctness-queued') 'Saved block record incomplete'
}
Check 'real wrapper blocks missing fixture gate without launch or stop' {
    $missing=Join-Path $fixture 'empty';[IO.Directory]::CreateDirectory($missing)|Out-Null
    $result=Invoke-NativeGateGuarded -CheckpointSHA 5b4c39d6 -Mode docking -GateDirectory $missing -EvidenceRoot $fixture
    Require ($result.blockedBeforeStart -and !$result.acceptanceEligible -and $result.wrapperExitCode -eq 3) 'Unknown gate launched'
    Require ($null -eq $result.process -and !$result.automaticRetry) 'Unknown gate created work/retry'
}
Check 'no process launch or termination occurred' {Require ($script:launches -eq 0 -and $script:stops -eq 0) 'Synthetic test touched processes'}
[ordered]@{passed=$true;count=$cases.Count;cases=$cases.ToArray();fixture=$fixture;processLaunches=$script:launches;processStops=$script:stops}|ConvertTo-Json -Depth 6
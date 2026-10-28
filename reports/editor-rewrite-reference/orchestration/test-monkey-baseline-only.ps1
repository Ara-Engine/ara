# Bounded scheduling/metadata checks. Driver startup, GPU work and native processes
# are never invoked; only parsed pure helpers and its real final scheduling block run.
Set-StrictMode -Version 2.0
$ErrorActionPreference='Stop'
$driver=Join-Path $PSScriptRoot 'monkey-acceptance.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($driver,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
foreach($name in @('Property','True','False','Assess-Request','Get-SoakSeeds','Totals','Save-Batch','Get-BatchExitCode')){
    $fn=$ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)|Where-Object{$_.Name -eq $name}|Select-Object -First 1
    if($null -eq $fn){throw "Missing helper $name"}
    Invoke-Expression $fn.Extent.Text
}
$mainTry=$ast.EndBlock.Statements|Where-Object{$_ -is [Management.Automation.Language.TryStatementAst]}|Select-Object -Last 1
if($null -eq $mainTry){throw 'Missing final scheduling block'}
$script:mainSchedule=[scriptblock]::Create($mainTry.Extent.Text)
$text=[IO.File]::ReadAllText($driver)
$start=$text.IndexOf('$requestFailures=@(Assess-Request')
$end=$text.IndexOf("if(`$env:OS -ne 'Windows_NT')",$start)
if($start -lt 0 -or $end -le $start){throw 'Cannot isolate request validation'}
$script:requestValidation=[scriptblock]::Create($text.Substring($start,$end-$start))
$cases=New-Object 'Collections.Generic.List[object]'
function Require([bool]$Condition,[string]$Message){if(!$Condition){throw $Message}}
function Check([string]$Name,[scriptblock]$Assert){&$Assert;$cases.Add([pscustomobject]@{case=$Name;passed=$true})}
function Json([string]$Path,$Value){$script:saved+=,($Value|ConvertTo-Json -Depth 16)}
function Start-Process {throw 'Tests must never launch a process'}
function Stop-GateOwnedTree {throw 'Tests must never stop a process'}
function Invoke-Run([string]$Label,[UInt64]$Seed,[int]$Duration,[bool]$Soak){
    $script:calls+=,[pscustomobject]@{label=$Label;seed=$Seed;duration=$Duration;soak=$Soak}
    $accepted=if($Soak){$script:failSeed -ne $Seed}else{$script:baselinePass}
    return [ordered]@{accepted=$accepted;discarded=(!$Soak -and $script:baselineDiscarded);reportParsed=$true;counts=$null;blockedBeforeStart=$false;process=@{completed=$true};authoritativeAcceptance=@{finished=$true}}
}
function Reset([bool]$Only,[bool]$WithBaseline,[string]$Mode='correctness-shared'){
    $script:BaselineOnly=$Only;$script:RunBoundedBaseline=$WithBaseline
    [UInt64[]]$script:Seeds=@(104729,130363,155921)
    $script:BaselineSeconds=60;$script:seconds=1800;$script:EvidenceMode=$Mode;$script:QuietSlotGranted=$false
    $script:BeforeMonkeyReport=@();$script:batch='D:/AraBuild/ux-editor-target/orchestration/monkey-baseline-only-test-state'
    $script:soakSeeds=Get-SoakSeeds $script:Seeds $Only
    $script:baselinePass=$true;$script:baselineDiscarded=$false;$script:failSeed=0
    $script:calls=@();$script:saved=@()
    $script:batchRecord=[ordered]@{historicalBefore=@();soaks=@();boundedBaseline=$null;stopScheduling=$false;error=$null;completed=$false;accepted=$false;baselineAccepted=$false;correctnessAccepted=$false;timingAccepted=$false;frameTimeStatus=if($Mode -eq 'timing-exclusive'){'unverified (exclusive GPU requested)'}else{'informal (shared GPU)'}}
}
Check 'BaselineOnly is an explicit switch parameter' {
    $parameter=$ast.ParamBlock.Parameters|Where-Object{$_.Name.VariablePath.UserPath -eq 'BaselineOnly'}
    Require ($null -ne $parameter -and $parameter.StaticType -eq [Management.Automation.SwitchParameter]) 'Missing explicit switch'
}
Check 'BaselineOnly requires the bounded baseline request' {Require (@(Assess-Request correctness-shared @(104729,130363,155921) $true $false).Count -gt 0) 'Only mode silently lacked its baseline'}
Check 'explicit bounded-only default seed request is valid' {Require (@(Assess-Request correctness-shared @(104729,130363,155921) $true $true).Count -eq 0) 'Bounded-only request rejected'}
Check 'empty seeds remain invalid in bounded-only mode' {Require (@(Assess-Request correctness-shared @() $true $true).Count -gt 0) 'Empty seeds bypassed validation'}
Check 'duplicate seeds remain invalid in bounded-only mode' {Require (@(Assess-Request correctness-shared @(104729,104729) $true $true).Count -gt 0) 'Duplicate seeds bypassed validation'}
Check 'normal shared three-seed request remains valid' {Require (@(Assess-Request correctness-shared @(104729,130363,155921)).Count -eq 0) 'Normal default changed'}
Check 'exclusive timing still rejects default three seeds' {Require (@(Assess-Request timing-exclusive @(104729,130363,155921)).Count -gt 0) 'Exclusive default restriction relaxed'}
Check 'exclusive timing one seed remains valid' {Require (@(Assess-Request timing-exclusive @(104729)).Count -eq 0) 'Explicit timing request changed'}
Check 'bounded-only has no requested long seeds' {Require ((Get-SoakSeeds @(104729,130363,155921) $true).Count -eq 0) 'Long seeds were scheduled'}
Check 'normal default seed identities and order remain unchanged' {Require (((Get-SoakSeeds @(104729,130363,155921) $false) -join ',') -eq '104729,130363,155921') 'Default identity/order changed'}
Check 'actual validation rejects missing RunBoundedBaseline before any scheduling' {Reset $true $false;$rejected=$false;try{&$script:requestValidation}catch{$rejected=$true};Require ($rejected -and $script:calls.Count -eq 0) 'Missing dependency allowed work'}
Check 'actual exclusive validation still requires a granted quiet slot' {Reset $false $false timing-exclusive;$script:Seeds=@([UInt64]104729);$rejected=$false;try{&$script:requestValidation}catch{$rejected=$true};Require ($rejected -and $script:calls.Count -eq 0) 'Quiet-slot requirement relaxed'}
Check 'actual exclusive validation accepts its explicit granted slot' {Reset $false $false timing-exclusive;$script:Seeds=@([UInt64]104729);$script:QuietSlotGranted=$true;. $script:requestValidation;Require ($soakSeeds.Count -eq 1) 'Granted timing request changed'}
Check 'bounded-only executes exactly one sixty-second non-soak baseline' {Reset $true $true;&$script:mainSchedule;Require ($script:calls.Count -eq 1 -and $script:calls[0].label -eq 'bounded-baseline' -and !$script:calls[0].soak -and $script:calls[0].duration -eq 60 -and $script:calls[0].seed -eq 104729) 'Unexpected schedule';Require ($null -eq $script:batchRecord.error) 'Unexpected schedule error'}
Check 'successful bounded-only result records zero requested and accepted long soaks' {Require ($script:batchRecord.counts.requestedSoaks -eq 0 -and $script:batchRecord.counts.attemptedSoaks -eq 0 -and $script:batchRecord.counts.acceptedSoaks -eq 0 -and $script:batchRecord.counts.completedSoaks -eq 0) 'Bounded baseline inflated long evidence'}
Check 'successful bounded-only result is never vacuous full acceptance' {Require ($script:batchRecord.baselineAccepted -and $script:batchRecord.completed -and !$script:batchRecord.accepted -and !$script:batchRecord.correctnessAccepted -and !$script:batchRecord.timingAccepted) 'Vacuous or misclassified acceptance'}
Check 'successful bounded-only exit is zero despite full acceptance remaining false' {Require ((Get-BatchExitCode $true $script:batchRecord) -eq 0) 'Passing bounded entry returned failure'}
Check 'bounded-only success never enters long after-count totals' {Require ($null -eq $script:batchRecord.afterCounts -and $script:batchRecord.soaks.Count -eq 0) 'Baseline mixed into long aggregates'}
Check 'failed baseline schedules no long soaks and exits failure' {Reset $true $true;$script:baselinePass=$false;&$script:mainSchedule;Require ($script:calls.Count -eq 1 -and !$script:batchRecord.baselineAccepted -and !$script:batchRecord.accepted -and !$script:batchRecord.completed -and (Get-BatchExitCode $true $script:batchRecord) -eq 1) 'Failed baseline escaped its boundary'}
Check 'discarded baseline can never certify bounded success' {Reset $true $true;$script:baselineDiscarded=$true;&$script:mainSchedule;Require (!$script:batchRecord.baselineAccepted -and (Get-BatchExitCode $true $script:batchRecord) -eq 1 -and $script:calls.Count -eq 1) 'Discarded baseline accepted'}
Check 'normal default still executes all three long seeds without baseline' {Reset $false $false;&$script:mainSchedule;Require ($script:calls.Count -eq 3 -and @($script:calls|Where-Object{!$_.soak -or $_.duration -ne 1800}).Count -eq 0 -and $script:batchRecord.counts.acceptedSoaks -eq 3 -and $script:batchRecord.accepted -and $script:batchRecord.correctnessAccepted -and !$script:batchRecord.timingAccepted -and (Get-BatchExitCode $false $script:batchRecord) -eq 0) 'Normal schedule/acceptance changed'}
Check 'normal optional baseline is still followed by the original three soaks' {Reset $false $true;&$script:mainSchedule;Require ($script:calls.Count -eq 4 -and !$script:calls[0].soak -and $script:batchRecord.baselineAccepted -and $script:batchRecord.counts.requestedSoaks -eq 3 -and $script:batchRecord.counts.acceptedSoaks -eq 3 -and $script:batchRecord.accepted) 'Normal baseline-plus-soak schedule changed'}
Check 'normal failed optional baseline still prevents all long seeds' {Reset $false $true;$script:baselinePass=$false;&$script:mainSchedule;Require ($script:calls.Count -eq 1 -and $script:batchRecord.stopScheduling -and !$script:batchRecord.accepted) 'Failure proceeded into soaks'}
Check 'normal single exclusive seed retains formal timing classification' {Reset $false $false timing-exclusive;$script:Seeds=@([UInt64]104729);$script:soakSeeds=Get-SoakSeeds $script:Seeds $false;&$script:mainSchedule;Require ($script:calls.Count -eq 1 -and $script:calls[0].soak -and $script:batchRecord.accepted -and $script:batchRecord.timingAccepted -and $script:batchRecord.frameTimeStatus -eq 'formal (exclusive GPU)') 'Formal accepted schedule changed'}
Check 'bounded-only cannot certify a thirty-minute timing batch' {Reset $true $true timing-exclusive;$script:Seeds=@([UInt64]104729);&$script:mainSchedule;Require ($script:batchRecord.baselineAccepted -and !$script:batchRecord.accepted -and !$script:batchRecord.timingAccepted -and $script:batchRecord.counts.requestedSoaks -eq 0) 'Short timing entry became a long timing batch'}
Check 'an accidental empty normal schedule cannot vacuously pass' {Reset $false $false;$script:soakSeeds=[UInt64[]]@();Save-Batch;Require (!$script:batchRecord.accepted -and (Get-BatchExitCode $false $script:batchRecord) -eq 1) 'Empty request certified acceptance'}
$output=[ordered]@{driverSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $driver).Hash;passed=$cases.Count;nativeProcessesLaunched=0;processesStopped=0;testKind='synthetic parameter and real scheduling-block checks only';cases=$cases.ToArray()}
$output|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $PSScriptRoot 'monkey-baseline-only-tests.json') -Encoding UTF8
Write-Output "$($cases.Count) bounded-only cases passed; no native workload was launched."
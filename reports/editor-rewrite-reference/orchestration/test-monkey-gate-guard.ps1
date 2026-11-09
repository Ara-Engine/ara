# Synthetic parser/ownership tests ONLY. No editor, build, GPU work, process launch
# or process termination occurs; Stop-Gate* is never invoked by these tests.
Set-StrictMode -Version 2.0
$ErrorActionPreference='Stop'
$guard=Join-Path $PSScriptRoot 'monkey-gate-guard.ps1'
. $guard
$driver=Join-Path $PSScriptRoot 'monkey-acceptance.ps1'
$cases=New-Object 'Collections.Generic.List[object]'
function Check([string]$Name,[scriptblock]$Assert) {
    & $Assert
    $cases.Add([pscustomobject]@{case=$Name;passed=$true})
}
function Require([bool]$Condition,[string]$Message) {if(!$Condition){throw $Message}}
function Phase([string]$Text,[string]$Expected,[bool]$Allowed) {
    $result=Parse-GateLog $Text
    Require ($result.phase -eq $Expected -and $result.allowed -eq $Allowed) "Expected $Expected/$Allowed; got $($result|ConvertTo-Json -Compress)"
    return $result
}
$sha='a'*40
$prefix="Running ara-dev.exe check --base abcdef12 --json`nBase: $sha (merge-base)`nCompilation: disabled by flag; CARGO_INCREMENTAL=0`n"
$prepared="tests: 62 prepared harnesses, 4 workers, 4053 MiB admission budget; 120s fast / per-rule slow deadlines`n"
$rows=@(foreach($name in @('metadata','unchanged tree','pending snapshots')){[ordered]@{check=$name;passed=$true;known_failure=$false;timed_out=$false;seconds=0.1;detail='ok'}})
$report=[ordered]@{passed=$true;elapsed_seconds=24.5;compiler_cache_bypassed=$true;compilation=@{incremental='0';cache='disabled by flag'};head=$sha;base=$sha;base_reason='merge-base';full=$false;green_checkpoint_updated=$false;affected=@('ara-editor');test_packages=@('ara-editor');changed_libraries=@();test_only=@();reasons=@();known_failures=@();checks=$rows}
$terminal=$report|ConvertTo-Json -Depth 12
Check 'recognized compiling permits shared correctness' {$null=Phase $prefix 'compiling' $true}
Check 'test-build compilation is not prepared harness execution' {$null=Phase ($prefix+"test-build: cargo test --no-run --message-format=json`n") 'compiling' $true}
Check 'prepared harnesses without terminal JSON block' {$null=Phase ($prefix+$prepared) 'prepared-tests' $false}
Check 'completed successful gate clears' {$result=Phase ($prefix+$prepared+$terminal) 'completed' $true;Require $result.terminal 'Missing terminal evidence';Require $result.gatePassed 'Gate success not retained'}
Check 'completed failed gate clears active tests without claiming success' {$failed=$terminal -replace '"passed":\s*true','"passed": false';$result=Phase ($prefix+$prepared+$failed+"`nerror: process did not exit successfully (exit code: 1)`n") 'completed' $true;Require (!$result.gatePassed) 'Failed gate became green'}
Check 'failure before prepared harnesses still completes' {$null=Phase ($prefix+$terminal) 'completed' $true}
Check 'header selection plan is not a terminal gate' {$plan=@{affected=@('ara-editor');test_packages=@('ara-editor');checks=@('clippy','tests')}|ConvertTo-Json -Depth 5;$null=Phase ($prefix+$prepared+$plan) 'prepared-tests' $false}
Check 'explicit plan-only invocation never grants permission' {$null=Phase ("Running ara-dev.exe check --plan --json`n"+$prefix+$terminal) 'unknown' $false}
Check 'large telemetry JSON is not terminal' {$telemetry=@{telemetry=@{text=('x'*131072);marker='escaped quote " and brace } {'}}|ConvertTo-Json -Depth 5;$null=Phase ($prefix+$prepared+$telemetry) 'prepared-tests' $false}
Check 'telemetry before actual final report does not conceal completion' {$telemetry=@{telemetry=@{frame=9;quoted='} " {'}}|ConvertTo-Json -Depth 5;$null=Phase ($prefix+$prepared+$telemetry+"`n"+$terminal) 'completed' $true}
Check 'partial pretty terminal JSON fails closed' {$null=Phase ($prefix+$prepared+$terminal.Substring(0,$terminal.Length-3)) 'unknown' $false}
Check 'partial one-line JSON fails closed even during compilation' {$null=Phase ($prefix+'{"passed": true') 'unknown' $false}
Check 'malformed terminal result fails closed' {$null=Phase ($prefix+$prepared+"{`n  invalid JSON`n}") 'unknown' $false}
Check 'valid but incomplete result schema is not terminal' {$null=Phase ($prefix+$prepared+"{`n  `"passed`": true`n}") 'unknown' $false}
Check 'string true cannot masquerade as typed terminal bool' {$invalid=$terminal -replace '"passed":\s*true','"passed": "true"';$null=Phase ($prefix+$prepared+$invalid) 'unknown' $false}
Check 'new prepared work after old terminal report blocks' {$null=Phase ($prefix+$prepared+$terminal+"`n"+$prepared) 'prepared-tests' $false}
Check 'new invocation excludes previous terminal report' {$null=Phase ($prefix+$terminal+"`nBase: $sha (new invocation)`n") 'unknown' $false}
Check 'empty or arbitrary newest log fails closed' {$null=Phase '' 'unknown' $false;$null=Phase 'cargo compiler output only' 'unknown' $false}

$time=[DateTime]::UtcNow.AddMinutes(-1)
function File([string]$Name,[DateTime]$Write){return [pscustomobject]@{Name=$Name;FullName=('D:\AraBuild\'+$Name);LastWriteTimeUtc=$Write;Length=99L}}
Check 'highest numeric gate wins over older late flush' {$chosen=Select-NewestGate @((File 'gate-b65.log' $time.AddSeconds(30)),(File 'gate-b66.log' $time));Require ($chosen.file.Name -eq 'gate-b66.log') 'Older b65 late write concealed b66'}
Check 'same numeric gate chooses latest retry write' {$chosen=Select-NewestGate @((File 'gate-b65.log' $time),(File 'gate-b65b.log' $time.AddSeconds(1)));Require ($chosen.file.Name -eq 'gate-b65b.log') 'Retry not selected'}
Check 'UTF8 derivative never overrides its live report' {$chosen=Select-NewestGate @((File 'gate-b66.log' $time),(File 'gate-b66.utf8.log' $time.AddSeconds(1)));Require ($chosen.file.Name -eq 'gate-b66.log') 'Derived copy became authoritative'}
Check 'numeric gate order is not lexicographic' {$chosen=Select-NewestGate @((File 'gate-b9.log' $time.AddSeconds(1)),(File 'gate-b10.log' $time));Require ($chosen.sequence -eq 10) 'b9 superseded b10'}

# A synthetic directory allows real cache/rollover/read-failure tests without
# touching any actual gate log or accepting these files as native evidence.
$fixture=Join-Path $PSScriptRoot ('gate-guard-fixture-'+[Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($fixture)|Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'SYNTHETIC.txt'),'Guard tests only; no native workload or process control ran.')
$path=Join-Path $fixture 'gate-b800.log'
[IO.File]::WriteAllText($path,$prefix)
$state=New-GateGuard
Check 'real reader caches recognized compilation' {$first=Read-GateGuard $state $fixture;$second=Read-GateGuard $state $fixture;Require ($first.allowed -and [object]::ReferenceEquals($first,$second)) 'Unchanged status was not cached'}
Check 'new higher empty gate fails closed instead of reusing clear cache' {[IO.File]::WriteAllText((Join-Path $fixture 'gate-b801.log'),'');$value=Read-GateGuard $state $fixture;Require (!$value.allowed -and $value.sequence -eq 801) 'Rollover inherited old clear state'}
Check 'higher prepared gate blocks then real final report clears' {$new=Join-Path $fixture 'gate-b801.log';[IO.File]::WriteAllText($new,$prefix+$prepared);$value=Read-GateGuard $state $fixture;Require (!$value.allowed -and $value.phase -eq 'prepared-tests') 'Prepared gate did not block';[IO.File]::AppendAllText($new,$terminal);$value=Read-GateGuard $state $fixture;Require ($value.allowed -and $value.phase -eq 'completed') 'Completed gate did not clear'}
Check 'truncated newest gate never reuses cached completion' {[IO.File]::WriteAllText((Join-Path $fixture 'gate-b801.log'),'');$value=Read-GateGuard $state $fixture;Require (!$value.allowed -and $value.reason -match 'truncated') 'Truncation reused old completed state'}
Check 'missing gate directory fails closed without cached completion' {$value=Read-GateGuard (New-GateGuard) (Join-Path $fixture 'absent');Require (!$value.allowed -and $value.phase -eq 'unknown') 'Missing directory was authorized'}
Check 'live held-open writer permits compiling then blocks prepared tests then completes' {
    $directory=Join-Path $fixture 'held-open';[IO.Directory]::CreateDirectory($directory)|Out-Null
    $livePath=Join-Path $directory 'gate-b900.log';$held=$null;$writer=$null
    try {
        # Match a redirecting writer: it grants read sharing, while readers must
        # independently grant its existing write access. UTF16 BOM is intentional.
        $held=[IO.File]::Open($livePath,[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::Read)
        $writer=New-Object IO.StreamWriter($held,[Text.Encoding]::Unicode)
        $liveState=New-GateGuard
        $writer.Write($prefix);$writer.Flush()
        $value=Read-GateGuard $liveState $directory
        Require ($value.allowed -and $value.phase -eq 'compiling') "Open writer concealed compiling phase: $($value.reason)"
        $writer.Write($prepared);$writer.Flush()
        $value=Read-GateGuard $liveState $directory
        Require (!$value.allowed -and $value.phase -eq 'prepared-tests') "Open writer concealed prepared harnesses: $($value.reason)"
        $writer.Write($terminal);$writer.Flush()
        $value=Read-GateGuard $liveState $directory
        Require ($value.allowed -and $value.phase -eq 'completed') "Open writer concealed terminal report: $($value.reason)"
    } finally {
        if($null -ne $writer){$writer.Dispose()}
        elseif($null -ne $held){$held.Dispose()}
    }
}

function ProcessRow([long]$Id,[long]$Parent,[int]$Second,[string]$Exe='D:\AraBuild\ux-editor-target\debug\ara-editor.exe') {
    return [pscustomobject]@{pid=$Id;parentPid=$Parent;startedUtc=([DateTimeOffset]::Parse('2026-10-28T10:00:00.1234560Z').AddSeconds($Second).ToString('o'));executable=$Exe}
}
$root=ProcessRow 100 90 0 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
$snapshot=@($root,(ProcessRow 101 100 1),(ProcessRow 102 101 2),(ProcessRow 103 102 3),(ProcessRow 200 90 1),(ProcessRow 201 200 2))
Check 'only root ancestry is selected; foreign GPU processes excluded' {$plan=Plan-GateOwnedTree $root @() $snapshot;Require (($plan.processes.pid -join ',') -eq '103,102,101,100') 'Owned tree/children-first order incorrect';Require ($plan.issues.Count -eq 0) 'Valid ancestry rejected'}
Check 'verified grandchildren precede editor and wrapper' {$plan=Plan-GateOwnedTree $root @() $snapshot;Require ($plan.processes[0].depth -eq 3 -and $plan.processes[-1].pid -eq 100) 'GPU descendant ordering incorrect';Require ($plan.processes[0].parentStartedUtc -eq $snapshot[2].startedUtc) 'Missing parent birth attribution'}
Check 'child predating supposed parent is not owned' {$bad=@($root,(ProcessRow 101 100 -1));$plan=Plan-GateOwnedTree $root @() $bad;Require ($plan.processes.Count -eq 1 -and $plan.issues.Count -gt 0) 'Impossible ancestry accepted'}
Check 'missing child executable refuses termination ownership' {$child=ProcessRow 101 100 1;$child.executable=$null;$plan=Plan-GateOwnedTree $root @() @($root,$child);Require ($plan.processes.Count -eq 1 -and $plan.issues.Count -gt 0) 'Unidentified executable accepted'}
Check 'PID birth reuse does not inherit a previous child identity' {$old=Plan-GateOwnedTree $root @() $snapshot;$replacement=ProcessRow 102 200 9;$now=@($root,$snapshot[1],$replacement,$snapshot[4]);$plan=Plan-GateOwnedTree $root @($old.known) $now;Require (!(102 -in @($plan.processes.pid)) -and $plan.issues.Count -gt 0) 'Replacement process inherited ownership'}
Check 'wrapper PID reuse never transfers ownership' {$replacement=ProcessRow 100 90 20 $root.executable;$plan=Plan-GateOwnedTree $root @() @($replacement);Require ($plan.processes.Count -eq 0 -and $plan.issues.Count -gt 0) 'Replacement wrapper accepted'}
Check 'known orphan remains owned after exact wrapper exits' {$old=Plan-GateOwnedTree $root @() $snapshot;$plan=Plan-GateOwnedTree $root @($old.known) @($snapshot[1],$snapshot[2],$snapshot[3]);Require (($plan.processes.pid -join ',') -eq '103,102,101') 'Observed surviving descendants lost'}
Check 'unobserved orphan does not receive invented ownership or clean drain' {$known=@([pscustomobject]@{pid=$root.pid;startedUtc=$root.startedUtc;executable=$root.executable;depth=0});$plan=Plan-GateOwnedTree $root $known @($snapshot[1]);Require ($plan.processes.Count -eq 0 -and $plan.issues.Count -gt 0) 'Unobserved orphan falsely retired'}
Check 'CIM microsecond and native process tick births agree' {$fine=$root|Select-Object *;$fine.startedUtc='2026-10-28T10:00:00.1234567Z';Require (Same-OwnedIdentity $root $fine) 'Shared Windows birth precision rejected';$fine.startedUtc='2026-10-28T10:00:00.1234570Z';Require (!(Same-OwnedIdentity $root $fine)) 'Different microsecond accepted'}
Check 'same birth with different executable fails verification' {$changed=$root|Select-Object *;$changed.executable='D:\foreign.exe';Require (!(Same-OwnedIdentity $root $changed)) 'Executable mismatch accepted'}

# Inspect control flow without executing the driver or either stop function.
Check 'driver syntax remains valid and polls a bounded interval' {$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile($driver,[ref]$tokens,[ref]$errors);Require ($errors.Count -eq 0) 'Driver syntax errors';$text=$ast.Extent.Text;Require ($text -match 'WaitForExit\(750\)') 'No bounded active poll';Require ($text -match 'Observe-SegmentGate \$entry') 'Missing gate check';Require ($text -match 'discardedSoaks=' -and $text -match 'Where-Object\{!\$_\.discarded\}') 'Discarded segments included in acceptance/crash totals';Require ($text -match 'QuietSlotGranted') 'Missing explicit timing authorization'}
Check 'guard syntax valid; process control never uses name-based kill' {$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile($guard,[ref]$tokens,[ref]$errors);Require ($errors.Count -eq 0) 'Guard syntax errors';$text=$ast.Extent.Text;Require ($text -notmatch '(?im)\b(?:taskkill|Stop-Process)\b[^\r\n]*(?:/IM|-Name)') 'Name-based termination present';Require ($text -match 'Same-OwnedIdentity \$Identity \$actual') 'Missing immediate birth/executable revalidation'}
$output=[ordered]@{testKind='synthetic gate parser/ownership only; no process launched or terminated';fixture=$fixture;driverSha256=(Get-FileHash -LiteralPath $driver -Algorithm SHA256).Hash;guardSha256=(Get-FileHash -LiteralPath $guard -Algorithm SHA256).Hash;passed=$cases.Count;cases=$cases.ToArray()}
$output|ConvertTo-Json -Depth 10|Set-Content -LiteralPath (Join-Path $PSScriptRoot 'monkey-gate-guard-tests.json') -Encoding UTF8
Write-Output "$($cases.Count) pure gate guard cases passed; no native workload or process control ran."

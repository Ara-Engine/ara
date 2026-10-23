param([Parameter(Mandatory)][string]$ControlBinary, [Parameter(Mandatory)][string]$CurrentBinary, [int]$Steps = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $PSScriptRoot 'artifacts.ps1')
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
$controlBuild = Get-BinaryBuild $ControlBinary (Get-FileHash -LiteralPath $ControlBinary -Algorithm SHA256).Hash
$currentBuild = Get-BinaryBuild $CurrentBinary (Get-FileHash -LiteralPath $CurrentBinary -Algorithm SHA256).Hash
if (-not $controlBuild.ecsOnly -or -not $currentBuild.ecsOnly -or
    $controlBuild.harnessSourceSha256.performance -ne $currentBuild.harnessSourceSha256.performance -or
    $controlBuild.harnessSourceSha256.workloads -ne $currentBuild.harnessSourceSha256.workloads) {
    throw 'ECS binaries must use identical standalone harness sources'
}
$base = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'RESULTS.json') -Raw | ConvertFrom-Json
if (-not $base.runId -or -not $base.caseNames) { throw 'A completed result manifest is required' }
$run = New-BenchmarkRun $repo $results
$run.runId = $base.runId
$env:ARA_PHYSICS_BENCH_RUN_ID = $base.runId
# Replace only this experiment on repeat; preserve completed earlier stages.
foreach ($name in $base.caseNames) {
    if ($name -notlike 'ecs-final-*' -and $name -ne 'ecs-provenance') { $run.cases.Add($name) }
}
foreach ($setting in @('SCRATCH_MB','NATIVE_SCRATCH_MB','POOL','NATIVE_POOL','SPLITTER','NATIVE_SPLITTER','ORDERING','NATIVE_ORDERING')) {
    Remove-Item "Env:ARA_PHYSICS_BENCH_$setting" -ErrorAction SilentlyContinue
}
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
$env:ARA_PHYSICS_BENCH_ECS_ONLY = '1'
Invoke-Benchmark $run $repo $results $ControlBinary 'ecs-final-before-a'
Invoke-Benchmark $run $repo $results $CurrentBinary 'ecs-final-after-a'
Invoke-Benchmark $run $repo $results $CurrentBinary 'ecs-final-after-b'
Invoke-Benchmark $run $repo $results $ControlBinary 'ecs-final-before-b'
$hashes = foreach ($name in @('ecs-final-before-a','ecs-final-after-a','ecs-final-after-b','ecs-final-before-b')) {
    $case = Get-Content -LiteralPath (Join-Path $results "$name.json") -Raw | ConvertFrom-Json
    $hash = $case.records[0].finalStateHash
    if (-not $hash -or -not $case.records[0].totalIncludesTrackerMaintenance) { throw 'Mismatched ECS harness' }
    $hash
}
if (@($hashes | Select-Object -Unique).Count -ne 1) { throw 'ECS before/after final body bits differ' }
Publish-BenchmarkRun $run $bins $results 'ecs-provenance'

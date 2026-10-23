param([int]$Steps = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $PSScriptRoot 'artifacts.ps1')
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
$binary = Join-Path $bins 'ecs-bowl.exe'
if (-not (Get-BinaryBuild $binary (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash)) {
    throw 'Record the ECS bowl binary build revision and SHA256 sidecar before measuring'
}
$base = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'RESULTS.json') -Raw | ConvertFrom-Json
if (-not $base.runId -or -not $base.caseNames) { throw 'A completed result manifest is required' }
$run = New-BenchmarkRun $repo $results
$run.runId = $base.runId
$env:ARA_PHYSICS_BENCH_RUN_ID = $base.runId
foreach ($name in $base.caseNames) {
    if ($name -notlike 'ecs-bowl-*') { $run.cases.Add($name) }
}
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
foreach ($preset in @('s6','game_default','stress','deterministic_network')) {
    $env:ARA_PHYSICS_BENCH_PRESET = $preset
    Invoke-Benchmark $run $repo $results $binary "ecs-bowl-$preset"
}
Publish-BenchmarkRun $run $bins $results 'ecs-bowl-provenance'

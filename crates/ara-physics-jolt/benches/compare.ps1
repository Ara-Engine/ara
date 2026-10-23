param([Parameter(Mandatory)][string]$BaselineBinary, [string]$PreviousEcsBinary, [switch]$IncludeMatrix, [switch]$FpCost, [int]$Steps = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $PSScriptRoot 'artifacts.ps1')
Clear-AggregationMode
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $results | Out-Null
if ($PreviousEcsBinary) {
    $build = Get-BinaryBuild $PreviousEcsBinary (Get-FileHash -LiteralPath $PreviousEcsBinary -Algorithm SHA256).Hash
    if (-not $build.trackerMaintenance -or -not $build.lookupControl) { throw 'ECS comparison requires a lookup control built with the matching tracker-maintenance harness' }
}
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_PRESET = 'game_default'
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
$env:ARA_PHYSICS_BENCH_ECS_ONLY = '0'
$run = New-BenchmarkRun $repo $results -IncludeMatrix:$IncludeMatrix
function Measure-Binary([string]$Binary, [string]$Name) {
    Invoke-Benchmark $run $repo $results $Binary $Name
}
if ($FpCost) {
    $env:ARA_PHYSICS_BENCH_ROUNDS = '1'
    $env:ARA_PHYSICS_BENCH_EXTRAS = '0'
    $env:ARA_PHYSICS_BENCH_PRESETS = 'game_default'
    $env:ARA_PHYSICS_BENCH_COUNTS = '5000,20000'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'fp-deterministic-a'
    Measure-Binary (Join-Path $bins 'fast.exe') 'fp-fast-a'
    Measure-Binary (Join-Path $bins 'fast.exe') 'fp-fast-b'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'fp-deterministic-b'
    $env:ARA_PHYSICS_BENCH_ROUNDS = '0'
    $env:ARA_PHYSICS_BENCH_EXTRAS = '1'
}
# The retained baseline is the pre-pass S6 15/2, 64 MiB executable. It ignores
# PRESET. The current executable uses game_default, retaining 15/2 with 256 MiB.
Measure-Binary $BaselineBinary 'scale-before-a'
Measure-Binary (Join-Path $bins 'scale.exe') 'scale-after-a'
Measure-Binary (Join-Path $bins 'scale.exe') 'scale-after-b'
Measure-Binary $BaselineBinary 'scale-before-b'
$env:ARA_PHYSICS_BENCH_THREADS = '1,4,8'
Measure-Binary (Join-Path $bins 'scale.exe') 'scale-threads'
$env:ARA_PHYSICS_BENCH_THREADS = '8'
if ($PreviousEcsBinary) {
    # The control differs only in per-row binding lookup; both harnesses clear
    # and time trackers. Compare ECS rows only.
    Measure-Binary $PreviousEcsBinary 'ecs-before-a'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'ecs-after-a'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'ecs-after-b'
    Measure-Binary $PreviousEcsBinary 'ecs-before-b'
}
Publish-BenchmarkRun $run $bins $results 'provenance'

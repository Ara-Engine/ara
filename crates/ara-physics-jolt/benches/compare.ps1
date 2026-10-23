param([Parameter(Mandatory)][string]$BaselineBinary, [string]$PreviousEcsBinary, [switch]$IncludeMatrix, [int]$Steps = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
. (Join-Path $PSScriptRoot 'artifacts.ps1')
Clear-AggregationMode
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $results | Out-Null
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_PRESET = 'game_default'
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
$run = New-BenchmarkRun $repo $results -IncludeMatrix:$IncludeMatrix
function Measure-Binary([string]$Binary, [string]$Name) {
    Invoke-Benchmark $run $repo $results $Binary $Name
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
    # This intermediate executable already has SoA/change-only writes, before
    # replacing repeated binding lookups with ordered walks. Compare ECS rows only.
    Measure-Binary $PreviousEcsBinary 'ecs-before-a'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'ecs-after-a'
    Measure-Binary (Join-Path $bins 'deterministic.exe') 'ecs-after-b'
    Measure-Binary $PreviousEcsBinary 'ecs-before-b'
}
Publish-BenchmarkRun $run $bins $results 'provenance'

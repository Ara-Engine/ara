param([switch]$BuildOnly, [int]$Steps = 600, [int]$Rounds = 2)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
Set-Location $repo
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '2'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $bins, $results | Out-Null
function Build-Bench([string]$Name, [string[]]$Features) {
    $messages = & cargo bench -p ara-physics-jolt --bench performance --no-run --message-format=json @Features
    if ($LASTEXITCODE -ne 0) { throw "Build failed: $Name" }
    $artifact = $messages | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'performance' -and $_.executable } | Select-Object -Last 1
    if (-not $artifact) { throw "Missing performance executable: $Name" }
    Copy-Item -LiteralPath $artifact.executable -Destination (Join-Path $bins "$Name.exe") -Force
}
Build-Bench 'deterministic' @('--features', 'bench')
Build-Bench 'fast' @('--no-default-features', '--features', 'bench')
Build-Bench 'profile' @('--features', 'profile')
if ($BuildOnly) { return }
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_ROUNDS = "$Rounds"
$env:ARA_PHYSICS_BENCH_PRESETS = 's6,game_default,stress,deterministic_network'
$env:ARA_PHYSICS_BENCH_COUNTS = '5000,20000'
function Run-Bench([string]$Binary, [string]$Name) {
    $env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $results "$Name.json"
    & (Join-Path $bins "$Binary.exe") *> (Join-Path $results "$Name.log")
    if ($LASTEXITCODE -ne 0) { throw "Benchmark failed: $Name (see results/$Name.log)" }
}
# Reverse build-flavour order across rounds. Each binary also alternates direct
# Jolt and Ara every tick; presets always disclose their distinct settings.
Run-Bench 'deterministic' 'deterministic-a'
Run-Bench 'fast' 'fast-a'
Run-Bench 'fast' 'fast-b'
Run-Bench 'deterministic' 'deterministic-b'
$env:ARA_PHYSICS_BENCH_ROUNDS = '1'
$env:ARA_PHYSICS_BENCH_PRESETS = 's6'
$env:ARA_PHYSICS_BENCH_SCRATCH_MB = '256'
$env:ARA_PHYSICS_BENCH_NATIVE_SCRATCH_MB = '64'
Run-Bench 'deterministic' 'arena256-vs64'
Remove-Item Env:ARA_PHYSICS_BENCH_NATIVE_SCRATCH_MB
$env:ARA_PHYSICS_BENCH_POOL = 'shared'
$env:ARA_PHYSICS_BENCH_NATIVE_POOL = 'dedicated'
Run-Bench 'deterministic' 'shared-vs-dedicated'
Remove-Item Env:ARA_PHYSICS_BENCH_NATIVE_POOL
Remove-Item Env:ARA_PHYSICS_BENCH_POOL
$env:ARA_PHYSICS_BENCH_SCRATCH_MB = '256'
$env:ARA_PHYSICS_BENCH_NATIVE_SPLITTER = 'off'
Run-Bench 'deterministic' 'splitter-on-vs-off'
Remove-Item Env:ARA_PHYSICS_BENCH_NATIVE_SPLITTER
$env:ARA_PHYSICS_BENCH_NATIVE_ORDERING = 'off'
Run-Bench 'deterministic' 'ordering-on-vs-off'
Remove-Item Env:ARA_PHYSICS_BENCH_NATIVE_ORDERING
$env:ARA_PHYSICS_BENCH_PRESETS = 'game_default,stress'
Run-Bench 'profile' 'phases'
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
Run-Bench 'deterministic' 'outpost-queries-ecs'
$env:ARA_PHYSICS_BENCH_AGGREGATE_DIR = $results
$env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $PSScriptRoot 'RESULTS.json'
& (Join-Path $bins 'deterministic.exe')
if ($LASTEXITCODE -ne 0) { throw 'Result aggregation failed' }

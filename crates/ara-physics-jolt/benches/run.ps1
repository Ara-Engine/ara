param([switch]$BuildOnly, [switch]$RunOnly, [int]$Steps = 600, [int]$Rounds = 2)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
Set-Location $repo
. (Join-Path $PSScriptRoot 'artifacts.ps1')
Clear-AggregationMode
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '2'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $bins, $results | Out-Null
function Build-Bench([string]$Name, [string[]]$Features, [string]$Bench = 'performance') {
    $stdout = Join-Path $bins "build-$Name.jsonl"
    $stderr = Join-Path $bins "build-$Name.log"
    $arguments = @('bench', '-p', 'ara-physics-jolt', '--bench', $Bench, '--no-run', '--message-format=json') + $Features
    $process = Start-Process -FilePath (Get-Command cargo.exe).Source -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Build failed: $Name (see $stderr and $stdout)" }
    $messages = Get-Content -LiteralPath $stdout
    $artifact = $messages | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq $Bench -and $_.executable } | Select-Object -Last 1
    if (-not $artifact) { throw "Missing $Bench executable: $Name" }
    Copy-Item -LiteralPath $artifact.executable -Destination (Join-Path $bins "$Name.exe") -Force
    $binary = Join-Path $bins "$Name.exe"
    Write-JsonInput "$binary.build.json" ([ordered]@{ buildRevision = (& git -C $repo rev-parse HEAD); dirty = [bool](& git -C $repo status --porcelain); binarySha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash; builtUtc = [DateTime]::UtcNow.ToString('o'); cargoArguments = $arguments })
}
if (-not $RunOnly) {
    Build-Bench 'deterministic' @('--features', 'bench')
    Build-Bench 'scale' @('--features', 'bench') 'scale'
    Build-Bench 'fast' @('--no-default-features', '--features', 'bench')
    Build-Bench 'profile' @('--features', 'profile')
}
if ($BuildOnly) { $global:LASTEXITCODE = 0; return }
$run = New-BenchmarkRun $repo $results
foreach ($setting in @('EXTRAS','SCRATCH_MB','NATIVE_SCRATCH_MB','POOL','NATIVE_POOL','SPLITTER','NATIVE_SPLITTER','ORDERING','NATIVE_ORDERING')) {
    Remove-Item "Env:ARA_PHYSICS_BENCH_$setting" -ErrorAction SilentlyContinue
}
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_ROUNDS = "$Rounds"
$env:ARA_PHYSICS_BENCH_PRESETS = 's6,game_default,stress,deterministic_network'
$env:ARA_PHYSICS_BENCH_COUNTS = '5000,20000'
function Run-Bench([string]$Binary, [string]$Name) {
    Invoke-Benchmark $run $repo $results (Join-Path $bins "$Binary.exe") $Name
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
$env:ARA_PHYSICS_BENCH_PRESETS = 's6,game_default,stress'
Remove-Item Env:ARA_PHYSICS_BENCH_SCRATCH_MB
Run-Bench 'profile' 'phases'
$env:ARA_PHYSICS_BENCH_PRESETS = 's6'
$env:ARA_PHYSICS_BENCH_SCRATCH_MB = '256'
$env:ARA_PHYSICS_BENCH_NATIVE_SCRATCH_MB = '64'
Run-Bench 'profile' 'phases-arena256-vs64'
Remove-Item Env:ARA_PHYSICS_BENCH_SCRATCH_MB
Remove-Item Env:ARA_PHYSICS_BENCH_NATIVE_SCRATCH_MB
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
Run-Bench 'deterministic' 'outpost-queries-ecs'
Publish-BenchmarkRun $run $bins $results 'matrix-provenance'

param([Parameter(Mandatory)][string]$BaselineBinary, [string]$PreviousEcsBinary, [int]$Steps = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$bins = Join-Path $target 'physics-bench'
$results = Join-Path $PSScriptRoot 'results'
New-Item -ItemType Directory -Force $results | Out-Null
$env:ARA_PHYSICS_BENCH_THREADS = '8'
$env:ARA_PHYSICS_BENCH_STEPS = "$Steps"
$env:ARA_PHYSICS_BENCH_PRESET = 'game_default'
$env:ARA_PHYSICS_BENCH_ROUNDS = '0'
$env:ARA_PHYSICS_BENCH_EXTRAS = '1'
$provenance = [ordered]@{ steps = $Steps; order = 'ABBA'; runs = @() }
function Host-Load {
    $cpu = Get-CimInstance Win32_Processor
    $os = Get-CimInstance Win32_OperatingSystem
    return [ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); cpuPercent = $cpu.LoadPercentage; freeMemoryKiB = $os.FreePhysicalMemory; processCount = (Get-Process).Count }
}
function Measure-Binary([string]$Binary, [string]$Name) {
    $binaryPath = (Resolve-Path -LiteralPath $Binary).Path
    $before = Host-Load
    $env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $results "$Name.json"
    $process = Start-Process -FilePath $binaryPath -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput (Join-Path $results "$Name.log") -RedirectStandardError (Join-Path $results "$Name.stderr.log") -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Benchmark failed: $Name" }
    $provenance.runs += [ordered]@{ name = $Name; binarySha256 = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash; loadBefore = $before; loadAfter = (Host-Load) }
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
# Intermediate metadata is canonicalized by Ara's writer in the final aggregate.
$metadata = $provenance | ConvertTo-Json -Depth 32
[System.IO.File]::WriteAllText((Join-Path $results 'provenance.json'), $metadata, [System.Text.UTF8Encoding]::new($false))
$env:ARA_PHYSICS_BENCH_AGGREGATE_DIR = $results
$env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $PSScriptRoot 'RESULTS.json'
& (Join-Path $bins 'deterministic.exe')
if ($LASTEXITCODE -ne 0) { throw 'Result aggregation failed' }

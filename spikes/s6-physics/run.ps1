# Builds each S6 flavour into its own target dir and runs its scenarios. Results land in
# evidence/<flavour-tag>/*.json; machine info and CPU load samples in evidence/machine.json
# and evidence/load-<flavour>.json. Run setup.ps1 once first.
#
#   ./run.ps1                         # every flavour (about 30 minutes on the 7800X3D)
#   ./run.ps1 -Flavors fast-st,det-par
#   ./run.ps1 -Only determinism       # only runs whose command starts with this
param(
    [string[]]$Flavors = @('fast-st', 'fast-par', 'fast-par-simd8-avx2', 'det-par'),
    [string]$Only = ''
)
$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_JOBS = '4'
Set-Location $PSScriptRoot
New-Item -ItemType Directory -Force evidence | Out-Null

function Get-Load {
    # Average total CPU % over 3 one-second samples (other agents share this PC).
    $s = Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 3
    [math]::Round(($s.CounterSamples | Measure-Object -Property CookedValue -Average).Average, 1)
}

$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os = Get-CimInstance Win32_OperatingSystem
[ordered]@{
    cpu = $cpu.Name.Trim()
    cores = $cpu.NumberOfCores
    threads = $cpu.NumberOfLogicalProcessors
    ram_bytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
    os = "$($os.Caption) $($os.Version)"
    rustc = (rustc --version)
    jolt_rev = (git -C vendor/JoltPhysics rev-parse HEAD)
    rapier = '0.36.1'
} | ConvertTo-Json | Out-File -Encoding utf8 evidence/machine.json

$defs = @{
    'fast-st'             = @{ features = ''; rustflags = ''; runs = @(
            'character', 'stacking', 'vehicle', 'queries',
            'determinism --threads 1',
            'scale --count 5000,20000 --threads 1') }
    'fast-par'            = @{ features = 'parallel'; rustflags = ''; runs = @(
            'determinism --threads 1,4,8',
            'scale --count 5000,20000 --threads 1,4,8') }
    'fast-par-simd8-avx2' = @{ features = 'parallel,simd8'; rustflags = '-C target-cpu=x86-64-v3'; runs = @(
            'queries',
            'determinism --threads 1,8',
            'scale --count 5000,20000 --threads 1,4,8') }
    'det-par'             = @{ features = 'det,parallel'; rustflags = ''; runs = @(
            'character', 'stacking', 'vehicle',
            'determinism --threads 1,4,8',
            'scale --count 5000,20000 --threads 1,8') }
}

foreach ($flavor in $Flavors) {
    $d = $defs[$flavor]
    if (-not $d) { throw "unknown flavour $flavor" }
    $env:RUSTFLAGS = $d.rustflags
    $env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot "target/$flavor"
    $featureArgs = if ($d.features) { @('--features', $d.features) } else { @() }
    Write-Host "== $flavor : building"
    cargo build --release --quiet @featureArgs
    if ($LASTEXITCODE -ne 0) { throw "build failed: $flavor" }
    $exe = Join-Path $env:CARGO_TARGET_DIR 'release/s6.exe'
    $load = [ordered]@{ flavor = $flavor; before_pct = (Get-Load); runs = @() }
    foreach ($run in ($d.runs | Where-Object { $_.StartsWith($Only) })) {
        Write-Host "== $flavor : s6 $run"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        & $exe @($run -split ' ') | Out-File -Encoding utf8 "evidence/log-$flavor.txt" -Append
        if ($LASTEXITCODE -ne 0) { throw "s6 $run failed ($flavor)" }
        $load.runs += [ordered]@{ run = $run; seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1); load_after_pct = (Get-Load) }
    }
    $suffix = if ($Only) { "-$($Only.Split(' ')[0])" } else { '' }
    $load | ConvertTo-Json -Depth 4 | Out-File -Encoding utf8 "evidence/load-$flavor$suffix.json"
}
$env:RUSTFLAGS = ''
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
python summarize.py

# Builds the S5 spike in release and regenerates every file in evidence/.
# Run from any directory in PowerShell 5.1:  ./spikes/s5-play-process/run.ps1
param([int]$Reps = 20, [int]$Patches = 200)
# Native tools report failure through exit codes; PowerShell 5.1 would turn their redirected
# stderr into terminating errors under 'Stop'.
$ErrorActionPreference = 'Continue'
Set-Location $PSScriptRoot
$env:CARGO_BUILD_JOBS = '4'

cargo build --release
if ($LASTEXITCODE) { throw 'cargo build failed' }
$exe = './target/release/ara-s5-play.exe'
function Run([string[]]$Arguments) {
    & $exe @Arguments
    if ($LASTEXITCODE) { throw "ara-s5-play $($Arguments -join ' ') failed" }
}

New-Item -ItemType Directory -Force evidence | Out-Null
Run gen-scene, --count, 5000, --out, scenes/orbit-5k.json
Run gen-scene, --count, 50000, --out, scenes/orbit-50k.json
Run probe-timers, --out, evidence/timers.json

# All suites at 5k entities, including Vulkan-initialising players.
Run host, --scene, scenes/orbit-5k.json, --out, evidence/host-5k.json, --reps, $Reps, --patches, $Patches, --gpu
# Transfer, boot, live link and keep-changes at 50k entities.
Run host, --scene, scenes/orbit-50k.json, --out, evidence/host-50k.json, --reps, $Reps, --patches, $Patches, --suites, 'snapshot,boot,live,keep'
# Same live link with Nagle's algorithm left on (TCP_NODELAY off).
$env:ARA_S5_NAGLE = '1'
try { Run host, --scene, scenes/orbit-5k.json, --out, evidence/live-nagle-5k.json, --patches, $Patches, --suites, live }
finally { Remove-Item Env:ARA_S5_NAGLE }

# Machine record.
$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.Name -like '*NVIDIA*' } | Select-Object -First 1
$machine = [ordered]@{
    os = "$($os.Caption) $($os.Version)"
    cpu = $cpu.Name.Trim()
    logicalProcessors = $cpu.NumberOfLogicalProcessors
    ramBytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
    gpu = $gpu.Name
    gpuDriver = $gpu.DriverVersion
    rustc = (rustc --version)
    defenderRealtime = (Get-MpComputerStatus -ErrorAction SilentlyContinue).RealTimeProtectionEnabled
}
[IO.File]::WriteAllText("$PSScriptRoot/evidence/machine.json", ($machine | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
Write-Host 'evidence/ updated'

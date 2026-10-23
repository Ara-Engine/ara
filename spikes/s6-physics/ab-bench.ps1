# Interleaved, repeated A/B timing of the scale scene across builds, because background load
# from other agents swings by tens of percent between runs. Each repetition runs every
# (engine, build) pair back to back, rotating the build order. Requires the binaries from
# run.ps1 (target/<flavour>/release/s6.exe). Results: evidence/ab/r<N>/<tag>/scale-*.json;
# summarize.py reports the median across repetitions.
param([int]$Reps = 5)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$builds = @('fast-par', 'det-par', 'fast-par-simd8-avx2')
foreach ($b in $builds) {
    if (-not (Test-Path "target/$b/release/s6.exe")) { throw "missing target/$b/release/s6.exe; run ./run.ps1 first" }
}
$loads = @()
for ($rep = 1; $rep -le $Reps; $rep++) {
    $order = $builds[($rep % 3)..2] + $builds[0..(($rep % 3) - 1)] | Select-Object -Unique
    foreach ($engine in 'jolt', 'rapier') {
        foreach ($b in $order) {
            $exe = "target/$b/release/s6.exe"
            & $exe scale --engine $engine --count 5000 --threads 1 --steps 600 --out "evidence/ab/r$rep" | Out-Null
            & $exe scale --engine $engine --count 20000 --threads 8 --steps 300 --out "evidence/ab/r$rep" | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "s6 failed ($b, $engine)" }
        }
    }
    $s = Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 3
    $loads += [math]::Round(($s.CounterSamples | Measure-Object -Property CookedValue -Average).Average, 1)
    Write-Host "rep $rep done, CPU load $($loads[-1]) %"
}
@{ reps = $Reps; load_after_each_rep_pct = $loads } | ConvertTo-Json | Out-File -Encoding utf8 evidence/ab/load.json

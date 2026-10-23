# Clean release build time and binary size of the three probes (no physics / Jolt shim /
# Rapier). Each probe has its own target dir, which this script deletes first.
# Writes evidence/build.json. Crates are already in the local registry cache, so download
# time is excluded. Run from this folder after setup.ps1.
$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_JOBS = '4'
$results = @()
foreach ($probe in 'empty', 'jolt', 'rapier') {
    $dir = Join-Path $PSScriptRoot "probes/$probe"
    $target = Join-Path $dir 'target'
    if (Test-Path $target) { Remove-Item -Recurse -Force $target }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    cargo build --release --quiet --manifest-path (Join-Path $dir 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "build failed: $probe" }
    $sw.Stop()
    $exe = Join-Path $target "release/probe-$probe.exe"
    $out = & $exe
    # Incremental: touch main.rs and rebuild (relink only).
    (Get-Item (Join-Path $dir 'src/main.rs')).LastWriteTime = Get-Date
    $inc = [Diagnostics.Stopwatch]::StartNew()
    cargo build --release --quiet --manifest-path (Join-Path $dir 'Cargo.toml')
    $inc.Stop()
    $results += [ordered]@{
        probe = $probe
        clean_build_s = [math]::Round($sw.Elapsed.TotalSeconds, 2)
        touch_rebuild_s = [math]::Round($inc.Elapsed.TotalSeconds, 2)
        exe_bytes = (Get-Item $exe).Length
        output = "$out"
    }
    Write-Host "$probe : clean $([math]::Round($sw.Elapsed.TotalSeconds, 1)) s, rebuild $([math]::Round($inc.Elapsed.TotalSeconds, 1)) s, $((Get-Item $exe).Length) bytes, prints $out"
}
New-Item -ItemType Directory -Force (Join-Path $PSScriptRoot 'evidence') | Out-Null
$results | ConvertTo-Json | Out-File -Encoding utf8 (Join-Path $PSScriptRoot 'evidence/build.json')

# Reproduce the compiler-only build gate without CMake/Python on PATH.
param([string]$TargetDirectory = '')
$ErrorActionPreference = 'Stop'
$crateRoot = Split-Path $PSScriptRoot -Parent
$repoRoot = Split-Path (Split-Path $crateRoot -Parent) -Parent
$previousPath = $env:PATH
$previousTarget = $env:CARGO_TARGET_DIR
$previousJobs = $env:CARGO_BUILD_JOBS
Push-Location $repoRoot
try {
    $env:PATH = ($env:PATH -split ';' | Where-Object {
        $_ -and -not (Test-Path (Join-Path $_ 'cmake.exe')) -and
        -not (Test-Path (Join-Path $_ 'python.exe'))
    }) -join ';'
    foreach ($tool in @('cmake', 'python', 'python3')) {
        if (Get-Command $tool -CommandType Application -ErrorAction SilentlyContinue) {
            throw "$tool is still executable on the stripped PATH"
        }
    }
    $env:CARGO_BUILD_JOBS = '2'
    $env:CARGO_TARGET_DIR = if ($TargetDirectory) { $TargetDirectory } else {
        Join-Path $crateRoot 'artifacts/clean-target'
    }
    Write-Output 'CMake/Python absent from PATH; CARGO_BUILD_JOBS=2'
    cargo check --workspace
    if ($LASTEXITCODE -ne 0) { throw 'Workspace check failed' }
    cargo test -p ara-audio -p ara-opus-sys -p ara-acoustics -p ara-audio-graph -p ara-audio-mix -p ara-music -p ara-audio-why
    if ($LASTEXITCODE -ne 0) { throw 'Audio tests failed' }
} finally {
    $env:PATH = $previousPath
    $env:CARGO_TARGET_DIR = $previousTarget
    $env:CARGO_BUILD_JOBS = $previousJobs
    Pop-Location
}

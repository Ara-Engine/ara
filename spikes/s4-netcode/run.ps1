# Run from any directory. All output and processes stay inside this spike.
$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_JOBS = '4'
Push-Location $PSScriptRoot
try {
    function Invoke-Cargo([string[]] $CargoArguments) {
        & cargo @CargoArguments
        if ($LASTEXITCODE -ne 0) { throw "cargo failed: $CargoArguments" }
    }
    function Save-CargoJson([string] $Destination, [string[]] $CargoArguments) {
        $json = & cargo @CargoArguments
        if ($LASTEXITCODE -ne 0) { throw "cargo failed: $CargoArguments" }
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot $Destination), ($json -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
    }
    Invoke-Cargo @('fmt', '--all', '--', '--check')
    Invoke-Cargo @('check', '--locked', '--all-targets')
    Invoke-Cargo @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo @('test', '--locked')
    Invoke-Cargo @('clippy', '--locked', '--no-default-features', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo @('test', '--locked', '--no-default-features')
    Save-CargoJson 'artifacts/local.json' @('run', '--quiet', '--locked', '--release', '--no-default-features', '--', 'local')
    Save-CargoJson 'artifacts/bench-local.json' @('run', '--quiet', '--locked', '--release', '--no-default-features', '--', 'bench')
    Save-CargoJson 'artifacts/bench-network-enabled.json' @('run', '--quiet', '--locked', '--release', '--', 'bench')
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'harness', 'dedicated3', 'impaired', '21668', 'artifacts/dedicated3-impaired.json')
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'harness', 'dedicated4', 'impaired', '21668', 'artifacts/dedicated4-impaired.json')
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'harness', 'listen', 'impaired', '21668', 'artifacts/listen-impaired.json')
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'harness', 'dedicated4', 'clean', '21668', 'artifacts/dedicated4-clean.json')
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'harness', 'dedicated4-live', 'impaired', '21668', 'artifacts/dedicated4-live-impaired.json')
    $measured = Get-Content -Raw 'artifacts/dedicated4-impaired.json' | ConvertFrom-Json
    Invoke-Cargo @('run', '--quiet', '--locked', '--release', '--', 'replay', (Join-Path $measured.runtime_artifacts 'replay.json'))
} finally {
    Pop-Location
}

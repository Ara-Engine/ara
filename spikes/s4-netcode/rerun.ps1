# Rust 1.99.1 comparison: independent clean targets, then identical socket scenarios.
$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_JOBS = '4'
$env:CARGO_INCREMENTAL = '0'
Push-Location $PSScriptRoot
try {
    function Invoke-Cargo([string[]] $CargoArguments) {
        & cargo @CargoArguments
        if ($LASTEXITCODE -ne 0) { throw "cargo failed: $CargoArguments" }
    }
    function Save-JsonOutput([string] $Executable, [string] $Destination, [string[]] $RunArguments) {
        $json = & $Executable @RunArguments
        if ($LASTEXITCODE -ne 0) { throw "command failed: $Executable $RunArguments" }
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot $Destination), ($json -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
    }
    Invoke-Cargo @('fmt', '--all', '--', '--check')
    Invoke-Cargo @('check', '--locked', '--all-features', '--all-targets')
    Invoke-Cargo @('clippy', '--locked', '--all-features', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo @('clippy', '--locked', '--no-default-features', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo @('test', '--locked', '--all-features')
    Invoke-Cargo @('test', '--locked', '--no-default-features')

    New-Item -ItemType Directory -Force artifacts/rust199 | Out-Null
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmss')
    $builds = @()
    foreach ($name in @('renet','lightyear','local')) {
        $target = "target/clean-$name-$stamp"
        if (Test-Path $target) { throw "Clean target already exists: $target" }
        $arguments = @('build','--offline','--locked','--release','--no-default-features','--target-dir',$target)
        if ($name -ne 'local') { $arguments += @('--features', "$name-stack") }
        $outLog = Join-Path $PSScriptRoot "artifacts/rust199/build-$name.stdout.txt"
        $errLog = Join-Path $PSScriptRoot "artifacts/rust199/build-$name.stderr.txt"
        $minimumFree = [long](Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $startInfo = New-Object Diagnostics.ProcessStartInfo
        $startInfo.FileName = (Get-Command cargo).Source
        $startInfo.Arguments = $arguments -join ' '
        $startInfo.WorkingDirectory = $PSScriptRoot
        $startInfo.UseShellExecute = $false
        $startInfo.CreateNoWindow = $true
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $buildProcess = [Diagnostics.Process]::Start($startInfo)
        $stdoutTask = $buildProcess.StandardOutput.ReadToEndAsync()
        $stderrTask = $buildProcess.StandardError.ReadToEndAsync()
        try {
            while (-not $buildProcess.WaitForExit(1000)) {
                if ($watch.Elapsed.TotalMinutes -gt 10) { throw "Clean $name build exceeded 10 minutes" }
                $minimumFree = [Math]::Min($minimumFree, [long](Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory)
            }
            $buildProcess.WaitForExit()
        } finally {
            if (-not $buildProcess.HasExited) { & taskkill /PID $buildProcess.Id /T /F | Out-Null }
        }
        $watch.Stop()
        [IO.File]::WriteAllText($outLog, $stdoutTask.GetAwaiter().GetResult(), (New-Object Text.UTF8Encoding $false))
        [IO.File]::WriteAllText($errLog, $stderrTask.GetAwaiter().GetResult(), (New-Object Text.UTF8Encoding $false))
        if ($buildProcess.ExitCode -ne 0) { throw "Clean $name build failed; inspect $errLog" }
        $exe = Join-Path $PSScriptRoot "$target/release/ara-s4-netcode.exe"
        $symbols = (Get-ChildItem "$target/release" -Filter '*.pdb' | Measure-Object Length -Sum).Sum
        $builds += [ordered]@{stack=$name;seconds=$watch.Elapsed.TotalSeconds;exe_bytes=(Get-Item $exe).Length;pdb_bytes=$symbols;sha256=(Get-FileHash $exe -Algorithm SHA256).Hash;minimum_free_ram_kb=$minimumFree;target=$target;executable=$exe;arguments=$arguments}
        Write-Output "$name clean build: $([Math]::Round($watch.Elapsed.TotalSeconds,2)) s, $((Get-Item $exe).Length) executable bytes, min free RAM $minimumFree KiB"
        Save-JsonOutput $exe "artifacts/rust199/local-$name.json" @('local')
        Save-JsonOutput $exe "artifacts/rust199/bench-$name.json" @('bench')
    }
    $buildReport = [ordered]@{rustc=(& rustc -vV) -join "`n";cargo=(& cargo --version);jobs=4;incremental=$false;registry_cached=$true;os=(Get-CimInstance Win32_OperatingSystem).Caption;cpu=(Get-CimInstance Win32_Processor).Name.Trim();ram_bytes=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory;builds=$builds}
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'artifacts/rust199/builds.json'), ($buildReport | ConvertTo-Json -Depth 6) + "`n", (New-Object Text.UTF8Encoding $false))

    foreach ($stack in @('renet','lightyear')) {
        $exe = ($builds | Where-Object {$_.stack -eq $stack}).executable
        foreach ($mode in @('dedicated3','dedicated4','listen','dedicated4-live')) {
            & $exe harness $mode impaired 21668 "artifacts/rust199/$stack-$mode-impaired.json" --stack $stack
            if ($LASTEXITCODE -ne 0) { throw "$stack $mode failed" }
        }
        & $exe harness dedicated4 clean 21668 "artifacts/rust199/$stack-dedicated4-clean.json" --stack $stack
        if ($LASTEXITCODE -ne 0) { throw "$stack clean mode failed" }
        $measured = Get-Content -Raw "artifacts/rust199/$stack-dedicated4-impaired.json" | ConvertFrom-Json
        & $exe replay (Join-Path $measured.runtime_artifacts 'replay.json')
        if ($LASTEXITCODE -ne 0) { throw "$stack persisted replay failed" }
    }
    foreach ($stack in @('renet','lightyear')) {
        $tree = & cargo tree --locked --offline --no-default-features --features "$stack-stack" --edges normal --prefix none --format '{p}'
        if ($LASTEXITCODE -ne 0) { throw "dependency tree failed" }
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot "artifacts/rust199/dependencies-$stack.txt"), ($tree -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
    }
    $tree = & cargo tree --locked --offline --no-default-features --edges normal --prefix none --format '{p}'
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'artifacts/rust199/dependencies-local.txt'), ($tree -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
    & python (Join-Path $PSScriptRoot 'summarize.py')
    if ($LASTEXITCODE -ne 0) { throw 'Comparison summary assertions failed' }
} finally {
    Pop-Location
}

param(
    [string]$Executable,
    [string]$Output = "$PSScriptRoot/../bench-results",
    [int]$Rounds = 2,
    [string[]]$Variants = @('baseline','tuned'),
    [string[]]$Workloads = @('w1','w2','w3','w4')
)
$ErrorActionPreference = 'Stop'
if (-not $Executable) { throw 'Pass the executable printed by cargo bench --no-run.' }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
New-Item -ItemType Directory -Force $Output | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
function Read-Load {
    $cpu = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'"
    $os = Get-CimInstance Win32_OperatingSystem
    $processes = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
        Where-Object { $_.Name -ne '_Total' -and $_.Name -ne 'Idle' } |
        Sort-Object PercentProcessorTime -Descending |
        Select-Object -First 8 Name, IDProcess, PercentProcessorTime
    [ordered]@{ at = [DateTime]::UtcNow.ToString('o'); cpuPercent = $cpu.PercentProcessorTime; freeKiB = $os.FreePhysicalMemory; processes = @($processes) }
}
$cases = @(@('w1',1000000,4096), @('w2',20000,128), @('w2',50000,128), @('w3',1000000,4096), @('w4',100000,1024))
for ($round=1; $round -le $Rounds; $round++) {
    $directory = Join-Path $Output "run$round"
    New-Item -ItemType Directory -Force $directory | Out-Null
    $order = @($cases)
    if ($round % 2 -eq 0) { [array]::Reverse($order) }
    foreach ($case in $order) {
        if ($case[0] -notin $Workloads) { continue }
        foreach ($threads in @(1,16)) {
            $variantOrder = @($Variants)
            if ($round % 2 -eq 0) { [array]::Reverse($variantOrder) }
            foreach ($variant in $variantOrder) {
                $workload,$count,$batch = $case
                $name = "${workload}_${count}_t${threads}_${variant}"
                $result = Join-Path $directory "$name.json"
                $stdout = Join-Path $directory "$name.stdout.txt"
                $warmup = if ($workload -eq 'w3') { 3000 } else { 120 }
                $load = [Collections.Generic.List[object]]::new()
                $load.Add((Read-Load))
                $arguments = @($workload,$count,$threads,$warmup,300,$variant,$batch,"`"$result`"")
                $process = Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout
                $null = $process.Handle
                # Benchmark process only; background applications are not modified.
                try { $process.PriorityClass = 'AboveNormal' } catch { Write-Warning $_ }
                while (-not $process.HasExited) {
                    Start-Sleep -Milliseconds 800
                    if (-not $process.HasExited) { $load.Add((Read-Load)) }
                }
                $process.WaitForExit()
                if (($null -ne $process.ExitCode -and $process.ExitCode -ne 0) -or -not (Test-Path -LiteralPath $result)) { throw "Benchmark $name failed ($($process.ExitCode))" }
                $load.Add((Read-Load))
                [IO.File]::WriteAllText((Join-Path $directory "$name.load.json"), (@{priority='AboveNormal'; samples=@($load.ToArray())} | ConvertTo-Json -Depth 6))
                Get-Content -LiteralPath $stdout
            }
        }
    }
}

param(
    [ValidateSet('Build','Validate','Measure')][string]$Phase = 'Measure',
    [string[]]$Setups = @('rust','jit','aot','unity'),
    [string[]]$Cases = @('w1:1000000','w2:20000','w2:50000','w3:1000000','w4:100000'),
    [int[]]$Threads = @(1,16),
    [int]$Warmup = 120,
    [int]$Frames = 300,
    [string]$Run = 'run1',
    [string]$Unity = 'C:/Program Files/Unity/Hub/Editor/6000.6.6f1/Editor/Unity.exe',
    [switch]$EditorFallback,
    [switch]$Dump
)
$ErrorActionPreference = 'Stop'
$s8Root = $PSScriptRoot
$env:CARGO_BUILD_JOBS = '4'
$env:RUSTFLAGS = '-C target-cpu=native'
$env:PATH = "$s8Root/target/release;${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer;" + $env:PATH
New-Item -ItemType Directory -Force "$s8Root/out" | Out-Null

# Launch only task-owned processes, wait with a finite deadline, and capture all output.
function Invoke-S8([string]$Exe, [string[]]$Arguments, [string]$Log, [int]$TimeoutSeconds = 900) {
    $quoted = $Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_.Replace('"','\"') + '"' } else { $_ } }
    $s8Child = Start-Process -FilePath $Exe -ArgumentList $quoted -WorkingDirectory $s8Root -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Log.stdout" -RedirectStandardError "$Log.stderr"
    # Keep the OS handle open: otherwise Windows PowerShell can lose ExitCode on exit.
    $null = $s8Child.Handle
    if (-not $s8Child.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $s8Child.Id -Force
        throw "Timed out: $Exe; logs: $Log"
    }
    # Drain redirected output after the bounded wait completes.
    $s8Child.WaitForExit()
    if ($s8Child.ExitCode -ne 0) {
        Get-Content "$Log.stdout", "$Log.stderr" -Tail 40
        throw "Exit $($s8Child.ExitCode): $Exe; logs: $Log"
    }
    return $s8Child.Id
}

if ($Phase -eq 'Build') {
    if ($Setups -contains 'rust' -or $Setups -contains 'jit' -or $Setups -contains 'aot') {
        & cargo build --release --manifest-path "$s8Root/Cargo.toml"
        if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    }
    if ($Setups -contains 'jit') {
        & dotnet build "$s8Root/managed/Managed.csproj" -c Release --nologo --disable-build-servers -warnaserror
        if ($LASTEXITCODE -ne 0) { throw 'JIT build failed' }
    }
    if ($Setups -contains 'aot') {
        & dotnet publish "$s8Root/managed/Managed.csproj" -c Release -r win-x64 -p:PublishAot=true --disable-build-servers -o "$s8Root/out/aot" --nologo
        if ($LASTEXITCODE -ne 0) { throw 'NativeAOT publish failed' }
    }
    if ($Setups -contains 'unity') {
        Invoke-S8 $Unity @('-batchmode','-nographics','-quit','-projectPath',"$s8Root/unity",'-executeMethod','S8Unity.Build.Player','-logFile',"$s8Root/out/unity-build.log") "$s8Root/out/unity-build" 1800 | Out-Null
    }
    return
}

if ($Phase -eq 'Validate') { $Warmup = 0; $Frames = 10 }
$s8Destination = if ($Phase -eq 'Validate') { "$s8Root/results/validation/$Run" } else { "$s8Root/results/$Run" }
New-Item -ItemType Directory -Force $s8Destination | Out-Null
foreach ($threadCount in $Threads) {
    foreach ($case in $Cases) {
        $parts = $case.Split(':'); $workload = $parts[0]; $count = [int]$parts[1]
        foreach ($setup in $Setups) {
            $variants = if ($workload -eq 'w3' -and $setup -in @('jit','aot')) { @('w3','w3simd') } else { @($workload) }
            foreach ($variant in $variants) {
                $name = "${setup}_${variant}_${count}_t${threadCount}"
                $json = "$s8Destination/$name.json"
                $dumpPath = if ($Phase -eq 'Validate' -or $Dump) { "$s8Root/out/${Run}_$name.bin" } else { '-' }
                $begin = Get-Date
                $before = @{}; Get-Process | ForEach-Object { $before[$_.Id] = $_.CPU }
                $os = Get-CimInstance Win32_OperatingSystem
                $common = @($variant, "$count", "$threadCount", "$Warmup", "$Frames", $json, $dumpPath)
                Write-Output "Starting $Phase $name"
                switch ($setup) {
                    'rust' { $childId = Invoke-S8 "$s8Root/target/release/s8-dots-bench.exe" $common "$s8Root/out/$name" }
                    'jit' { $childId = Invoke-S8 (Get-Command dotnet).Source (@("$s8Root/managed/bin/Release/net10.0/Managed.dll") + $common) "$s8Root/out/$name" }
                    'aot' { $childId = Invoke-S8 "$s8Root/out/aot/Managed.exe" $common "$s8Root/out/$name" }
                    'unity' {
                        if ($EditorFallback) {
                            $childId = Invoke-S8 $Unity (@('-batchmode','-nographics','-projectPath',"$s8Root/unity",'-executeMethod','S8Unity.Build.EditorBenchmark','-logFile',"$s8Root/out/$name.log",'-s8') + $common) "$s8Root/out/$name"
                        } else {
                            $childId = Invoke-S8 "$s8Root/unity/Build/S8.exe" (@('-batchmode','-nographics','-logFile',"$s8Root/out/$name.log",'-s8') + $common) "$s8Root/out/$name"
                        }
                    }
                    default { throw "Unknown setup $setup" }
                }
                $deltas = @(Get-Process | ForEach-Object {
                    if ($_.Id -ne $childId -and $before.ContainsKey($_.Id)) {
                        $delta = $_.CPU - $before[$_.Id]
                        if ($delta -gt 0) { [PSCustomObject]@{name=$_.ProcessName; pid=$_.Id; cpu_seconds=$delta} }
                    }
                })
                $duration = ((Get-Date) - $begin).TotalSeconds
                $background = ($deltas | Measure-Object -Property cpu_seconds -Sum).Sum
                [PSCustomObject]@{ started=$begin.ToString('o'); wall_seconds=$duration; free_memory_kib=$os.FreePhysicalMemory;
                    background_cpu_percent=100.0*$background/($duration*16); processes=@($deltas | Sort-Object cpu_seconds -Descending | Select-Object -First 12) } |
                    ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 "$s8Destination/$name.load.json"
                if (-not (Test-Path $json)) { throw "Runner produced no JSON: $json" }
                $row = Get-Content $json -Raw | ConvertFrom-Json
                Write-Output ("{0}: p50={1:N3} p95={2:N3} ms; background={3:N1}%" -f $name,$row.p50_ms,$row.p95_ms,(100.0*$background/($duration*16)))
            }
        }
    }
}

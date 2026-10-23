# Shared process/provenance handling. Each aggregate names its cases explicitly.
function Clear-AggregationMode {
    Remove-Item Env:ARA_PHYSICS_BENCH_AGGREGATE_DIR, Env:ARA_PHYSICS_BENCH_CASES -ErrorAction SilentlyContinue
}
function Write-JsonInput([string]$Path, $Value) {
    # Intermediate metadata is canonicalized by Ara's writer at publication.
    [System.IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 64), [System.Text.UTF8Encoding]::new($false))
}
function New-BenchmarkRun([string]$Repo, [string]$Results, [switch]$IncludeMatrix) {
    Clear-AggregationMode
    $runId = [Guid]::NewGuid().ToString('N')
    $cases = [System.Collections.Generic.List[string]]::new()
    if ($IncludeMatrix) {
        $matrix = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'RESULTS.json') -Raw | ConvertFrom-Json
        if (-not $matrix.runId -or -not $matrix.caseNames) { throw 'A completed matrix manifest is required' }
        $runId = $matrix.runId
        foreach ($name in $matrix.caseNames) { $cases.Add($name) }
    }
    $env:ARA_PHYSICS_BENCH_RUN_ID = $runId
    return [pscustomobject]@{ runId = $runId; sourceCommit = (& git -C $Repo rev-parse HEAD); cases = $cases; launches = [System.Collections.Generic.List[object]]::new() }
}
function Host-Load {
    $cpu = Get-CimInstance Win32_Processor
    $os = Get-CimInstance Win32_OperatingSystem
    return [ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); cpuPercent = $cpu.LoadPercentage; freeMemoryKiB = $os.FreePhysicalMemory; processCount = (Get-Process).Count }
}
function Invoke-Benchmark($Run, [string]$Repo, [string]$Results, [string]$Binary, [string]$Name) {
    Clear-AggregationMode
    $binaryPath = (Resolve-Path -LiteralPath $Binary).Path
    $before = Host-Load
    $hash = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash
    $started = [DateTime]::UtcNow
    $env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $Results "$Name.json"
    $process = Start-Process -FilePath $binaryPath -WorkingDirectory $Repo -WindowStyle Hidden -RedirectStandardOutput (Join-Path $Results "$Name.log") -RedirectStandardError (Join-Path $Results "$Name.stderr.log") -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Benchmark failed: $Name" }
    $artifact = Get-Item -LiteralPath $env:ARA_PHYSICS_BENCH_OUTPUT
    if ($artifact.LastWriteTimeUtc -lt $started) { throw "Stale output: $Name" }
    $data = Get-Content -LiteralPath $artifact.FullName -Raw | ConvertFrom-Json
    if ($data.PSObject.Properties['complete'] -and -not $data.complete) { throw "Incomplete output: $Name" }
    if ($data.PSObject.Properties['runId'] -and $data.runId -ne $Run.runId) { throw "Wrong run ID: $Name" }
    $Run.cases.Add($Name)
    $Run.launches.Add([ordered]@{ name = $Name; runId = $Run.runId; sourceCommit = $Run.sourceCommit; binarySha256 = $hash; startedUtc = $started.ToString('o'); loadBefore = $before; loadAfter = (Host-Load) })
}
function Publish-BenchmarkRun($Run, [string]$Bins, [string]$Results, [string]$MetadataName) {
    Write-JsonInput (Join-Path $Results "$MetadataName.json") ([ordered]@{ runId = $Run.runId; sourceCommit = $Run.sourceCommit; launches = @($Run.launches.ToArray()) })
    $Run.cases.Add($MetadataName)
    $env:ARA_PHYSICS_BENCH_AGGREGATE_DIR = $Results
    $env:ARA_PHYSICS_BENCH_CASES = $Run.cases -join ','
    $env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $PSScriptRoot 'RESULTS.json'
    try {
        & (Join-Path $Bins 'deterministic.exe')
        if ($LASTEXITCODE -ne 0) { throw 'Result aggregation failed' }
    } finally { Clear-AggregationMode }
}

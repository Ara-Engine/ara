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
        $matrixProvenance = $matrix.cases.'matrix-provenance'
        if (-not $matrix.runId -or -not $matrixProvenance.launches) { throw 'A completed matrix manifest is required' }
        # Upgrade launcher-only receipts from the first harness version. Source
        # mappings are explicit sidecars, never inferred from the launcher HEAD.
        if ($matrixProvenance.PSObject.Properties['sourceCommit']) {
            $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
            $builds = @{}
            foreach ($sidecar in (Get-ChildItem (Join-Path $target 'physics-bench') -Filter '*.build.json')) {
                $build = Get-Content -LiteralPath $sidecar.FullName -Raw | ConvertFrom-Json
                $builds[$build.binarySha256] = $build
            }
            $matrixProvenance | Add-Member -NotePropertyName launcherRevision -NotePropertyValue $matrixProvenance.sourceCommit
            $matrixProvenance.PSObject.Properties.Remove('sourceCommit')
            foreach ($launch in $matrixProvenance.launches) {
                $launch | Add-Member -NotePropertyName launcherRevision -NotePropertyValue $launch.sourceCommit
                $launch.PSObject.Properties.Remove('sourceCommit')
                $launch | Add-Member -NotePropertyName build -NotePropertyValue $builds[$launch.binarySha256]
            }
            $matrixProvenance | Add-Member -NotePropertyName receiptNormalization -NotePropertyValue 'Launcher field renamed; retrospective build mappings are labelled inside each build object.'
            Write-JsonInput (Join-Path $Results 'matrix-provenance.json') $matrixProvenance
        }
        $runId = $matrix.runId
        # Repeated comparisons extend only the matrix, never a prior comparison.
        foreach ($launch in $matrixProvenance.launches) {
            if ($cases.Contains($launch.name)) { throw 'Duplicate matrix launch' }
            $cases.Add($launch.name)
        }
        $cases.Add('matrix-provenance')
    }
    $env:ARA_PHYSICS_BENCH_RUN_ID = $runId
    return [pscustomobject]@{ runId = $runId; launcherRevision = (& git -C $Repo rev-parse HEAD); cases = $cases; launches = [System.Collections.Generic.List[object]]::new() }
}
function Get-BinaryBuild([string]$Binary, [string]$Hash) {
    $sidecar = "$Binary.build.json"
    if (-not (Test-Path -LiteralPath $sidecar)) { return $null }
    $build = Get-Content -LiteralPath $sidecar -Raw | ConvertFrom-Json
    if ($build.binarySha256 -ne $Hash) { throw "Build metadata hash mismatch: $Binary" }
    return $build
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
    $build = Get-BinaryBuild $binaryPath $hash
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
    $Run.launches.Add([ordered]@{ name = $Name; runId = $Run.runId; launcherRevision = $Run.launcherRevision; build = $build; binarySha256 = $hash; startedUtc = $started.ToString('o'); loadBefore = $before; loadAfter = (Host-Load) })
}
function Publish-BenchmarkRun($Run, [string]$Bins, [string]$Results, [string]$MetadataName) {
    Write-JsonInput (Join-Path $Results "$MetadataName.json") ([ordered]@{ runId = $Run.runId; launcherRevision = $Run.launcherRevision; launches = @($Run.launches.ToArray()) })
    $Run.cases.Add($MetadataName)
    $env:ARA_PHYSICS_BENCH_AGGREGATE_DIR = $Results
    $env:ARA_PHYSICS_BENCH_CASES = $Run.cases -join ','
    $env:ARA_PHYSICS_BENCH_OUTPUT = Join-Path $PSScriptRoot 'RESULTS.json'
    try {
        & (Join-Path $Bins 'deterministic.exe')
        if ($LASTEXITCODE -ne 0) { throw 'Result aggregation failed' }
    } finally { Clear-AggregationMode }
}

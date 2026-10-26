# Installed startup A/B. Run through ara-dev gpu; the children never open windows.
param([Parameter(Mandatory=$true)][string]$Setup,
      [Parameter(Mandatory=$true)][string]$Directory,
      [int]$Runs=5)
$ErrorActionPreference='Stop'
$target=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../target')).TrimEnd('\')
$root=[IO.Path]::GetFullPath($Directory).TrimEnd('\')
if(!$root.StartsWith($target+'\',[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $root)) { throw 'Use a fresh directory under this checkout target' }
$Setup=(Resolve-Path -LiteralPath $Setup).ProviderPath
New-Item -ItemType Directory -Path $root | Out-Null
$measurements=@()
function Run-Hidden([string]$Exe,[string[]]$Arguments,[string]$Log,[string]$Cwd) {
    $start=New-Object Diagnostics.ProcessStartInfo
    $start.FileName=$Exe
    # Arguments here are flags and Windows paths, never shell commands. Quotes cannot occur in paths.
    $start.Arguments=($Arguments | ForEach-Object {'"'+$_+'"'}) -join ' '
    $start.WorkingDirectory=$Cwd
    $start.UseShellExecute=$false
    $start.CreateNoWindow=$true
    $start.RedirectStandardOutput=$true
    $start.RedirectStandardError=$true
    $process=New-Object Diagnostics.Process
    $process.StartInfo=$start
    $launched=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    if(!$process.Start()){ throw 'Process failed to start' }
    $stdout=$process.StandardOutput.ReadToEndAsync()
    $stderr=$process.StandardError.ReadToEndAsync()
    if(!$process.WaitForExit(120000)){ $process.Kill(); $process.WaitForExit(); throw 'Startup probe timed out' }
    [IO.File]::WriteAllText($Log,$stdout.Result+$stderr.Result)
    if($process.ExitCode -ne 0){ throw "Process failed; see $Log" }
    $process.Dispose()
    return $launched
}
for($index=1;$index -le $Runs;$index++) {
    $case=Join-Path $root "pair-$index"
    foreach($name in @('local','roaming','config','crashes','empty-layers')) {
        New-Item -ItemType Directory -Path (Join-Path $case $name) -Force | Out-Null
    }
    $env:ARA_CONFIG_DIR=Join-Path $case 'config'
    $env:ARA_CRASH_DIR=Join-Path $case 'crashes'
    $env:LOCALAPPDATA=Join-Path $case 'local'
    $env:APPDATA=Join-Path $case 'roaming'
    $env:VK_IMPLICIT_LAYER_PATH=Join-Path $case 'empty-layers'
    Remove-Item Env:GITHUB_TOKEN,Env:GH_TOKEN,Env:ARA_RELEASE_TEST_API,Env:ARA_HUB_STARTUP_TRACE -ErrorAction SilentlyContinue
    $hub=Join-Path $env:LOCALAPPDATA 'Programs/Ara Hub'
    try {
        $null=Run-Hidden $Setup @('--silent','--no-system-integration','--destination',$hub) (Join-Path $case 'install.log') $case
        foreach($mode in @('cold','warm')) {
            $output=Join-Path $case $mode
            New-Item -ItemType Directory -Path $output | Out-Null
            $env:ARA_HUB_STARTUP_TRACE=Join-Path $output 'trace.jsonl'
            $launched=Run-Hidden (Join-Path $hub 'ara-hub.exe') @('--startup-probe',$output,'--size','1440x900','--dpi','1') (Join-Path $output 'process.log') $hub
            $report=Get-Content (Join-Path $output 'startup.json') -Raw | ConvertFrom-Json
            $measurements+= [pscustomobject]@{pair=$index;mode=$mode;firstFrameMs=($report.firstFrameUnixMs-$launched);connectedMs=($report.connectedUnixMs-$launched);inputMs=$report.inputWhileConnectingMs;output=$output;build=$report.build}
            $measurements | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $root 'measurements.json') -Encoding UTF8
        }
    } finally {
        # Only the service in this freshly owned profile can be stopped.
        $discovery=Join-Path $env:ARA_CONFIG_DIR 'hub/.ara/daemon.json'
        if(Test-Path -LiteralPath $discovery) {
            $service=Get-Content -LiteralPath $discovery -Raw | ConvertFrom-Json
            $owned=Get-CimInstance Win32_Process -Filter "ProcessId=$($service.pid)"
            if(!$owned -or !$owned.ExecutablePath.StartsWith($case+'\',[StringComparison]::OrdinalIgnoreCase)){ throw 'Refusing to stop a process outside this test profile' }
            $null=Run-Hidden $owned.ExecutablePath @('hub','shutdown') (Join-Path $case 'shutdown.log') $case
            $running=Get-Process -Id $service.pid -ErrorAction SilentlyContinue
            if($running -and !$running.WaitForExit(10000)){ throw 'Owned Hub service did not exit' }
        }
    }
}
$measurements | Select-Object pair,mode,firstFrameMs,connectedMs,inputMs | Format-Table -AutoSize

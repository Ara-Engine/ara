param([string]$Root, [string]$Release, [string]$HubInstall)
$ErrorActionPreference = 'Stop'
$source = Join-Path $Root 'source [engine]'
$destination = Join-Path $Root 'installed [engine]'
[IO.Directory]::CreateDirectory($source) | Out-Null
[IO.Directory]::CreateDirectory($env:ARA_CONFIG_DIR) | Out-Null
[IO.Directory]::CreateDirectory((Join-Path $env:ARA_CONFIG_DIR 'hub\.ara')) | Out-Null
# A local fixture records CLI calls and exits; it never starts a Hub or daemon.
Add-Type -OutputAssembly (Join-Path $Root 'fixture.exe') -OutputType ConsoleApplication -TypeDefinition @'
using System;
using System.IO;
public static class FixtureAra {
    public static int Main(string[] args) {
        File.AppendAllText(Path.Combine(Environment.GetEnvironmentVariable("ARA_CONFIG_DIR"), "calls.txt"), String.Join(" ", args) + "\n");
        if (args.Length > 1 && args[1] == "unregister-engine" && Environment.GetEnvironmentVariable("ARA_TEST_REFUSE_UNINSTALL") == "1") return 1;
        return 0;
    }
}
'@
Copy-Item -LiteralPath (Join-Path $Root 'fixture.exe') -Destination (Join-Path $source 'ara.exe')
foreach ($file in @('install.ps1','uninstall.ps1','launch-hub.ps1','launch-editor.ps1','paths.ps1','hub-installation.ps1')) {
    $path = Join-Path $Release $file
    if (Test-Path -LiteralPath $path) { Copy-Item -LiteralPath $path -Destination (Join-Path $source $file) }
}
Set-Content -LiteralPath (Join-Path $source 'engine.json') -Value '{"format":"ara.engine/1","version":"0.0.1-alpha"}'
Set-Content -LiteralPath (Join-Path $env:ARA_CONFIG_DIR 'hub\.ara\daemon.json') -Value '{"pid":2147483647}'
$files = @(Get-ChildItem -LiteralPath $source -File | ForEach-Object { @{path=$_.Name} })
@{version='0.0.1-alpha';files=$files} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $source 'files.json')
$verbatimSource = '\\?\' + $source
$verbatimDestination = '\\?\' + $destination
& ($verbatimSource + '\install.ps1') -Destination $verbatimDestination -Silent -NoSystemIntegration
if (-not (Test-Path -LiteralPath (Join-Path $destination 'install-receipt.json'))) { throw 'Missing install receipt' }
$receipt = Get-Content -LiteralPath (Join-Path $destination 'install-receipt.json') -Raw | ConvertFrom-Json
if ($receipt.root -ne $destination -or $receipt.integration) { throw 'Receipt did not use the normal isolated path' }
# Receipts written by an earlier verbatim-path install must remain uninstallable.
$receipt.root = $verbatimDestination
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destination 'install-receipt.json')
function Invoke-UninstallCmd {
    $info=New-Object Diagnostics.ProcessStartInfo
    $info.FileName=$env:ComSpec
    $info.Arguments='/d /s /c ""'+(Join-Path $destination 'uninstall.cmd')+'" -Silent"'
    $info.WorkingDirectory=$Root
    $info.UseShellExecute=$false
    $info.CreateNoWindow=$true
    $info.RedirectStandardOutput=$true
    $info.RedirectStandardError=$true
    $process=[Diagnostics.Process]::Start($info)
    $stdout=$process.StandardOutput.ReadToEndAsync()
    $stderr=$process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    [IO.File]::WriteAllText((Join-Path $Root 'uninstall-output.log'),$stdout.Result+$stderr.Result)
    $result=$process.ExitCode
    $process.Dispose()
    return $result
}
$env:ARA_TEST_REFUSE_UNINSTALL='1'
if ((Invoke-UninstallCmd) -ne 1) { throw 'A refused uninstall must exit 1' }
if (-not (Test-Path -LiteralPath (Join-Path $destination 'ara.exe'))) { throw 'Refused uninstall removed the engine' }
Remove-Item Env:ARA_TEST_REFUSE_UNINSTALL
$project=Join-Path $Root 'Keep my project'
[IO.Directory]::CreateDirectory($project) | Out-Null
Set-Content -LiteralPath (Join-Path $project 'user.txt') -Value 'keep project'
Set-Content -LiteralPath (Join-Path $env:ARA_CONFIG_DIR 'hub-ui.json') -Value 'keep settings'
if ((Invoke-UninstallCmd) -ne 0) { throw 'Successful uninstall.cmd must exit 0' }
if (Test-Path -LiteralPath $destination) { throw 'Uninstall left owned files behind' }
if ((Get-Content -LiteralPath (Join-Path $project 'user.txt')) -ne 'keep project') { throw 'Uninstall removed user project' }
if ((Get-Content -LiteralPath (Join-Path $env:ARA_CONFIG_DIR 'hub-ui.json')) -ne 'keep settings') { throw 'Uninstall changed settings' }
$calls = @(Get-Content -LiteralPath (Join-Path $env:ARA_CONFIG_DIR 'calls.txt'))
if (($calls -join '|') -ne 'hub register-engine --no-start|doctor|hub unregister-engine --no-start|hub unregister-engine --no-start') { throw "Unexpected CLI calls: $calls" }

$hubSource = Join-Path $Root 'source [hub]'
$hubDestination = Join-Path $Root 'installed [hub]'
[IO.Directory]::CreateDirectory($hubSource) | Out-Null
Copy-Item -LiteralPath $HubInstall -Destination (Join-Path $hubSource 'install.ps1')
Copy-Item -LiteralPath (Join-Path $Release 'hub-installation.ps1') -Destination (Join-Path $hubSource 'hub-installation.ps1')
$helper = Join-Path $Release 'paths.ps1'
if (Test-Path -LiteralPath $helper) { Copy-Item -LiteralPath $helper -Destination (Join-Path $hubSource 'paths.ps1') }
Set-Content -LiteralPath (Join-Path $hubSource 'hub.json') -Value '{"format":"ara.hub/1","version":"0.0.1-alpha"}'
& ('\\?\' + $hubSource + '\install.ps1') -Destination ('\\?\' + $hubDestination) -Silent -NoSystemIntegration
if (-not (Test-Path -LiteralPath (Join-Path $hubDestination 'hub.json'))) { throw 'Hub not installed' }

# These conversions are lexical: no SMB connection or share is needed.
. $helper
foreach ($pair in @(
    @('\\?\C:\Ara [preview]\engine','C:\Ara [preview]\engine'),
    @('\\server\share\Ara [preview]','\\server\share\Ara [preview]'),
    @('\\?\UNC\server\share\Ara [preview]','\\server\share\Ara [preview]')
)) {
    if ((ConvertTo-AraPath $pair[0]) -ne $pair[1]) { throw "Incorrect Windows path normalization: $($pair[0])" }
}
Write-Host 'Verbatim install, uninstall, standalone Hub and UNC normalization passed.'

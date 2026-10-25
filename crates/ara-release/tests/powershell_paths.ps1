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
        return 0;
    }
}
'@
Copy-Item -LiteralPath (Join-Path $Root 'fixture.exe') -Destination (Join-Path $source 'ara.exe')
foreach ($file in @('install.ps1','uninstall.ps1','launch-hub.ps1','launch-editor.ps1','paths.ps1')) {
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
& ($verbatimDestination + '\uninstall.ps1') -Silent
if (Test-Path -LiteralPath $destination) { throw 'Uninstall left owned files behind' }
$calls = @(Get-Content -LiteralPath (Join-Path $env:ARA_CONFIG_DIR 'calls.txt'))
if (($calls -join '|') -ne 'hub register-engine|doctor|hub unregister-engine|hub shutdown') { throw "Unexpected CLI calls: $calls" }

$hubSource = Join-Path $Root 'source [hub]'
$hubDestination = Join-Path $Root 'installed [hub]'
[IO.Directory]::CreateDirectory($hubSource) | Out-Null
Copy-Item -LiteralPath $HubInstall -Destination (Join-Path $hubSource 'install.ps1')
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

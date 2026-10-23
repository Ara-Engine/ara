[CmdletBinding()]
param([switch]$Silent)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($PSScriptRoot)
$receipt = Get-Content -LiteralPath (Join-Path $root 'install-receipt.json') -Encoding UTF8 -Raw | ConvertFrom-Json
$manifest = Get-Content -LiteralPath (Join-Path $root 'files.json') -Encoding UTF8 -Raw | ConvertFrom-Json
if ($receipt.format -ne 'ara.install/1' -or [IO.Path]::GetFullPath($receipt.root) -ne $root -or $receipt.version -ne $manifest.version -or $receipt.version -notmatch '^[0-9A-Za-z.-]+$') { throw 'Refusing to uninstall: installation receipt does not match this directory' }
# Validate every target before touching the Hub or files; never traverse a reparse point.
$paths = @($manifest.files | ForEach-Object { $_.path }) + @('files.json','install-receipt.json','uninstall.cmd')
$targets = foreach ($relative in $paths) {
    $path = [IO.Path]::GetFullPath((Join-Path $root $relative))
    if (-not $path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe uninstall path' }
    $parent = $path
    while ($parent -and $parent.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        if ((Test-Path -LiteralPath $parent) -and ((Get-Item -LiteralPath $parent -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Refusing to uninstall through a reparse point' }
        if ($parent -eq $root) { break }
        $parent = Split-Path -Parent $parent
    }
    $path
}
# An open Hub window would restart the service from this installation; close it first.
$hubWindows = @(Get-Process -Name 'ara-hub' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase) })
if ($hubWindows.Count -gt 0) { throw 'Close the Ara Hub window before uninstalling' }
$ara = Join-Path $root 'ara.exe'
& $ara hub unregister-engine
if ($LASTEXITCODE -ne 0) { throw 'Close projects using this engine before uninstalling' }
$configRoot = if ($env:ARA_CONFIG_DIR) { $env:ARA_CONFIG_DIR } else { Join-Path $env:APPDATA 'Ara' }
$discovery = Get-Content -LiteralPath (Join-Path $configRoot 'hub\.ara\daemon.json') -Encoding UTF8 -Raw | ConvertFrom-Json
$hubProcess = Get-Process -Id $discovery.pid -ErrorAction SilentlyContinue
# A Hub hosted by this installation holds ara.exe open. Shutdown preserves projects.
& $ara hub shutdown
if ($LASTEXITCODE -ne 0) { throw 'Could not stop the Hub; installation retained' }
if ($hubProcess -and -not $hubProcess.WaitForExit(10000)) { throw 'Hub did not exit; installation retained for retry' }
if ($receipt.integration) {
    $expected = Join-Path ([Environment]::GetFolderPath('Programs')) ('Ara\' + $receipt.version)
    if ($receipt.shortcuts -ne $expected) { throw 'Invalid shortcut directory in receipt' }
    foreach ($app in @('Hub','Editor')) { $link = Join-Path $expected ("Ara $app.lnk"); if (Test-Path -LiteralPath $link) { Remove-Item -LiteralPath $link -Force } }
    if ((Test-Path -LiteralPath $expected) -and -not (Get-ChildItem -LiteralPath $expected -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $expected }
    $entries = @(([Environment]::GetEnvironmentVariable('Path','User') -split ';') | Where-Object { $_ -and $_ -ne $root })
    [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Ara-' + $receipt.version
    if (Test-Path -LiteralPath $key) { Remove-Item -LiteralPath $key -Recurse }
}
$ownedDirectories = @(foreach ($path in $targets) {
    $directory = Split-Path -Parent $path
    while ($directory -ne $root) { $directory; $directory = Split-Path -Parent $directory }
}) | Sort-Object -Unique | Sort-Object Length -Descending
foreach ($path in $targets) { if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force } }
# Visit only validated inventory parents; never traverse user-added directories.
foreach ($directory in $ownedDirectories) {
    if ((Test-Path -LiteralPath $directory) -and -not (Get-ChildItem -LiteralPath $directory -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $directory }
}
if (-not (Get-ChildItem -LiteralPath $root -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $root }
Write-Host 'Ara uninstalled. Projects, settings and local crash reports were preserved.'

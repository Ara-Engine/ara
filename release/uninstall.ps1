[CmdletBinding()]
param([switch]$Silent)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath($PSScriptRoot)
$receipt = Get-Content -LiteralPath (Join-Path $root 'install-receipt.json') -Raw | ConvertFrom-Json
$manifest = Get-Content -LiteralPath (Join-Path $root 'files.json') -Raw | ConvertFrom-Json
if ($receipt.format -ne 'ara.install/1' -or [IO.Path]::GetFullPath($receipt.root) -ne $root -or $receipt.version -ne $manifest.version) { throw 'Refusing to uninstall: installation receipt does not match this directory' }
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
$ara = Join-Path $root 'ara.exe'
& $ara hub unregister-engine
if ($LASTEXITCODE -ne 0) { throw 'Close projects using this engine before uninstalling' }
# A Hub hosted by this installation holds ara.exe open. Shutdown preserves projects.
& $ara hub shutdown
if ($LASTEXITCODE -ne 0) { throw 'Could not stop the Hub; installation retained' }
Start-Sleep -Milliseconds 500
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
foreach ($path in $targets) { if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force } }
# Remove empty directories only. User-added files are preserved.
Get-ChildItem -LiteralPath $root -Directory -Recurse -Force | Sort-Object { $_.FullName.Length } -Descending | ForEach-Object {
    if (-not (Get-ChildItem -LiteralPath $_.FullName -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $_.FullName }
}
if (-not (Get-ChildItem -LiteralPath $root -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $root }
Write-Host 'Ara uninstalled. Projects, settings and local crash reports were preserved.'

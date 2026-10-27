[CmdletBinding()]
param([string]$Destination, [switch]$Silent, [switch]$RegisterOnly, [switch]$NoSystemIntegration)
$ErrorActionPreference = 'Stop'
. ([IO.Path]::Combine($PSScriptRoot, 'paths.ps1'))
$source = ConvertTo-AraPath $PSScriptRoot
$engine = Get-Content -LiteralPath (Join-Path $source 'engine.json') -Encoding UTF8 -Raw | ConvertFrom-Json
if ($engine.format -ne 'ara.engine/1' -or $engine.version -notmatch '^[0-9A-Za-z.-]+$') { throw 'Invalid Ara engine manifest' }
if (-not $RegisterOnly) {
    if (-not $Destination) { $Destination = Join-Path (ConvertTo-AraPath $env:LOCALAPPDATA) ('Programs\Ara\' + $engine.version) }
    $Destination = ConvertTo-AraPath $Destination
    if (Test-Path -LiteralPath $Destination) { throw "Destination already exists: $Destination" }
    $parent = [IO.Path]::GetDirectoryName($Destination)
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $stage = Join-Path $parent ('.ara-install-' + [guid]::NewGuid().ToString('N'))
    try {
        [IO.Directory]::CreateDirectory($stage) | Out-Null
        Get-ChildItem -LiteralPath $source -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse }
        Move-Item -LiteralPath $stage -Destination $Destination
    } catch {
        if (Test-Path -LiteralPath $stage) {
            $resolved = ConvertTo-AraPath $stage
            if ([IO.Path]::GetDirectoryName($resolved) -ne $parent -or [IO.Path]::GetFileName($resolved) -notlike '.ara-install-*') { throw 'Unsafe staging cleanup' }
            Remove-Item -LiteralPath $resolved -Recurse -Force
        }
        throw
    }
    $source = $Destination
}
$ara = Join-Path $source 'ara.exe'
# The Vulkan SDK tools import these Microsoft CRT DLLs; do not copy system DLLs.
foreach ($dll in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
    if (-not (Test-Path -LiteralPath (Join-Path ([Environment]::SystemDirectory) $dll))) {
        throw 'Install Microsoft Visual C++ v14 Redistributable (x64), then rerun install.ps1 -RegisterOnly: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist'
    }
}
$registration = @('hub','register-engine')
if ($NoSystemIntegration) { $registration += '--no-start' }
& $ara @registration
if ($LASTEXITCODE -ne 0) { throw 'Hub engine registration failed; installation retained for retry' }
& $ara doctor
if ($LASTEXITCODE -ne 0) { Write-Warning 'Prerequisites missing. Run ara doctor and install the x64 .NET 10 SDK: https://dotnet.microsoft.com/en-us/download/dotnet/10.0' }
$shortcutDirectory = Join-Path (ConvertTo-AraPath ([Environment]::GetFolderPath('Programs'))) ('Ara\' + $engine.version)
$receipt = [ordered]@{ format='ara.install/1'; version=$engine.version; root=$source; integration=(-not $NoSystemIntegration); shortcuts=$shortcutDirectory }
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $source 'install-receipt.json') -Encoding UTF8
if (-not $NoSystemIntegration) {
    [IO.Directory]::CreateDirectory($shortcutDirectory) | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    # Shortcuts and the Apps entry use the icons embedded in each program's executable
    # (Hub icon in ara-hub.exe, engine icon in ara-editor.exe), never the legacy ara.ico.
    $nativeHub = Join-Path $source 'ara-hub.exe'
    $engineIcon = (Join-Path $source 'ara-editor.exe') + ',0'
    foreach ($app in @('Hub','Editor')) {
        $link = $shell.CreateShortcut((Join-Path $shortcutDirectory ("Ara $app.lnk")))
        $link.IconLocation = $engineIcon
        if ($app -eq 'Hub' -and (Test-Path -LiteralPath $nativeHub)) {
            $link.IconLocation = $nativeHub + ',0'
            # The native Hub is a GUI app: start it directly, without a console window.
            $link.TargetPath = $nativeHub
        } else {
            $link.TargetPath = Join-Path (ConvertTo-AraPath $env:SystemRoot) 'System32\WindowsPowerShell\v1.0\powershell.exe'
            $link.Arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $source ('launch-' + $app.ToLowerInvariant() + '.ps1')) + '"'
        }
        $link.WorkingDirectory = $source
        $link.Save()
    }
    $entries = @(([Environment]::GetEnvironmentVariable('Path','User') -split ';') | Where-Object { $_ })
    if ($entries -notcontains $source) { [Environment]::SetEnvironmentVariable('Path', (($entries + $source) -join ';'), 'User') }
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Ara-' + $engine.version
    New-Item -Path $key -Force | Out-Null
    $properties = @{ DisplayName=('Ara ' + $engine.version); DisplayVersion=$engine.version; Publisher='Ara'; InstallLocation=$source; DisplayIcon=$engineIcon; UninstallString=('"' + (Join-Path $source 'uninstall.cmd') + '"'); NoModify=1; NoRepair=1 }
    foreach ($entry in $properties.GetEnumerator()) { Set-ItemProperty -LiteralPath $key -Name $entry.Key -Value $entry.Value }
}
$uninstall = '@echo off' + "`r`n" + '(goto) 2>nul & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1" %*' + "`r`n"
[IO.File]::WriteAllText((Join-Path $source 'uninstall.cmd'), $uninstall, [Text.Encoding]::ASCII)
Write-Host "Ara $($engine.version) installed at $source"
Write-Host 'Connect agents once: ara hub install-mcp --client claude|codex'
if (-not $Silent) { Write-Host 'Restart your terminal for PATH changes. Start Ara Hub from the Start menu.' }

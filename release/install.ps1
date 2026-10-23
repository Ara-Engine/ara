[CmdletBinding()]
param([string]$Destination, [switch]$Silent, [switch]$RegisterOnly, [switch]$NoSystemIntegration)
$ErrorActionPreference = 'Stop'
$source = [IO.Path]::GetFullPath($PSScriptRoot)
$engine = Get-Content -LiteralPath (Join-Path $source 'engine.json') -Raw | ConvertFrom-Json
if ($engine.format -ne 'ara.engine/1' -or $engine.version -notmatch '^[0-9A-Za-z.-]+$') { throw 'Invalid Ara engine manifest' }
if (-not $RegisterOnly) {
    if (-not $Destination) { $Destination = Join-Path $env:LOCALAPPDATA ('Programs\Ara\' + $engine.version) }
    $Destination = [IO.Path]::GetFullPath($Destination)
    if (Test-Path -LiteralPath $Destination) { throw "Destination already exists: $Destination" }
    $parent = Split-Path -Parent $Destination
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $stage = Join-Path $parent ('.ara-install-' + [guid]::NewGuid().ToString('N'))
    try {
        New-Item -ItemType Directory -Path $stage | Out-Null
        Get-ChildItem -LiteralPath $source -Force | Copy-Item -Destination $stage -Recurse
        Move-Item -LiteralPath $stage -Destination $Destination
    } catch {
        if (Test-Path -LiteralPath $stage) {
            $resolved = [IO.Path]::GetFullPath($stage)
            if ((Split-Path -Parent $resolved) -ne $parent -or (Split-Path -Leaf $resolved) -notlike '.ara-install-*') { throw 'Unsafe staging cleanup' }
            Remove-Item -LiteralPath $resolved -Recurse -Force
        }
        throw
    }
    $source = $Destination
}
$ara = Join-Path $source 'ara.exe'
& $ara hub register-engine
if ($LASTEXITCODE -ne 0) { throw 'Hub engine registration failed; installation retained for retry' }
& $ara doctor
if ($LASTEXITCODE -ne 0) { Write-Warning 'Prerequisites missing. Run ara doctor and install the x64 .NET 10 SDK: https://dotnet.microsoft.com/en-us/download/dotnet/10.0' }
$shortcutDirectory = Join-Path ([Environment]::GetFolderPath('Programs')) ('Ara\' + $engine.version)
$receipt = [ordered]@{ format='ara.install/1'; version=$engine.version; root=$source; integration=(-not $NoSystemIntegration); shortcuts=$shortcutDirectory }
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $source 'install-receipt.json') -Encoding UTF8
if (-not $NoSystemIntegration) {
    New-Item -ItemType Directory -Path $shortcutDirectory -Force | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    foreach ($app in @('Hub','Editor')) {
        $link = $shell.CreateShortcut((Join-Path $shortcutDirectory ("Ara $app.lnk")))
        $link.TargetPath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $link.Arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $source ('launch-' + $app.ToLowerInvariant() + '.ps1')) + '"'
        $link.WorkingDirectory = $source
        $link.IconLocation = (Join-Path $source 'assets\brand\ara.ico') + ',0'
        $link.Save()
    }
    $entries = @(([Environment]::GetEnvironmentVariable('Path','User') -split ';') | Where-Object { $_ })
    if ($entries -notcontains $source) { [Environment]::SetEnvironmentVariable('Path', (($entries + $source) -join ';'), 'User') }
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Ara-' + $engine.version
    New-Item -Path $key -Force | Out-Null
    $properties = @{ DisplayName=('Ara ' + $engine.version); DisplayVersion=$engine.version; Publisher='Ara'; InstallLocation=$source; DisplayIcon=(Join-Path $source 'assets\brand\ara.ico'); UninstallString=('"' + (Join-Path $source 'uninstall.cmd') + '"'); NoModify=1; NoRepair=1 }
    foreach ($entry in $properties.GetEnumerator()) { Set-ItemProperty -Path $key -Name $entry.Key -Value $entry.Value }
}
$uninstall = '@echo off' + "`r`n" + 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1" %*' + "`r`n"
[IO.File]::WriteAllText((Join-Path $source 'uninstall.cmd'), $uninstall, [Text.Encoding]::ASCII)
Write-Host "Ara $($engine.version) installed at $source"
Write-Host 'Connect agents once: ara hub install-mcp --client claude|codex'
if (-not $Silent) { Write-Host 'Restart your terminal for PATH changes. Start Ara Editor from the Start menu.' }

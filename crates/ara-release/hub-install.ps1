[CmdletBinding()]
param([string]$Destination, [switch]$RegisterOnly, [switch]$Silent, [switch]$NoSystemIntegration)
$ErrorActionPreference = 'Stop'
$source = [IO.Path]::GetFullPath($PSScriptRoot)
$hub = Get-Content -LiteralPath (Join-Path $source 'hub.json') -Raw | ConvertFrom-Json
if ($hub.format -ne 'ara.hub/1') { throw 'Invalid Ara Hub package' }
if (-not $RegisterOnly) {
    if (-not $Destination) { $Destination = Join-Path $env:LOCALAPPDATA 'Programs\Ara Hub' }
    $Destination = [IO.Path]::GetFullPath($Destination)
    if (Test-Path -LiteralPath $Destination) { throw 'Hub already installed; use Restart to update in Ara Hub' }
    $parent = Split-Path -Parent $Destination
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $stage = Join-Path $parent ('.ara-hub-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    Get-ChildItem -LiteralPath $source -Force | Copy-Item -Destination $stage -Recurse
    Move-Item -LiteralPath $stage -Destination $Destination
    $source = $Destination
}
if (-not $NoSystemIntegration) {
    if ($hub.version -notmatch '^[0-9A-Za-z.-]+$') { throw 'Invalid Ara Hub service version' }
    $serviceBase = Join-Path $env:LOCALAPPDATA 'Ara\Services'
    $build = (Get-FileHash -LiteralPath (Join-Path $source 'bootstrap\ara.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    $service = Join-Path (Join-Path $serviceBase $hub.version) $build
    if (-not (Test-Path -LiteralPath $service)) {
        $serviceParent = Split-Path -Parent $service
        New-Item -ItemType Directory -Path $serviceParent -Force | Out-Null
        $serviceStage = Join-Path $serviceParent ('.service-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $serviceStage | Out-Null
        Get-ChildItem -LiteralPath (Join-Path $source 'bootstrap') -File | Copy-Item -Destination $serviceStage
        if (-not (Test-Path -LiteralPath (Join-Path $serviceStage 'service.json'))) { throw 'Bootstrap service marker is missing' }
        Move-Item -LiteralPath $serviceStage -Destination $service
    }
    if (-not (Test-Path -LiteralPath (Join-Path $service 'service.json'))) { throw 'Existing service directory is unrelated' }
    $entries = @(([Environment]::GetEnvironmentVariable('Path','User') -split ';') | Where-Object {
        $_ -and -not $_.StartsWith($serviceBase + '\', [StringComparison]::OrdinalIgnoreCase)
    })
    if ($entries -notcontains $service) { [Environment]::SetEnvironmentVariable('Path', (($entries + $service) -join ';'), 'User') }
    $shortcuts = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ara'
    New-Item -ItemType Directory -Path $shortcuts -Force | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut((Join-Path $shortcuts 'Ara Hub.lnk'))
    $link.TargetPath = Join-Path $source 'ara-hub.exe'
    $link.WorkingDirectory = $source
    $link.IconLocation = (Join-Path $source 'assets\brand\ara.ico') + ',0'
    $link.Save()
}
Write-Host "Ara Hub installed at $source"
if (-not $Silent) { Write-Host 'Open Ara Hub from the Start menu to install an engine version.' }
if (-not $NoSystemIntegration) { Write-Host 'Restart your terminal to use ara hub and install the global MCP.' }

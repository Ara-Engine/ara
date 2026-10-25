
# Keep the launcher outside this versioned engine: its updater must not swap a running service.
if (-not $NoSystemIntegration -and (Test-Path -LiteralPath (Join-Path $source 'ara-hub.exe'))) {
    $hubRoot = Join-Path (ConvertTo-AraPath $env:LOCALAPPDATA) 'Programs\Ara Hub'
    if (-not (Test-Path -LiteralPath $hubRoot)) {
        $hubParent = [IO.Path]::GetDirectoryName($hubRoot)
        [IO.Directory]::CreateDirectory($hubParent) | Out-Null
        $hubStage = Join-Path $hubParent ('.ara-hub-install-' + [guid]::NewGuid().ToString('N'))
        [IO.Directory]::CreateDirectory((Join-Path $hubStage 'bootstrap')) | Out-Null
        foreach ($file in @('ara-hub.exe','ara-hub-updater.exe')) {
            Copy-Item -LiteralPath (Join-Path $source $file) -Destination (Join-Path $hubStage $file)
        }
        Copy-Item -LiteralPath (Join-Path $source 'ara.exe') -Destination (Join-Path $hubStage 'bootstrap\ara.exe')
        Copy-Item -LiteralPath (Join-Path $source 'hub-service.json') -Destination (Join-Path $hubStage 'bootstrap\service.json')
        Copy-Item -LiteralPath (Join-Path $source 'standalone-hub-install.ps1') -Destination (Join-Path $hubStage 'install.ps1')
        Copy-Item -LiteralPath (Join-Path $source 'paths.ps1') -Destination (Join-Path $hubStage 'paths.ps1')
        Get-ChildItem -LiteralPath $source -Filter '*.dll' | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $hubStage
            Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $hubStage 'bootstrap')
        }
        [IO.Directory]::CreateDirectory((Join-Path $hubStage 'assets')) | Out-Null
        Copy-Item -LiteralPath (Join-Path $source 'assets\brand') -Destination (Join-Path $hubStage 'assets') -Recurse
        Copy-Item -LiteralPath (Join-Path $source 'hub-launcher.json') -Destination (Join-Path $hubStage 'hub.json')
        Copy-Item -LiteralPath (Join-Path $source 'THIRD_PARTY_NOTICES.txt') -Destination $hubStage
        Move-Item -LiteralPath $hubStage -Destination $hubRoot
    }
    $installedHub = Get-Content -LiteralPath (Join-Path $hubRoot 'hub.json') -Raw | ConvertFrom-Json
    if ($installedHub.format -ne 'ara.hub/1') { throw 'Existing Ara Hub directory is unrelated; inspect it before installing the launcher' }
    if (Test-Path -LiteralPath (Join-Path $hubRoot 'install.ps1')) { & (Join-Path $hubRoot 'install.ps1') -RegisterOnly -Silent }
    if (-not $NoSystemIntegration) {
        $hubLink = $shell.CreateShortcut((Join-Path $shortcutDirectory 'Ara Hub.lnk'))
        $hubLink.TargetPath = Join-Path $hubRoot 'ara-hub.exe'
        $hubLink.WorkingDirectory = $hubRoot
        $hubLink.IconLocation = (Join-Path $hubRoot 'assets\brand\ara.ico') + ',0'
        $hubLink.Save()
    }
}

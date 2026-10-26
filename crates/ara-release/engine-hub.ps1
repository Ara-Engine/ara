# The shared launcher is updated independently of the versioned engine/service.
if (Test-Path -LiteralPath (Join-Path $source 'ara-hub.exe')) {
    . (Join-Path $source 'hub-installation.ps1')
    $hubRoot = Join-Path (ConvertTo-AraPath $env:LOCALAPPDATA) 'Programs\Ara Hub'
    Install-AraEngineHub $source $hubRoot
    & (Join-Path $hubRoot 'install.ps1') -RegisterOnly -Silent -NoSystemIntegration:$NoSystemIntegration
    if (-not $NoSystemIntegration) {
        New-AraHubShortcut (Join-Path $shortcutDirectory 'Ara Hub.lnk') $hubRoot
    }
}

$ErrorActionPreference = 'Stop'
. ([IO.Path]::Combine($PSScriptRoot, 'paths.ps1'))
$source = ConvertTo-AraPath $PSScriptRoot
$hub = Join-Path $source 'ara-hub.exe'
if (Test-Path -LiteralPath $hub) { & $hub; exit $LASTEXITCODE }
$engine = Get-Content -LiteralPath (Join-Path $source 'engine.json') -Encoding UTF8 -Raw | ConvertFrom-Json
Write-Host "Ara $($engine.version): the native Hub app is not included in this build yet."
& (Join-Path $source 'ara.exe') hub projects
Write-Host 'Use ara hub add/open/create or the Ara Editor shortcut. Global MCP is available through ara mcp.'
Read-Host 'Press Enter to close'

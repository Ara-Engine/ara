$ErrorActionPreference = 'Stop'
$hub = Join-Path $PSScriptRoot 'ara-hub.exe'
if (Test-Path -LiteralPath $hub) { & $hub; exit $LASTEXITCODE }
$engine = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'engine.json') -Encoding UTF8 -Raw | ConvertFrom-Json
Write-Host "Ara $($engine.version): the native Hub app is not included in this build yet."
& (Join-Path $PSScriptRoot 'ara.exe') hub projects
Write-Host 'Use ara hub add/open/create or the Ara Editor shortcut. Global MCP is available through ara mcp.'
Read-Host 'Press Enter to close'

$ErrorActionPreference='Stop'
# Benchmark input stays in the repository's existing gitignored .sn directory.
$assetDirectory=Join-Path $PSScriptRoot '.sn/assets'
$archive=Join-Path $assetDirectory 'sponza.zip'
$destination=Join-Path $assetDirectory 'sponza'
New-Item -ItemType Directory -Force $assetDirectory | Out-Null
if (!(Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest -Uri 'https://casual-effects.com/g3d/data10/common/model/crytek_sponza/sponza.zip' -OutFile $archive
}
$expected='DA005CBEE0BE2DF2ABC8513F3CEB61BCB6F69AAC112BABCD9C00169A27C2770C'
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) {throw 'Sponza archive differs from the measured input'}
Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
Get-Content -LiteralPath (Join-Path $destination 'copyright.txt')
Write-Output 'Local benchmark only. Source and attribution: Frank Meinl / Crytek; Morgan McGuire Computer Graphics Archive.'

param([string]$Destination = "$PSScriptRoot/.sn/assets/beauty")
$ErrorActionPreference = 'Stop'
# Pinned CC0 input bytes. Blender/Python are not required to fetch or render.
$manifest = Get-Content "$PSScriptRoot/assets/beauty-manifest.json" -Raw | ConvertFrom-Json
foreach ($asset in $manifest.assets) {
    foreach ($file in $asset.files) {
        $target = Join-Path $Destination $file.path
        if ((Test-Path -LiteralPath $target) -and ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -eq $file.sha256)) { continue }
        New-Item -ItemType Directory -Force -Path (Split-Path $target) | Out-Null
        Invoke-WebRequest -Uri $file.url -OutFile $target
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $file.sha256) {
            throw "Beauty asset hash mismatch: $($file.path)"
        }
    }
}
Write-Output 'Powered by Poly Haven. CC0 licenses and authors: THIRD_PARTY_NOTICES.md.'

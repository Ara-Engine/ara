param([switch]$Window, [int]$Frames = 120, [switch]$NoShadows, [string]$Output = 'RESULT.png', [string]$Evidence = 'evidence/run.json')
$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_JOBS = '4'
$env:PATH = "$PSScriptRoot/tools/SDL3-3.4.18/lib/x64;$env:PATH"
# Keep unrelated injected overlays out of this process; do not alter global settings.
New-Item -ItemType Directory -Force "$PSScriptRoot/tools/implicit-layers" | Out-Null
$env:VK_IMPLICIT_LAYER_PATH = "$PSScriptRoot/tools/implicit-layers"
$env:VK_LAYER_PATH = 'C:/VulkanSDK/1.4.363.0/Bin'
Push-Location $PSScriptRoot
try {
    $options = @('--frames', "$Frames", '--output', $Output, '--evidence', $Evidence)
    if ($Window) { $options += '--window' }
    if ($NoShadows) { $options += '--no-shadows' }
    cargo run -- @options
    if ($LASTEXITCODE -ne 0) { throw 'S1 execution failed' }
} finally { Pop-Location }

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Push-Location $PSScriptRoot
try {
    New-Item -ItemType Directory -Force tools | Out-Null
    if (!(Test-Path tools/SDL3-3.4.18/lib/x64/SDL3.dll)) {
        Invoke-WebRequest -UseBasicParsing 'https://github.com/libsdl-org/SDL/releases/download/release-3.4.18/SDL3-devel-3.4.18-VC.zip' -OutFile tools/SDL3.zip
        Expand-Archive -LiteralPath tools/SDL3.zip -DestinationPath tools -Force
    }
    Write-Output 'SDL 3.4.18 ready. Install Vulkan SDK 1.4.363.0 separately if not present.'
} finally { Pop-Location }

param([string]$Version = '3.4.18')
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$destination = Join-Path $PSScriptRoot 'native'
New-Item -ItemType Directory -Force $destination | Out-Null
$archive = Join-Path $destination 'SDL3.zip'
Invoke-WebRequest -UseBasicParsing "https://github.com/libsdl-org/SDL/releases/download/release-$Version/SDL3-devel-$Version-VC.zip" -OutFile $archive
Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
Copy-Item -LiteralPath (Join-Path $destination "SDL3-$Version/lib/x64/SDL3.lib") -Destination $destination
Copy-Item -LiteralPath (Join-Path $destination "SDL3-$Version/lib/x64/SDL3.dll") -Destination $destination

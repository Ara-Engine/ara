param([switch]$Dev,[string]$ScratchRoot=(Join-Path $env:TEMP ('ara-editor-panels-'+[guid]::NewGuid().ToString('N'))))
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','VULKAN_SDK','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) {
    $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
}
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    if(-not $env:VULKAN_SDK){$env:VULKAN_SDK='C:/VulkanSDK/1.4.363.0'}
    $env:VK_LAYER_PATH=Join-Path $env:VULKAN_SDK 'Bin'
    $env:VK_IMPLICIT_LAYER_PATH=Join-Path $PSScriptRoot 'evidence/empty-vulkan-layers'
    New-Item -ItemType Directory -Force $env:VK_IMPLICIT_LAYER_PATH | Out-Null
    if($Dev){cargo build -p ara-editor}else{cargo build --release -p ara-editor}
    if($LASTEXITCODE -ne 0){throw 'editor build failed'}
    # A separate executable allows checks to link while the desktop route is running.
    New-Item -ItemType Directory -Force $ScratchRoot | Out-Null
    $executable=Join-Path $ScratchRoot 'ara-editor.exe'
    $profile=if($Dev){'debug'}else{'release'}
    Copy-Item -LiteralPath "target/$profile/ara-editor.exe" -Destination $executable
    foreach($view in @(@('1440x900','1','1440'),@('2560x1440','2','2560'))) {
        $project=Join-Path $ScratchRoot $view[2]
        $ErrorActionPreference='Continue'
        & $executable $project --sample --verify-panels --frames 10000 --size $view[0] --dpi $view[1] --output "$PSScriptRoot/evidence/panels/$($view[2])"
        $code=$LASTEXITCODE
        $ErrorActionPreference='Stop'
        if($code -ne 0){throw "Panel verification failed at $($view[0])"}
    }
} finally {
    foreach($key in $previous.Keys){[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')}
    Pop-Location
}

param([switch]$Release,[string]$Output=(Join-Path $PSScriptRoot 'evidence/ux-v2'),[string]$ScratchRoot=(Join-Path $env:TEMP ('ara-editor-ux-'+[guid]::NewGuid().ToString('N'))))
# Editor UX v2 capture route: menus, context menus, palette, Settings, a floating
# Inspector window and every tool tab in both themes, at 1440x900 and 2560x1369
# (1x), then idle and camera timings. Captures and reports go to $Output/<size>.
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
    $ErrorActionPreference='Continue'
    if($Release){cargo build --release -p ara-editor}else{cargo build -p ara-editor}
    $code=$LASTEXITCODE
    $ErrorActionPreference='Stop'
    if($code -ne 0){throw 'editor build failed'}
    New-Item -ItemType Directory -Force $ScratchRoot | Out-Null
    $profile=if($Release){'release'}else{'debug'}
    $executable=Join-Path $ScratchRoot 'ara-editor.exe'
    Copy-Item -LiteralPath "target/$profile/ara-editor.exe" -Destination $executable
    foreach($view in @(@('1440x900','1440'),@('2560x1369','2560'))) {
        $project=Join-Path $ScratchRoot $view[1]
        $out=Join-Path $Output $view[1]
        New-Item -ItemType Directory -Force $out | Out-Null
        $ErrorActionPreference='Continue'
        & $executable $project --sample --verify-ux --frames 60000 --size $view[0] --dpi 1 --output $out
        $code=$LASTEXITCODE
        $ErrorActionPreference='Stop'
        if($code -ne 0){throw "UX verification failed at $($view[0])"}
    }
} finally {
    foreach($key in $previous.Keys){[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')}
    Pop-Location
}

param([ValidateSet('Check','Cook','Native','Upscaled')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$saved=@{}
foreach($name in @('CARGO_BUILD_JOBS','VULKAN_SDK','VK_LAYER_PATH','VK_IMPLICIT_LAYER_PATH')) {
    $saved[$name]=[Environment]::GetEnvironmentVariable($name,'Process')
}
Push-Location (Join-Path $PSScriptRoot '../..')
try {
    $env:CARGO_BUILD_JOBS='2'
    if(-not $env:VULKAN_SDK){$env:VULKAN_SDK='C:\VulkanSDK\1.4.363.0'}
    $env:VK_LAYER_PATH=Join-Path $env:VULKAN_SDK 'Bin'
    $empty=Join-Path $env:TEMP 'ara-geo-empty-layers'
    New-Item -ItemType Directory -Force $empty | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH=$empty
    if($Mode -eq 'Cook') {
        cargo run -p ara-render-geo --example prepare_cooks
        if($LASTEXITCODE -ne 0){throw 'Shader preparation failed'}
        cargo run -p ara-shader-cook -- crates/ara-texture-stream/shaders/pipelines.json crates/ara-texture-stream/cooked
        if($LASTEXITCODE -ne 0){throw 'Texture shader cook failed'}
        cargo run -p ara-shader-cook -- crates/ara-render-geo/shaders/pipelines.json crates/ara-render-geo/cooked
        if($LASTEXITCODE -ne 0){throw 'Shader cook failed'}
    } elseif($Mode -eq 'Native' -or $Mode -eq 'Upscaled') {
        $flags=@();if($Mode -eq 'Upscaled'){$flags=@('--upscaled')}
        cargo run -p ara-render-geo --example photogrammetry -- @flags
        if($LASTEXITCODE -ne 0){throw 'Benchmark failed'}
    } else {
        cargo test -p ara-geo -- --include-ignored --test-threads=1
        if($LASTEXITCODE -ne 0){throw 'Geometry cook tests failed'}
        cargo test -p ara-render-geo -p ara-vsm -p ara-texture-stream -p ara-color -p ara-gpu -p ara-platform -p ara-importers -p ara-landscape -- --test-threads=1
        if($LASTEXITCODE -ne 0){throw 'Tests failed'}
        cargo clippy -p ara-geo -p ara-render-geo -p ara-vsm -p ara-texture-stream -p ara-color -p ara-gpu -p ara-platform -p ara-importers -p ara-landscape --all-targets -- -D warnings
        if($LASTEXITCODE -ne 0){throw 'Clippy failed'}
        cargo fmt -p ara-geo -p ara-render-geo -p ara-vsm -p ara-texture-stream -p ara-color -p ara-gpu -p ara-platform -p ara-importers -p ara-landscape -- --check
        if($LASTEXITCODE -ne 0){throw 'Formatting failed'}
    }
} finally {
    Pop-Location
    foreach($name in $saved.Keys){[Environment]::SetEnvironmentVariable($name,$saved[$name],'Process')}
}

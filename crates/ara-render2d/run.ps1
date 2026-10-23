param([ValidateSet('Check','Cook','Capture','Benchmark')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$previous=@{}
foreach ($key in @('CARGO_BUILD_JOBS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:PATH="$env:SDL3_LIB_DIR;$repo/crates/ara-platform/native;C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    New-Item -ItemType Directory -Force "$repo/.sn/empty-vulkan-layers" | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH="$repo/.sn/empty-vulkan-layers"
    $env:VK_LAYER_PATH='C:/VulkanSDK/1.4.363.0/Bin'
    if ($Mode -eq 'Cook') { cargo run -p ara-shader-cook -- crates/ara-render2d/shaders/pipelines.json crates/ara-render2d/cooked }
    elseif ($Mode -eq 'Capture') { cargo run -p ara-render2d --example platformer -- capture }
    elseif ($Mode -eq 'Benchmark') { cargo run -p ara-render2d --example platformer -- benchmark }
    else {
        $packages=@('-p','ara-2d','-p','ara-import2d','-p','ara-physics2d','-p','ara-render2d','-p','ara-gpu','-p','ara-render')
        cargo check @packages
        if ($LASTEXITCODE -ne 0) { throw 'cargo check failed' }
        cargo test @packages
        if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
        cargo clippy @packages --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
        cargo fmt @packages -- --check
    }
    if ($LASTEXITCODE -ne 0) { throw '2D verification failed' }
} finally {
    Pop-Location
    foreach ($key in $previous.Keys) { [Environment]::SetEnvironmentVariable($key,$previous[$key],'Process') }
}

param([ValidateSet('Capture','Window','Check')][string]$Mode='Check',[int]$Frames=120)
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Push-Location $repo
$previous=@{}
foreach ($key in @('CARGO_BUILD_JOBS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
try {
    $env:CARGO_BUILD_JOBS='4'
    $env:PATH="$repo/crates/ara-platform/native;C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    New-Item -ItemType Directory -Force "$repo/.sn/empty-vulkan-layers" | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH="$repo/.sn/empty-vulkan-layers"
    $env:VK_LAYER_PATH='C:/VulkanSDK/1.4.363.0/Bin'
    if ($Mode -eq 'Capture') { cargo run -p ara-render --example capture }
    elseif ($Mode -eq 'Window') { cargo run -p ara-render --example windowed -- "--frames=$Frames" }
    else {
        cargo check -p ara-gpu -p ara-render -p ara-platform -p ara-shader-cook
        if ($LASTEXITCODE -ne 0) { throw 'cargo check failed' }
        cargo test -p ara-gpu -p ara-render -p ara-platform -p ara-shader-cook
        if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
        cargo clippy -p ara-gpu -p ara-render -p ara-platform -p ara-shader-cook --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
        cargo fmt -p ara-gpu -p ara-render -p ara-platform -p ara-shader-cook -- --check
    }
    if ($LASTEXITCODE -ne 0) { throw 'Renderer command failed' }
} finally {
    foreach($key in $previous.Keys) { [Environment]::SetEnvironmentVariable($key,$previous[$key],'Process') }
    Pop-Location
}

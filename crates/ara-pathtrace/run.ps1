param([ValidateSet('Check','Smoke','Capture','Timings')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Push-Location $repo
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) {$previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process')}
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:PATH="$repo/crates/ara-platform/native;C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    New-Item -ItemType Directory -Force "$PSScriptRoot/.sn/empty-vulkan-layers" | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH="$PSScriptRoot/.sn/empty-vulkan-layers"
    $env:VK_LAYER_PATH='C:/VulkanSDK/1.4.363.0/Bin'
    if($Mode -eq 'Check') {
        cargo test -p ara-render-fidelity -p ara-pathtrace
        if($LASTEXITCODE -ne 0){throw 'Tests failed'}
        cargo clippy -p ara-render-fidelity -p ara-pathtrace --all-targets -- -D warnings
        if($LASTEXITCODE -ne 0){throw 'Clippy failed'}
        cargo fmt -p ara-render-fidelity -p ara-pathtrace -- --check
    } else {cargo run -p ara-pathtrace -- $Mode.ToLowerInvariant()}
    if($LASTEXITCODE -ne 0){throw 'Fidelity verification failed'}
} finally {foreach($key in $previous.Keys){[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')};Pop-Location}

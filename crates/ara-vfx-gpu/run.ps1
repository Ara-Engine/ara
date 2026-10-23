param([ValidateSet('Check','Cook','Capture','Timings')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
Push-Location $repo
$previous=@{}
foreach ($key in @('CARGO_BUILD_JOBS','RUST_TEST_THREADS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH','ARA_VFX_GPU_TESTS')) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:RUST_TEST_THREADS='2'
    $env:PATH="$repo/crates/ara-platform/native;C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    $empty=Join-Path $PSScriptRoot '.cache/empty-layers'
    New-Item -ItemType Directory -Force $empty | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH=$empty
    $env:VK_LAYER_PATH='C:/VulkanSDK/1.4.363.0/Bin'
    if ($Mode -eq 'Cook') {cargo run -p ara-vfx-gpu --example vfx_cook}
    elseif ($Mode -eq 'Capture') {cargo run -p ara-vfx-gpu --example vfx_capture}
    elseif ($Mode -eq 'Timings') {cargo run -p ara-vfx-gpu --example vfx_timings}
    else {
        cargo check -p ara-vfx -p ara-vfx-gpu -p ara-gpu -p ara-render -p ara-json -p ara-render-fidelity --all-targets
        if ($LASTEXITCODE -ne 0) {throw 'check failed'}
        $env:ARA_VFX_GPU_TESTS='1'
        cargo test -p ara-vfx -p ara-vfx-gpu -p ara-gpu -p ara-render -p ara-json -p ara-render-fidelity --no-fail-fast
        if ($LASTEXITCODE -ne 0) {throw 'tests failed'}
        cargo clippy -p ara-vfx -p ara-vfx-gpu -p ara-gpu -p ara-render -p ara-json -p ara-render-fidelity --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) {throw 'clippy failed'}
        cargo fmt -p ara-vfx -p ara-vfx-gpu -p ara-gpu -p ara-render -p ara-json -p ara-render-fidelity -- --check
    }
    if ($LASTEXITCODE -ne 0) {throw 'VFX command failed'}
} finally {
    foreach($key in $previous.Keys) {[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')}
    Pop-Location
}

param(
    [ValidateSet('Gate', 'Overhead')][string]$Mode = 'Gate',
    [string]$TargetDir = ''
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $TargetDir) { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' } }
$keys = @('CARGO_BUILD_JOBS', 'CARGO_TARGET_DIR', 'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG', 'CARGO_INCREMENTAL', 'VULKAN_SDK', 'VK_IMPLICIT_LAYER_PATH', 'SDL3_LIB_DIR', 'PATH', 'TEMP', 'TMP')
$previous = @{}
foreach ($key in $keys) { $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS = '2'
    $env:CARGO_TARGET_DIR = $TargetDir
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
    $env:CARGO_PROFILE_TEST_DEBUG = '0'
    $env:CARGO_INCREMENTAL = '0'
    $temporary = Join-Path $TargetDir 'temp'
    New-Item -ItemType Directory -Force -Path $temporary | Out-Null
    $env:TEMP = $temporary
    $env:TMP = $temporary
    if (-not $env:VULKAN_SDK) { $env:VULKAN_SDK = 'C:/VulkanSDK/1.4.363.0' }
    if (-not $env:SDL3_LIB_DIR) { $env:SDL3_LIB_DIR = "$repo/crates/ara-platform/native" }
    $env:PATH = "$env:SDL3_LIB_DIR;$env:VULKAN_SDK/Bin;$env:PATH"
    $layers = Join-Path $TargetDir 'empty-vulkan-layers'
    New-Item -ItemType Directory -Force -Path $layers | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH = $layers
    # Windows PowerShell 5 turns redirected native stderr into ErrorRecords.
    # Cargo reports progress there even on success; check its exit code explicitly.
    $ErrorActionPreference = 'Continue'
    if ($Mode -eq 'Overhead') {
        foreach ($features in @('shipping', '', 'allocation', 'tracy')) {
            $arguments = @('run', '-p', 'ara-bench', '--release', '--example', 'overhead')
            if ($features -eq 'shipping') { $arguments += '--no-default-features' }
            elseif ($features) { $arguments += @('--features', $features) }
            & cargo @arguments
            if ($LASTEXITCODE -ne 0) { throw "overhead measurement failed: $features" }
        }
    } else {
        cargo test -p ara-profile --features allocation,gpu
        if ($LASTEXITCODE -ne 0) { throw 'native profiler tests failed' }
        cargo test -p ara-profile --features instrumentation,allocation,gpu
        if ($LASTEXITCODE -ne 0) { throw 'static scope tests failed' }
        cargo test -p ara-profile --all-features
        if ($LASTEXITCODE -ne 0) { throw 'Tracy profiler tests failed' }
        cargo test -p ara-profile --features gpu --test gpu -- --ignored
        if ($LASTEXITCODE -ne 0) { throw 'Vulkan correlation test failed' }
        $packages = @('ara-profile','ara-bench','ara-host','ara','ara-sim','ara-anim','ara-physics','ara-nav','ara-audio','ara-asset','ara-render','ara-script')
        $packageArgs = @()
        foreach ($package in $packages) { $packageArgs += @('-p', $package) }
        $features = 'ara-profile/allocation,ara-profile/gpu,ara-bench/scenario,ara-sim/profiling,ara-anim/profiling,ara-physics/profiling,ara-nav/profiling,ara-audio/profiling,ara-asset/profiling,ara-render/profiling,ara-script/profiling'
        & cargo test @packageArgs --features $features
        if ($LASTEXITCODE -ne 0) { throw 'subsystem tests failed' }
        & cargo clippy @packageArgs --features "$features,ara-profile/tracy,ara-bench/tracy" --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'clippy failed' }
        & cargo fmt @packageArgs -- --check
        if ($LASTEXITCODE -ne 0) { throw 'format check failed' }
    }
} finally {
    Pop-Location
    foreach ($key in $keys) { [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process') }
}

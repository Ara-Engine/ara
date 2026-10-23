param([ValidateSet('Check','Cook','Prepare','Capture','Benchmark')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$previous=@{}
foreach ($key in @('CARGO_BUILD_JOBS','PATH')) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:PATH="C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    if ($Mode -eq 'Cook') { cargo run -p ara-shader-cook -- crates/ara-render2d/shaders/pipelines.json crates/ara-render2d/cooked }
    elseif ($Mode -eq 'Prepare') { cargo run -p ara-render2d --example platformer -- prepare }
    elseif ($Mode -eq 'Capture') { cargo run -p ara-render2d --example platformer -- capture }
    elseif ($Mode -eq 'Benchmark') { cargo run -p ara-render2d --example platformer -- benchmark }
    else {
        $packages=@('-p','ara-2d','-p','ara-import2d','-p','ara-physics2d','-p','ara-render2d')
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

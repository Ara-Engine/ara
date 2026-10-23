param([ValidateSet('Check','Cook','Capture')][string]$Mode='Check')
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')){$previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process')}
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:PATH="C:/VulkanSDK/1.4.363.0/Bin;$env:PATH"
    New-Item -ItemType Directory -Force "$PSScriptRoot/cache/empty-layers" | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH="$PSScriptRoot/cache/empty-layers"
    $env:VK_LAYER_PATH='C:/VulkanSDK/1.4.363.0/Bin'
    if($Mode -eq 'Cook'){cargo run -p ara-landscape --example cook}
    elseif($Mode -eq 'Capture'){cargo run -p ara-landscape --example landscape}
    else {
        cargo check -p ara-terrain -p ara-foliage -p ara-landscape -p ara-gpu
        if($LASTEXITCODE -ne 0){throw 'cargo check failed'}
        cargo test -p ara-terrain -p ara-foliage -p ara-landscape -p ara-gpu
        if($LASTEXITCODE -ne 0){throw 'cargo test failed'}
        cargo clippy -p ara-terrain -p ara-foliage -p ara-landscape -p ara-gpu --all-targets -- -D warnings
        if($LASTEXITCODE -ne 0){throw 'cargo clippy failed'}
        cargo fmt -p ara-terrain -p ara-foliage -p ara-landscape -p ara-gpu -- --check
    }
    if($LASTEXITCODE -ne 0){throw 'landscape command failed'}
} finally {
    foreach($key in $previous.Keys){[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')}
    Pop-Location
}

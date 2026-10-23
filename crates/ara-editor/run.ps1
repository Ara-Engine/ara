param(
    [ValidateSet('Check','Verify','Window')][string]$Mode='Check',
    [string]$Project,
    [int]$Frames=5300
)
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','VULKAN_SDK','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    if (-not $env:VULKAN_SDK) { $env:VULKAN_SDK='C:/VulkanSDK/1.4.363.0' }
    $empty=Join-Path $PSScriptRoot 'evidence/empty-vulkan-layers'
    New-Item -ItemType Directory -Force $empty | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH=$empty
    $env:VK_LAYER_PATH=Join-Path $env:VULKAN_SDK 'Bin'
    if ($Mode -eq 'Check') {
        cargo check -p ara-editor -p ara-editor-gpu
        if ($LASTEXITCODE -ne 0) { throw 'check failed' }
        cargo test -p ara-editor -p ara-editor-gpu -p ara-render -p ara-ui -p ara-ui-document -p ara-host
        if ($LASTEXITCODE -ne 0) { throw 'tests failed' }
        cargo test -p ara-editor --test editor hierarchy_100k -- --ignored --nocapture
        if ($LASTEXITCODE -ne 0) { throw 'hierarchy scale gate failed' }
        cargo test -p ara-editor --test native -- --ignored --nocapture
        if ($LASTEXITCODE -ne 0) { throw 'native golden gate failed' }
        cargo clippy -p ara-editor -p ara-editor-gpu -p ara-render -p ara-ui -p ara-ui-document -p ara-host --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'clippy failed' }
        cargo fmt -p ara-editor -p ara-editor-gpu -p ara-render -p ara-ui -p ara-ui-document -p ara-host -- --check
    } else {
        if (-not $Project) { throw '-Project must name an initialized project' }
        if ($Mode -eq 'Verify') {
            cargo run --release -p ara-editor -- $Project --sample --verify --frames $Frames --output "$PSScriptRoot/evidence/final"
        } else {
            cargo run -p ara-editor -- $Project --sample
        }
    }
    if ($LASTEXITCODE -ne 0) { throw 'editor command failed' }
} finally {
    foreach($key in $previous.Keys) { [Environment]::SetEnvironmentVariable($key,$previous[$key],'Process') }
    Pop-Location
}

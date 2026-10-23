param([Parameter(Mandatory=$true)][string]$Project, [int]$Frames=5500)
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$projectRoot=(Resolve-Path -LiteralPath $Project).Path
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','CARGO_INCREMENTAL','VULKAN_SDK','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH')) {
    $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
}
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    $env:CARGO_INCREMENTAL='0'
    if (-not $env:VULKAN_SDK) { $env:VULKAN_SDK='C:/VulkanSDK/1.4.363.0' }
    $empty=Join-Path $PSScriptRoot 'evidence/empty-vulkan-layers'
    New-Item -ItemType Directory -Force $empty | Out-Null
    $env:VK_IMPLICIT_LAYER_PATH=$empty
    $env:VK_LAYER_PATH=Join-Path $env:VULKAN_SDK 'Bin'
    foreach($view in @(@('1440x900','1','1440'),@('2560x1440','2','2560'))) {
        # PowerShell 5 maps native progress on stderr to error records when logged.
        $ErrorActionPreference='Continue'
        cargo run --release -p ara-editor -- $projectRoot --sample --verify --frames $Frames --size $view[0] --dpi $view[1] --output "$PSScriptRoot/evidence/polish/$($view[2])"
        $code=$LASTEXITCODE
        $ErrorActionPreference='Stop'
        if($code -ne 0){throw "Polish verification failed at $($view[0])"}
    }
} finally {
    foreach($key in $previous.Keys) { [Environment]::SetEnvironmentVariable($key,$previous[$key],'Process') }
    Pop-Location
}

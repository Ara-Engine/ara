param([string]$Sdk = 'C:/VulkanSDK/1.4.363.0')
$ErrorActionPreference = 'Stop'
$shader = Join-Path $PSScriptRoot 'ui.slang'
$output = Join-Path $PSScriptRoot '../evidence'
New-Item -ItemType Directory -Force $output | Out-Null
foreach ($entry in @(@('vertexMain','vertex'), @('fragmentMain','fragment'))) {
    $name = $entry[0]
    & "$Sdk/Bin/slangc.exe" $shader -entry $name -stage $entry[1] -target spirv -profile sm_6_6+spirv_1_6 -O2 -o "$output/$name.spv"
    if ($LASTEXITCODE -ne 0) { throw "SPIR-V compilation failed: $name" }
    & "$Sdk/Bin/spirv-val.exe" --target-env vulkan1.3 "$output/$name.spv"
    if ($LASTEXITCODE -ne 0) { throw "SPIR-V validation failed: $name" }
    & "$Sdk/Bin/slangc.exe" $shader -entry $name -stage $entry[1] -target dxil -profile sm_6_6 -O2 -o "$output/$name.dxil"
    if ($LASTEXITCODE -ne 0) { throw "DXIL compilation failed: $name" }
}

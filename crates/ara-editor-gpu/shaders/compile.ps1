param([string]$Sdk='C:/VulkanSDK/1.4.363.0')
$ErrorActionPreference='Stop'
foreach($entry in @(@('vertexMain','vertex'),@('fragmentMain','fragment'))) {
    & "$Sdk/Bin/slangc.exe" "$PSScriptRoot/ui.slang" -entry $entry[0] -stage $entry[1] -target spirv -profile sm_6_6+spirv_1_6 -O2 -o "$PSScriptRoot/$($entry[0]).spv"
    if($LASTEXITCODE -ne 0){throw 'UI shader cook failed'}
    & "$Sdk/Bin/spirv-val.exe" --target-env vulkan1.3 "$PSScriptRoot/$($entry[0]).spv"
    if($LASTEXITCODE -ne 0){throw 'UI SPIR-V validation failed'}
}

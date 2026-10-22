param([string]$Sdk = 'C:/VulkanSDK/1.4.363.0')
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    New-Item -ItemType Directory -Force cooked | Out-Null
    foreach ($shader in @(@('vertexMain', 'vertex'), @('fragmentMain', 'fragment'), @('computeMain', 'compute'))) {
        $entry = $shader[0]; $stage = $shader[1]
        $extra = @()
        if ($stage -eq 'vertex') { $extra = @('-fvk-invert-y') }
        & "$Sdk/Bin/slangc.exe" shaders/scene.slang -D VULKAN=1 -entry $entry -stage $stage -target spirv -profile sm_6_6+spirv_1_6 -capability spvRayQueryKHR -O2 @extra -reflection-json "cooked/$entry.spirv.json" -o "cooked/$entry.spv"
        if ($LASTEXITCODE -ne 0) { throw "SPIR-V compilation failed: $entry" }
        & "$Sdk/Bin/spirv-val.exe" --target-env vulkan1.3 "cooked/$entry.spv"
        if ($LASTEXITCODE -ne 0) { throw "SPIR-V validation failed: $entry" }
        & "$Sdk/Bin/slangc.exe" shaders/scene.slang -entry $entry -stage $stage -target dxil -profile sm_6_6 -O2 -reflection-json "cooked/$entry.dxil.json" -o "cooked/$entry.dxil"
        if ($LASTEXITCODE -ne 0) { throw "DXIL compilation failed: $entry" }
    }
    & "$Sdk/Bin/slangc.exe" -version
    Get-FileHash shaders/scene.slang,cooked/*.spv,cooked/*.dxil | Format-Table -AutoSize
} finally { Pop-Location }

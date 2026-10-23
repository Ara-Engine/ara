param([switch]$UpdateGoldens, [switch]$UpdateMeasurements, [switch]$SharedGpu, [switch]$TimingsOnly)
# Reproduce round-3 receipts. SharedGpu records budget failures without hiding them.
$ErrorActionPreference='Stop'
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$scratch=Join-Path $PSScriptRoot '.sn'
$previous=@{}
foreach($key in @('CARGO_BUILD_JOBS','PATH','VK_IMPLICIT_LAYER_PATH','VK_LAYER_PATH','ARA_UPDATE_FIDELITY_GOLDENS','ARA_UPDATE_MEASUREMENTS','CARGO_TARGET_TMPDIR')) {
    $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
}
$sampler=$null
Push-Location $repo
try {
    $env:CARGO_BUILD_JOBS='2'
    if(-not $env:CARGO_TARGET_TMPDIR){$env:CARGO_TARGET_TMPDIR=Join-Path $repo 'target'}
    if($UpdateMeasurements){$env:ARA_UPDATE_MEASUREMENTS='1'}else{Remove-Item Env:ARA_UPDATE_MEASUREMENTS -ErrorAction SilentlyContinue}
    $sdk=$env:VULKAN_SDK
    if(-not $sdk){$sdk='C:/VulkanSDK/1.4.363.0'}
    $env:PATH="$repo/crates/ara-platform/native;$sdk/Bin;$env:PATH"
    $env:VK_IMPLICIT_LAYER_PATH=Join-Path $scratch 'empty-vulkan-layers'
    $env:VK_LAYER_PATH=Join-Path $sdk 'Bin'
    New-Item -ItemType Directory -Force $env:VK_IMPLICIT_LAYER_PATH | Out-Null
    if($UpdateGoldens){$env:ARA_UPDATE_FIDELITY_GOLDENS='1'}else{Remove-Item Env:ARA_UPDATE_FIDELITY_GOLDENS -ErrorAction SilentlyContinue}
    if(-not $TimingsOnly) {
        cargo run -p ara-shader-cook -- crates/ara-render-fidelity/shaders/pipelines.json crates/ara-render-fidelity/cooked
        if($LASTEXITCODE -ne 0){throw 'Shader cook failed'}
    }
    cargo build -p ara-pathtrace
    if($LASTEXITCODE -ne 0){throw 'Build failed'}
    $nvidia=(Get-Command nvidia-smi.exe -ErrorAction Stop).Source
    $sampler=Start-Process -FilePath $nvidia -WindowStyle Hidden -ArgumentList '--query-gpu=timestamp,utilization.gpu,utilization.memory,memory.used,power.draw,clocks.sm,clocks.mem','--format=csv','--loop-ms=2000' -RedirectStandardOutput "$scratch/round3-shared-gpu-load.csv" -RedirectStandardError "$scratch/round3-shared-gpu-load.err" -PassThru
    $stages=if($TimingsOnly){@('content','robust-timings','many-timings','beauty-timings')}else{@('robust-quality','night-noise','sponza-reference','sponza-quality','lighting-sequence','many-lights','many-noise','beauty','quality','capture','content','robust-timings','many-timings','beauty-timings')}
    foreach($stage in $stages) {
        Write-Output "START STAGE $stage $(Get-Date -Format o)"
        & "$repo/target/debug/ara-pathtrace.exe" $stage
        $code=$LASTEXITCODE
        $code | Set-Content "$scratch/round3-resolution-final-$stage.exit"
        Write-Output "END STAGE $stage exit=$code $(Get-Date -Format o)"
        if($code -ne 0) {
            if($SharedGpu -and $stage -in @('content','robust-timings')){Write-Warning "${stage}: shared-GPU timing gate failed; retained in the report"}
            else{throw "Verification stage failed: $stage"}
        }
    }
    if(-not $sampler.HasExited){$sampler.Kill()}
    $sampler.WaitForExit();$sampler=$null
    if(-not $TimingsOnly) {
        Remove-Item Env:ARA_UPDATE_FIDELITY_GOLDENS,Env:ARA_UPDATE_MEASUREMENTS -ErrorAction SilentlyContinue
        cargo test -p ara-render-fidelity -p ara-pathtrace --all-targets -- --test-threads=1
        $LASTEXITCODE | Set-Content "$scratch/round3-resolution-final-tests.exit"
        if($LASTEXITCODE -ne 0){throw 'Tests failed'}
        cargo clippy -p ara-render-fidelity -p ara-pathtrace --all-targets -- -D warnings
        $LASTEXITCODE | Set-Content "$scratch/round3-resolution-final-clippy.exit"
        if($LASTEXITCODE -ne 0){throw 'Clippy failed'}
        cargo fmt -p ara-render-fidelity -p ara-pathtrace -- --check
        $LASTEXITCODE | Set-Content "$scratch/round3-resolution-final-fmt.exit"
        if($LASTEXITCODE -ne 0){throw 'Format failed'}
        $key=(Get-Content crates/ara-render-fidelity/cooked/reservoir_init.json -Raw | ConvertFrom-Json).key
        $modules=@(Get-ChildItem -LiteralPath "$repo/crates/ara-render-fidelity/cooked/$key" -Filter '*.spv')
        if($modules.Count -ne 48){throw 'Shader count differs'}
        foreach($module in $modules){& spirv-val --target-env vulkan1.3 $module.FullName;if($LASTEXITCODE -ne 0){throw "Invalid SPIR-V: $($module.Name)"}}
        if($UpdateMeasurements) {
            # Tests always write scratch output. Promotion is a separate explicit action.
            foreach($name in @('quality-targets.json','denoiser-resolution.json')) {
                Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_TMPDIR "ara-pathtrace/measurements/$name") -Destination (Join-Path $PSScriptRoot "measurements/$name")
            }
            $env:ARA_UPDATE_MEASUREMENTS='1'
        }
        cargo run -p ara-pathtrace --example record_round3
        if($LASTEXITCODE -ne 0){throw 'Measurement aggregation failed'}
    }
} finally {
    if($sampler -and -not $sampler.HasExited){$sampler.Kill();$sampler.WaitForExit()}
    foreach($key in $previous.Keys){[Environment]::SetEnvironmentVariable($key,$previous[$key],'Process')}
    Pop-Location
}

$ErrorActionPreference='Stop'
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
$source='D:/AraBuild/ux-editor-source'
$target='D:/AraBuild/ux-editor-target'
if(git -C $source status --porcelain){throw 'Acceptance checkout is dirty'}
git -C $source checkout --detach 1e1db65a
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='3'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:DOTNET_CLI_HOME="$target/dotnet-home"
$env:TEMP="$target/tmp/cook-echo"
$env:TMP=$env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP|Out-Null
Set-Location $source
$ErrorActionPreference='Continue'
& cargo build -p ara-shader-cook --locked 2>&1 | Tee-Object -FilePath "$target/orchestration/build-cooker-1e1db65a.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$jobs=@(
 @('shaders/pipelines.json','crates/ara-render/cooked'),
 @('crates/ara-runtime-render/shaders/pipelines.json','crates/ara-runtime-render/cooked')
)
foreach($manifest in Get-ChildItem -LiteralPath crates/ara-vfx-gpu/generated -Directory){$jobs+=,@("crates/ara-vfx-gpu/generated/$($manifest.Name)/pipelines.json","crates/ara-vfx-gpu/cooked/$($manifest.Name)")}
foreach($job in $jobs){
 $label=($job[1] -replace '[^a-zA-Z0-9-]','-')
 & "$target/debug/ara-shader-cook.exe" $job[0] $job[1] 'C:/VulkanSDK/1.4.363.0' 2>&1 | Tee-Object -FilePath "$target/orchestration/recook-1e1db65a-$label.log"
 if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
}
git status --short
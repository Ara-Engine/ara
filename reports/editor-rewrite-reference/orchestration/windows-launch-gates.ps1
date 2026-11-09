$ErrorActionPreference='Continue'
$target='D:/AraBuild/ux-editor-target'
$source='D:/AraBuild/ux-editor-source'
$profile=Join-Path $target ('launch-gates-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path "$profile/config","$profile/crashes","$profile/temp" | Out-Null
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='3'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:ARA_CONFIG_DIR="$profile/config"
$env:ARA_CRASH_DIR="$profile/crashes"
$env:TEMP="$profile/temp"
$env:TMP="$profile/temp"
$env:DOTNET_CLI_HOME="$target/dotnet-home"
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $source
& cargo clippy -p ara-shader-cook -p ara-export --all-targets --locked -- -D warnings 2>&1 | Tee-Object -FilePath "$profile/clippy.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
& cargo test -p ara-shader-cook -p ara-export --locked 2>&1 | Tee-Object -FilePath "$profile/tests.log"
exit $LASTEXITCODE
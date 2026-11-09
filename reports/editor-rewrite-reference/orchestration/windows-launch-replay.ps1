$ErrorActionPreference='Continue'
$target='D:/AraBuild/ux-editor-target'
$source='D:/AraBuild/ux-editor-source'
$profile=Join-Path $target ('launch-replay-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path "$profile/config","$profile/crashes","$profile/temp","$profile/dotnet","$target/nuget" | Out-Null
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
$env:DOTNET_CLI_HOME="$profile/dotnet"
$env:NUGET_PACKAGES="$target/nuget"
$env:ARA_DOTNET_ARTIFACTS="$target/sdk-tools/bin"
$env:ARA_DOTNET_SDK="$source/dotnet"
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $source
& dotnet build dotnet/src/Ara.CompileServer/Ara.CompileServer.csproj -c Release --artifacts-path "$target/sdk-tools" --disable-build-servers -m:3 -nr:false -p:UseSharedCompilation=false 2>&1 | Tee-Object -FilePath "$profile/sdk-build.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
& dotnet build dotnet/src/Ara.Host/Ara.Host.csproj -c Release --artifacts-path "$target/sdk-tools" --disable-build-servers -m:3 -nr:false -p:UseSharedCompilation=false 2>&1 | Tee-Object -FilePath "$profile/host-build.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
& cargo test -p ara-export --test shipping_scripts --locked 2>&1 | Tee-Object -FilePath "$profile/shipping.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
& cargo test -p ara-shader-cook --locked 2>&1 | Tee-Object -FilePath "$profile/shader-tests.log"
exit $LASTEXITCODE
$ErrorActionPreference='Continue'
$target='D:/AraBuild/ux-editor-target'
$source='D:/AraBuild/ux-editor-source'
$profile=Join-Path $target ('editor-lib-windows-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path "$profile/config","$profile/crashes","$profile/temp","$profile/dotnet","$profile/nuget" | Out-Null
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
$env:NUGET_PACKAGES="$profile/nuget"
$env:DOTNET_NOLOGO='1'
$env:DOTNET_CLI_TELEMETRY_OPTOUT='1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE='false'
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1'
$env:ARA_DOTNET_ARTIFACTS="$target/sdk-tools/bin"
$env:ARA_DOTNET_SDK="$source/dotnet"
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $source
& cargo test -p ara-editor --lib --locked 2>&1 | Tee-Object -FilePath "$profile/tests.log"
exit $LASTEXITCODE
param([Parameter(Mandatory=$true)][string]$CheckpointSHA)
$ErrorActionPreference='Continue'
$target='D:/AraBuild/ux-editor-target'
$source='D:/AraBuild/ux-editor-source'
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
& powershell -NoLogo -NoProfile -File "$target/orchestration/build-windows.ps1" -CheckpointSHA $CheckpointSHA
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$profile=Join-Path $target ('windows-async-'+$CheckpointSHA+'-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path "$profile/config","$profile/crashes","$profile/temp","$profile/dotnet","$profile/tests" | Out-Null
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='3'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:ARA_CONFIG_DIR="$profile/config"
$env:ARA_CRASH_DIR="$profile/crashes"
$env:ARA_TEST_ROOT="$profile/tests"
$env:TEMP="$profile/temp"
$env:TMP="$profile/temp"
$env:DOTNET_CLI_HOME="$profile/dotnet"
$env:NUGET_PACKAGES="$target/nuget"
$env:DOTNET_NOLOGO='1'
$env:DOTNET_CLI_TELEMETRY_OPTOUT='1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE='false'
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1'
$env:ARA_DOTNET_ARTIFACTS="$target/sdk-tools/bin"
$env:ARA_DOTNET_SDK="$source/dotnet"
$env:ARA_DOTNET_PREBUILT='1'
$env:ARA_PLAYER="$target/debug/ara-player.exe"
$env:ARA_IMPORT_WORKER="$target/debug/ara-import-worker.exe"
$env:ARA_PLAY_OFFSCREEN='1'
$env:SDL_AUDIODRIVER='dummy'
Set-Location $source
& dotnet build dotnet/src/Ara.CompileServer/Ara.CompileServer.csproj -c Release --artifacts-path "$target/sdk-tools" --disable-build-servers -m:3 -nr:false -p:UseSharedCompilation=false 2>&1 | Tee-Object -FilePath "$profile/sdk-build.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
& dotnet build dotnet/src/Ara.Host/Ara.Host.csproj -c Release --artifacts-path "$target/sdk-tools" --disable-build-servers -m:3 -nr:false -p:UseSharedCompilation=false 2>&1 | Tee-Object -FilePath "$profile/host-build.log"
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
Write-Output 'MATCHING_SDK_COMPLETE'
exit 0

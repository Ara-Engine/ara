param([Parameter(Mandatory=$true)][string]$CheckpointSHA,[string]$BuildProfileCheckpoint=$CheckpointSHA)
$source='D:/AraBuild/ux-editor-source'
$dirty=git -C $source status --porcelain
if ($dirty) {throw "Acceptance source is dirty: $dirty"}
git -C $source checkout --detach $CheckpointSHA
if ($LASTEXITCODE -ne 0) {exit $LASTEXITCODE}
$target='D:/AraBuild/ux-editor-target'
$buildProfileDir=Join-Path $target "windows-build-$BuildProfileCheckpoint"
New-Item -ItemType Directory -Force -Path "$buildProfileDir/config","$buildProfileDir/crashes","$buildProfileDir/temp","$target/dotnet-home" | Out-Null
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='2'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:DOTNET_CLI_HOME="$target/dotnet-home"
$env:NUGET_PACKAGES="$target/nuget"
$env:DOTNET_NOLOGO='1'
$env:DOTNET_CLI_TELEMETRY_OPTOUT='1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE='false'
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1'
$env:ARA_CONFIG_DIR="$buildProfileDir/config"
$env:ARA_CRASH_DIR="$buildProfileDir/crashes"
$env:TEMP="$buildProfileDir/temp"
$env:TMP="$buildProfileDir/temp"
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $source
& 'C:/Users/snowy/.t3/worktrees/snengine/t3-9c4a1f92/target/ara-dev-bootstrap/debug/ara-dev.exe' cargo -- build -p ara-editor -p ara-dev -p ara-import-worker --locked 2>&1 | Tee-Object -FilePath "$target/orchestration/windows-build-$CheckpointSHA.log"
exit $LASTEXITCODE

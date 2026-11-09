$ErrorActionPreference='Continue'
$source='D:/AraBuild/ux-editor-source'
$target='D:/AraBuild/ux-editor-target'
$run="$target/acceptance/219902aa/dotnet-prerequisite"
New-Item -ItemType Directory -Force -Path "$run/config","$run/crashes","$run/temp","$target/dotnet-home" | Out-Null
$env:DOTNET_CLI_HOME="$target/dotnet-home"
$env:DOTNET_CLI_TELEMETRY_OPTOUT='1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE='false'
$env:ARA_CONFIG_DIR="$run/config"
$env:ARA_CRASH_DIR="$run/crashes"
$env:TEMP="$run/temp"
$env:TMP="$run/temp"
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='3'
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $source
& dotnet build dotnet/Ara.slnx -c Release --disable-build-servers -m:3 *> "$run/build.log"
$prereqCode=$LASTEXITCODE
Get-Content "$run/build.log" -Tail 22
if($prereqCode -ne 0){exit $prereqCode}
& powershell -NoProfile -ExecutionPolicy Bypass -File "$target/orchestration/run-native.ps1" -CheckpointSHA 219902aa -Mode behaviours
exit $LASTEXITCODE
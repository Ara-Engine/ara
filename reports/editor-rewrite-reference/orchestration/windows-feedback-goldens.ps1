param([Parameter(Mandatory=$true)][string]$CheckpointSHA)
$ErrorActionPreference='Continue'
$env:PATH='C:/Users/snowy/.cargo/bin;'+$env:PATH
& powershell -NoLogo -NoProfile -File "$PSScriptRoot/build-windows.ps1" -CheckpointSHA $CheckpointSHA
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$target='D:/AraBuild/ux-editor-target'
$profile=Join-Path $target ('acceptance/feedback-windows-'+$CheckpointSHA)
New-Item -ItemType Directory -Force -Path "$profile/config","$profile/crashes","$profile/temp"|Out-Null
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='2'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:ARA_CONFIG_DIR="$profile/config"
$env:ARA_CRASH_DIR="$profile/crashes"
$env:TEMP="$profile/temp"
$env:TMP="$profile/temp"
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location D:/AraBuild/ux-editor-source
& cargo test -p ara-ui-test --test suite --locked 2>&1|Tee-Object -FilePath "$profile/story-goldens.log"
exit $LASTEXITCODE

$ErrorActionPreference='Stop'
$kitSource='D:/AraBuild/ux-editor-source'
$kitTarget='D:/AraBuild/ux-editor-target'
$kitSha=(git -C $kitSource rev-parse --short HEAD).Trim()
$kitRun=Join-Path $kitTarget "acceptance/kit-$kitSha"
New-Item -ItemType Directory -Force -Path "$kitRun/config","$kitRun/crashes","$kitRun/temp","$kitTarget/dotnet-home" | Out-Null
$env:CARGO_TARGET_DIR=$kitTarget
$env:CARGO_BUILD_BUILD_DIR=$kitTarget
$env:CARGO_BUILD_JOBS='3'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
$env:DOTNET_CLI_HOME="$kitTarget/dotnet-home"
$env:ARA_CONFIG_DIR="$kitRun/config"
$env:ARA_CRASH_DIR="$kitRun/crashes"
$env:TEMP="$kitRun/temp"
$env:TMP="$kitRun/temp"
$env:ARA_EDITOR_OFFSCREEN='1'
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
Set-Location $kitSource
$ErrorActionPreference='Continue'
& cargo test -p ara-ui-text -p ara-ui -p ara-ui-document -p ara-ui-test -p ara-ui-stories --locked *> "$kitRun/kit-tests.log"
$kitCode=$LASTEXITCODE
Get-Content "$kitRun/kit-tests.log" -Tail 30
if ($kitCode -ne 0) { exit $kitCode }
& cargo test -p ara-editor --lib --locked *> "$kitRun/editor-lib.log"
$kitCode=$LASTEXITCODE
Get-Content "$kitRun/editor-lib.log" -Tail 50
exit $kitCode

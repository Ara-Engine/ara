$ErrorActionPreference='Stop'
Set-Location 'C:/Users/snowy/.t3/worktrees/snengine/t3-602598ba'
$root="$PWD/target/ara-dev/import-speed/lod-prepared"
if(Test-Path "$PWD/target/ara-dev/import-speed/HOLD"){throw 'hold active'}
$env:CARGO_BUILD_JOBS='2'
$env:CARGO_TARGET_DIR='D:/AraBuild/ara-dev-infra-target'
$env:CARGO_BUILD_BUILD_DIR=$env:CARGO_TARGET_DIR
$env:CARGO_INCREMENTAL='0'
Remove-Item Env:RUSTC_WRAPPER,Env:RUSTC_WORKSPACE_WRAPPER,Env:ARA_SHARED_BUILD_DIR -ErrorAction SilentlyContinue
& "$root/setup.ps1"
$ErrorActionPreference='Continue'
& cargo --config target/ara-dev/import-speed/no-wrapper.toml build --manifest-path "$root/Cargo.toml" --bin ara-lod-prepared --message-format=json-render-diagnostics 1> "$root/build.log" 2> "$root/build.stderr"
$code=$LASTEXITCODE
$code|Set-Content "$root/build.exit"
if($code){exit $code}
$ErrorActionPreference='Stop'
$binary='D:/AraBuild/ara-dev-infra-target/lod-prepared/prototype.exe'
Copy-Item "$env:CARGO_TARGET_DIR/debug/ara-lod-prepared.exe" $binary
@{binary=$binary;binarySha256=(Get-FileHash $binary).Hash;sourceHead=(& git rev-parse HEAD).Trim();manifestSha256=(Get-FileHash "$root/Cargo.toml").Hash;privateImporterLodsSha256=(Get-FileHash 'D:/AraBuild/ara-dev-infra-target/lod-prepared/importers/src/mesh/lods.rs').Hash;nativeSha256=(Get-FileHash 'D:/AraBuild/ara-dev-infra-target/lod-profile/meshopt/vendor/src/simplifier.cpp').Hash;headerSha256=(Get-FileHash "$root/ara_prepared_state.h").Hash;threadBudget=1}|ConvertTo-Json -Depth 10|Set-Content "$root/provenance.json"

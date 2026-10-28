param(
    [ValidateSet('release', 'dev', 'profile')][string]$Configuration = 'release',
    [string]$Zig = $env:ARA_ZIG,
    [string]$AndroidSdk = $env:ANDROID_HOME,
    [string]$Bundletool = $env:ARA_BUNDLETOOL,
    [ValidateSet('windows','linux','android')][string[]]$Platforms = @('windows','linux','android')
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if ($Platforms -contains 'linux' -and !(Test-Path -LiteralPath $Zig)) { throw 'Set ARA_ZIG to Zig 0.17.0/zig.exe.' }
if ($Platforms -contains 'android' -and !(Test-Path -LiteralPath $AndroidSdk)) { throw 'Set ANDROID_HOME to the Android SDK.' }
if ($Platforms -contains 'android' -and !(Test-Path -LiteralPath $Bundletool)) { throw 'Set ARA_BUNDLETOOL to bundletool-all-1.18.3.jar.' }
$ndk = if ($AndroidSdk) { Join-Path $AndroidSdk 'ndk/28.2.13676358/toolchains/llvm/prebuilt/windows-x86_64/bin' } else { $null }
$templates = Join-Path $repo "target/export-templates/$Configuration"
$presets = Join-Path $repo "target/export-presets/$Configuration"
New-Item -ItemType Directory -Force $templates, $presets | Out-Null
$names = @('CARGO_BUILD_JOBS','CARGO_PROFILE_RELEASE_DEBUG','CARGO_PROFILE_RELEASE_STRIP','ARA_ZIG','ARA_ZIGCC','ANDROID_HOME','ARA_BUNDLETOOL','CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER','CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER','CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS','CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER','CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
function CheckedCargo([string[]]$CargoArgs) {
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo failed: $CargoArgs" }
}
try {
    $env:CARGO_BUILD_JOBS = '2'
    $env:CARGO_PROFILE_RELEASE_DEBUG = 'line-tables-only'
    $env:CARGO_PROFILE_RELEASE_STRIP = 'none'
    $env:ARA_ZIG = $Zig
    $env:ANDROID_HOME = $AndroidSdk
    $env:ARA_BUNDLETOOL = $Bundletool
    Push-Location $repo
    CheckedCargo @('build','--release','-p','ara-export','-p','ara-export-installer','--bins')
    $cargoTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    $exporter = Join-Path $cargoTarget 'release/ara-export.exe'
    $env:ARA_ZIGCC = Join-Path $cargoTarget 'release/ara-zigcc.exe'
    $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = $env:ARA_ZIGCC
    if ($Platforms -contains 'android') {
        $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = Join-Path $ndk 'clang.exe'
        $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = '-C link-arg=--target=aarch64-linux-android29 -C link-arg=-Wl,-z,max-page-size=16384'
        $env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER = Join-Path $ndk 'clang.exe'
        $env:CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS = '-C link-arg=--target=x86_64-linux-android29 -C link-arg=-Wl,-z,max-page-size=16384'
    }
    $features = @()
    if ($Configuration -ne 'release') { $features = @('--features','dev-bridge') }
    if ($Platforms -contains 'windows') {
        CheckedCargo (@('build','--release','-p','ara-export-player','--bin','ara-export-smoke') + $features)
        Copy-Item -LiteralPath (Join-Path $cargoTarget 'release/ara-export-smoke.exe') -Destination (Join-Path $templates 'windows-x64.exe')
        Copy-Item -LiteralPath (Join-Path $cargoTarget 'release/ara_export_smoke.pdb') -Destination (Join-Path $templates 'windows-x64.pdb')
    }
    if ($Platforms -contains 'linux') {
        CheckedCargo (@('build','--release','-p','ara-export-player','--bin','ara-export-smoke','--target','x86_64-unknown-linux-gnu') + $features)
        Copy-Item -LiteralPath (Join-Path $cargoTarget 'x86_64-unknown-linux-gnu/release/ara-export-smoke') -Destination (Join-Path $templates 'linux-x64')
    }
    foreach ($triple in $(if ($Platforms -contains 'android') { @('aarch64-linux-android','x86_64-linux-android') } else { @() })) {
        CheckedCargo (@('build','--release','-p','ara-export-player','--lib','--target',$triple) + $features)
        $abi = if ($triple -eq 'aarch64-linux-android') { 'arm64-v8a' } else { 'x86_64' }
        Copy-Item -LiteralPath (Join-Path $cargoTarget "$triple/release/libara_export_player.so") -Destination (Join-Path $templates "android-$abi.so")
    }
    foreach ($platform in $Platforms) {
        $arches = if ($platform -eq 'android') { @('arm64-v8a','x86_64') } else { @('x64') }
        $pins = @()
        foreach ($arch in $arches) {
            $filename = if ($platform -eq 'windows') { 'windows-x64.exe' } elseif ($platform -eq 'linux') { 'linux-x64' } else { "android-$arch.so" }
            $symbolArgs = if ($platform -eq 'windows') { @('--symbols',(Join-Path $templates 'windows-x64.pdb')) } else { @() }
            $pin = & $exporter pin --platform $platform --arch $arch --config $Configuration --file (Join-Path $templates $filename) @symbolArgs
            if ($LASTEXITCODE -ne 0) { throw 'template pin failed' }
            $pins += ($pin | ConvertFrom-Json)
        }
        $platformName = if ($platform -eq 'windows') { 'Windows' } elseif ($platform -eq 'linux') { 'Linux' } else { 'Android' }
        $configName = if ($Configuration -eq 'release') { 'Release' } elseif ($Configuration -eq 'dev') { 'Dev' } else { 'Profile' }
        $preset = @{ '$schema'='ara://schema/export/1'; platform=$platform; config=$Configuration; scriptProject='Game.csproj'; assembly='Game'; content='content'; templates=$pins }
        $path = Join-Path $presets "$platform.export.json"
        [IO.File]::WriteAllText($path, ($preset | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding $false))
        & $exporter preset-fmt $path
        if ($LASTEXITCODE -ne 0) { throw 'preset canonicalization failed' }
    }
    Write-Output "Prepared templates/presets: $presets"
} finally {
    Pop-Location
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}

# Keep the small xtask bootstrap out of the contended engine build directory.
$workspaceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$bootstrapBuild = (Join-Path $env:LOCALAPPDATA 'ara/dev-build').Replace('\', '/')
$bootstrapTarget = Join-Path $workspaceRoot 'target/ara-dev-bootstrap'
$oldTarget = $env:CARGO_TARGET_DIR
$oldBuild = $env:CARGO_BUILD_BUILD_DIR
$oldJobs = $env:CARGO_BUILD_JOBS
$oldWrapper = $env:RUSTC_WRAPPER
$bootstrapConfig = @()
if ($args.Count -gt 0 -and $args[0] -in @('check', 'merge') -and $args -contains '--no-compiler-cache') {
    # CLI config clears a configured wrapper; remove an environment override too.
    $env:RUSTC_WRAPPER = $null
    $bootstrapConfig = @('--config', "build.rustc-wrapper=''")
}
$env:CARGO_BUILD_JOBS = '2'
try {
    $metadataText = & cargo @bootstrapConfig metadata --manifest-path (Join-Path $workspaceRoot 'Cargo.toml') --format-version 1 --no-deps --locked --offline
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $metadata = $metadataText | ConvertFrom-Json
    # Hashed intermediates stay shared; final executable names are worktree-local.
    if (!$env:CARGO_BUILD_BUILD_DIR) { $env:CARGO_BUILD_BUILD_DIR = $metadata.target_directory }
    $env:CARGO_TARGET_DIR = Join-Path $workspaceRoot 'target'
    & cargo @bootstrapConfig run -p ara-dev --manifest-path (Join-Path $workspaceRoot 'Cargo.toml') --target-dir $bootstrapTarget --config "build.build-dir='$bootstrapBuild'" --config "target.x86_64-pc-windows-msvc.linker='rust-lld.exe'" -- @args
    exit $LASTEXITCODE
} finally {
    $env:CARGO_TARGET_DIR = $oldTarget
    $env:CARGO_BUILD_BUILD_DIR = $oldBuild
    $env:CARGO_BUILD_JOBS = $oldJobs
    $env:RUSTC_WRAPPER = $oldWrapper
}

# Fetches the pinned Jolt Physics source into vendor/ (gitignored).
# jolt-sys/build.rs compiles it with the `cc` crate; no CMake is needed.
$ErrorActionPreference = 'Stop'
$JoltRev = 'e77f175595e64cb44218cc9d9d56fc365ad0e36a' # tag v5.6.0 (2026-07-11)
$Dest = Join-Path $PSScriptRoot 'vendor/JoltPhysics'

if (Test-Path (Join-Path $Dest '.git')) {
    $have = (git -C $Dest rev-parse HEAD).Trim()
    if ($have -eq $JoltRev) { Write-Host "Jolt already at $JoltRev"; exit 0 }
    Write-Host "Jolt at $have, moving to $JoltRev"
} else {
    New-Item -ItemType Directory -Force $Dest | Out-Null
    git -C $Dest init -q
    git -C $Dest remote add origin https://github.com/jrouwe/JoltPhysics.git
}
$env:GIT_LFS_SKIP_SMUDGE = '1'
git -C $Dest fetch -q --depth 1 origin $JoltRev
git -C $Dest checkout -q --detach FETCH_HEAD
Write-Host "Jolt checked out at $((git -C $Dest rev-parse HEAD).Trim())"

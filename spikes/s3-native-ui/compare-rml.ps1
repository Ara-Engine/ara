# Local, pinned, font-free RmlUi build and integration smoke. No global installation.
$ErrorActionPreference = 'Stop'
$spike = $PSScriptRoot
$vendor = Join-Path $spike '.vendor'
$revision = '821cbd229d804cb1c905b729af86c21133abceee'
New-Item -ItemType Directory -Force $vendor | Out-Null
$cmake = Join-Path $vendor 'python/cmake/data/bin/cmake.exe'
if (-not (Test-Path -LiteralPath $cmake)) {
    python -m pip install --target (Join-Path $vendor 'python') cmake==4.3.1 --disable-pip-version-check
    if ($LASTEXITCODE -ne 0) { throw 'Local CMake installation failed' }
}
$source = Join-Path $vendor 'RmlUi'
if (-not (Test-Path -LiteralPath (Join-Path $source '.git'))) {
    git init $source
    git -C $source remote add origin https://github.com/mikke89/RmlUi.git
    git -C $source fetch --depth 1 origin $revision
    if ($LASTEXITCODE -ne 0) { throw 'RmlUi fetch failed' }
    git -C $source checkout --detach FETCH_HEAD
}
if ((git -C $source rev-parse HEAD) -ne $revision) { throw "Expected RmlUi $revision" }
$build = Join-Path $vendor 'rml-build18'
& $cmake -S $source -B $build -G 'Visual Studio 18 2026' -A x64 -DRMLUI_FONT_ENGINE=none -DRMLUI_SAMPLES=OFF -DBUILD_SHARED_LIBS=OFF -DRMLUI_COMPILER_OPTIONS=OFF
if ($LASTEXITCODE -ne 0) { throw 'RmlUi configure failed' }
& $cmake --build $build --config Release --parallel 4
if ($LASTEXITCODE -ne 0) { throw 'RmlUi build failed' }
$smoke = Join-Path $vendor 'smoke-build'
& $cmake -S (Join-Path $spike 'rml-smoke') -B $smoke -G 'Visual Studio 18 2026' -A x64
if ($LASTEXITCODE -ne 0) { throw 'Smoke configure failed' }
& $cmake --build $smoke --config Release --parallel 4
if ($LASTEXITCODE -ne 0) { throw 'Smoke build failed' }
$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = Join-Path $smoke 'Release/ara_rml_smoke.exe'
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$process = [System.Diagnostics.Process]::Start($info)
$stdout = $process.StandardOutput.ReadToEnd()
$stderr = $process.StandardError.ReadToEnd()
$process.WaitForExit()
if ($process.ExitCode -ne 0) { throw "RmlUi smoke failed: $stderr" }
[IO.File]::WriteAllText((Join-Path $spike 'evidence/rmlui-smoke.json'), $stdout)
[IO.File]::WriteAllText((Join-Path $spike 'evidence/rmlui-stderr.txt'), $stderr)
$process.Dispose()
Write-Output $stdout

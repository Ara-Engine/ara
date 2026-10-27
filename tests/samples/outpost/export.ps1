param(
    [string]$Bin = $(if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR 'debug' } else { Join-Path $PSScriptRoot '../../../target/debug' }),
    [string]$Output = $(Join-Path $PSScriptRoot "../../../target/outpost-export/$([guid]::NewGuid().ToString('N'))")
)
$ErrorActionPreference = 'Stop'
$engine = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$project = Join-Path $engine 'samples/outpost'
$settings = Get-Content -Raw -LiteralPath (Join-Path $project 'settings/builds/windows.export.json') | ConvertFrom-Json
$Bin = [IO.Path]::GetFullPath($Bin)
$exporter = Join-Path $Bin 'ara-export.exe'
$player = Join-Path $Bin 'ara-player.exe'
$symbols = Join-Path $Bin 'ara_player.pdb'
if (!(Test-Path -LiteralPath $symbols)) { $symbols = Join-Path $Bin 'ara-player.pdb' }
foreach ($file in @($exporter,$player,$symbols)) {
    if (!(Test-Path -LiteralPath $file)) { throw "Missing build input: $file. See samples/README.md (Outpost Windows export)." }
}
$work = Join-Path $engine "target/outpost-export-tools/$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $work -Force | Out-Null
$env:ARA_CONFIG_DIR = Join-Path $work 'profile'
$env:ARA_CRASH_DIR = Join-Path $work 'crashes'
$env:ARA_IMPORT_WORKER = Join-Path $Bin 'ara-import-worker.exe'
$env:ARA_DOTNET_SDK = Join-Path $engine 'dotnet'
$env:ARA_DOTNET_ARTIFACTS = Join-Path $engine 'dotnet/artifacts/bin'
$bundle = Join-Path $work 'runtime.zip'
& $exporter runtime-template --player $player --engine $engine --output $bundle
if ($LASTEXITCODE) { throw 'Runtime template failed' }
$pin = & $exporter pin --platform windows --arch x64 --config $settings.config --file $bundle --symbols $symbols
if ($LASTEXITCODE) { throw 'Runtime pin failed' }
$preset = Join-Path $work 'outpost.export.json'
$settings.templates = @((($pin -join "`n") | ConvertFrom-Json))
[IO.File]::WriteAllText($preset,($settings | ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
& $exporter preset-fmt $preset
if ($LASTEXITCODE) { throw 'Export preset validation failed' }
& $exporter export --project $project --preset $preset --output $Output --cache (Join-Path $work 'template-cache')
if ($LASTEXITCODE) { throw "Export failed; logs remain under $Output" }
$build = Join-Path $Output 'build'
Copy-Item -LiteralPath (Join-Path $engine 'THIRD_PARTY_NOTICES.md') -Destination (Join-Path $build 'THIRD_PARTY_NOTICES.md')
Copy-Item -LiteralPath (Join-Path $engine 'samples/THIRD_PARTY_ASSETS.md') -Destination (Join-Path $build 'THIRD_PARTY_ASSETS.md')
Write-Output "Outpost Windows build: $build (open Outpost.exe)"

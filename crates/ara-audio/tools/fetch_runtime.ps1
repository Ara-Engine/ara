# Optional runtime setup. Cargo never calls this script or downloads native binaries.
param([ValidateSet('windows-x64','linux-x64','osx','android-armv8','ios')][string]$Platform = 'windows-x64')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$runtimeRoot = Join-Path (Split-Path $PSScriptRoot -Parent) 'runtime'
[void][IO.Directory]::CreateDirectory($runtimeRoot)
function Get-PinnedArchive([string]$Name, [string]$Url, [string]$Hash) {
    $archivePath = Join-Path $runtimeRoot $Name
    if (-not (Test-Path -LiteralPath $archivePath) -or
        (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ne $Hash) {
        $partialPath = "$archivePath.partial"
        Invoke-WebRequest -Uri $Url -OutFile $partialPath -UseBasicParsing
        if ((Get-FileHash -LiteralPath $partialPath -Algorithm SHA256).Hash -ne $Hash) {
            throw "Runtime hash mismatch: $Name"
        }
        Move-Item -LiteralPath $partialPath -Destination $archivePath -Force
    }
    return $archivePath
}
function Expand-Selected([string]$Archive, [string]$Directory, [scriptblock]$Accept) {
    $destinationRoot = [IO.Path]::GetFullPath($Directory) + [IO.Path]::DirectorySeparatorChar
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            if ($name.EndsWith('/') -or -not (& $Accept $name)) { continue }
            $target = [IO.Path]::GetFullPath((Join-Path $Directory $name))
            if (-not $target.StartsWith($destinationRoot, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Runtime entry escapes destination: $name"
            }
            [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
        }
    } finally { $zip.Dispose() }
}
$steam = Get-PinnedArchive 'steam.zip' 'https://github.com/ValveSoftware/steam-audio/releases/download/v4.8.1/steamaudio_4.8.1.zip' '4a0aa5ec1176f38f0b0993a37c2259d9e86f27e22d5e24f83ec4c3cb9a1d5449'
Expand-Selected $steam (Join-Path $runtimeRoot 'steam') {
    param($name)
    $name.StartsWith("steamaudio/lib/$Platform/") -or
    $name -in @('steamaudio/THIRDPARTY.md','steamaudio/include/phonon.h','steamaudio/include/phonon_version.h')
}
if ($Platform -eq 'windows-x64') {
    $sdl = Get-PinnedArchive 'sdl.zip' 'https://github.com/libsdl-org/SDL/releases/download/release-3.4.18/SDL3-3.4.18-win32-x64.zip' '75c2c0fc74e7d1206aaedc22893a35887332f4f85f5c8b74fddc10d177985f2c'
    Expand-Selected $sdl (Join-Path $runtimeRoot 'sdl') {
        param($name)
        $name -in @('SDL3.dll','LICENSE.txt','README.md')
    }
}
Write-Output "Optional runtimes ready: $runtimeRoot ($Platform). No Cargo build uses these files."

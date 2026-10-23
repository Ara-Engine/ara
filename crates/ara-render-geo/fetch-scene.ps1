param([string]$Destination = (Join-Path $PSScriptRoot 'captures/assets/coast_rocks_01'))
$ErrorActionPreference = 'Stop'
$base = 'https://dl.polyhaven.org/file/ph-assets/Models'
$files = @(
    @('coast_rocks_01_2k.gltf', "$base/gltf/2k/coast_rocks_01/coast_rocks_01_2k.gltf", 'd763b6ff59061342a2e0b85c43abecf6'),
    @('coast_rocks_01.bin', "$base/gltf/8k/coast_rocks_01/coast_rocks_01.bin", '4cf9d5009388434a0520e1643127a725'),
    @('textures/coast_rocks_01_diff_2k.jpg', "$base/jpg/2k/coast_rocks_01/coast_rocks_01_diff_2k.jpg", '3f00b2ef63ccea7952236f1c6e32fa58'),
    @('textures/coast_rocks_01_arm_2k.jpg', "$base/jpg/2k/coast_rocks_01/coast_rocks_01_arm_2k.jpg", 'e29f84556de4da79f3c08ddee3b7ad42'),
    @('textures/coast_rocks_01_nor_gl_2k.jpg', "$base/jpg/2k/coast_rocks_01/coast_rocks_01_nor_gl_2k.jpg", '1a06be557b5709d355e20a3fea390c2a'),
    @('textures/coast_rocks_01_diff_8k.jpg', "$base/jpg/8k/coast_rocks_01/coast_rocks_01_diff_8k.jpg", 'c92e50f2b1ade3a128a02b78fc65ee2a')
)
foreach($file in $files) {
    $path = Join-Path $Destination $file[0]
    New-Item -ItemType Directory -Force (Split-Path -Parent $path) | Out-Null
    if(-not (Test-Path -LiteralPath $path)) {
        Write-Host "Fetching $($file[0])"
        Invoke-WebRequest -UseBasicParsing $file[1] -OutFile $path
    }
    if((Get-FileHash -LiteralPath $path -Algorithm MD5).Hash.ToLowerInvariant() -ne $file[2]) {
        throw "Checksum mismatch for $path; remove the invalid file and fetch again."
    }
}
Write-Host "CC0 scan ready: $Destination (see ASSET-LICENSES.md)"

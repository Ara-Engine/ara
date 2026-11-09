param(
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$Output,
    [string[]]$Cases=@()
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$outputPath=[IO.Path]::GetFullPath($Output)
[IO.Directory]::CreateDirectory($outputPath) | Out-Null
$font=New-Object Drawing.Font('Segoe UI',10)
$brush=[Drawing.Brushes]::White
try {
    foreach($case in Get-ChildItem -LiteralPath (Join-Path $Root 'cases') -Directory) {
        if($Cases.Count -gt 0 -and $case.Name -notin $Cases){continue}
        $runPath=Join-Path $case.FullName 'capped/run.json'
        if(-not(Test-Path -LiteralPath $runPath)){continue}
        $run=Get-Content -LiteralPath $runPath -Raw | ConvertFrom-Json
        $frames=@($run.frames | Where-Object {$_.capture -and $_.reference})
        for($page=0;$page*9 -lt $frames.Count;$page++) {
            $batch=@($frames | Select-Object -Skip ($page*9) -First 9)
            $rows=[Math]::Ceiling($batch.Count/3)
            $sheet=New-Object Drawing.Bitmap(1920,([int]($rows*204+30)))
            $g=[Drawing.Graphics]::FromImage($sheet)
            try {
                $g.Clear([Drawing.Color]::FromArgb(24,24,24))
                $g.DrawString("$($case.Name) | raster / pathtrace | page $($page+1)",$font,$brush,4,4)
                for($i=0;$i -lt $batch.Count;$i++) {
                    $row=[Math]::Floor($i/3); $col=$i%3
                    for($side=0;$side -lt 2;$side++) {
                        $file=if($side -eq 0){$batch[$i].capture}else{$batch[$i].reference}
                        $picture=[Drawing.Image]::FromFile($file)
                        try {
                            $x=[int]($col*640+$side*320); $y=[int](30+$row*204)
                            $g.DrawString("frame $($batch[$i].frame) $([IO.Path]::GetFileName($file))",$font,$brush,$x,$y)
                            $g.DrawImage($picture,$x,($y+24),320,180)
                        } finally {$picture.Dispose()}
                    }
                }
                $sheet.Save((Join-Path $outputPath "$($case.Name)-$page.png"),[Drawing.Imaging.ImageFormat]::Png)
            } finally {$g.Dispose();$sheet.Dispose()}
        }
    }
} finally {$font.Dispose()}
